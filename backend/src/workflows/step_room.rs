//! A workflow Agent step with a `room_id` (KT-793): the runner mints a
//! capability for the step's run, hands it to the agent's bridge through the
//! environment, and the bridge exchanges it for a membership of that room.
//!
//! The capability lives only in this process, so it is valid exactly while
//! the step runs here: a finished step, another step, another run or a
//! restarted backend holds nothing the join endpoint accepts.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::agents::run_progress::{LiveRuns, RunIdentity, RunProgress, StopOnDrop};
use crate::agents::runner::WorkflowStepBridgeContext;
use crate::db::discussion_sessions::sha256_hex;
use crate::db::Database;
use crate::models::{
    ActiveWorkflowStep, AgentRunStop, AgentType, RunStatus, StepResult, StepType, Workflow,
    WorkflowStep, WorkflowStepIdentity, WsMessage,
};
use crate::workflows::steps::StepOutcome;
use crate::workflows::template::TemplateContext;

struct LiveStep {
    step_key: String,
    disc_id: String,
    capability_hash: String,
    sessions: Vec<i64>,
    /// The `run_id` of the step's `AgentRunProgress` frames in the room.
    progress_run_id: String,
}

/// A run that will enter a room with an Agent step, and what it executes
/// until then (KT-1252): the room shows it so a preparation never reads as a
/// stall.
#[derive(Clone)]
struct RoomTarget {
    disc_id: String,
    workflow_id: String,
    workflow_name: String,
    step_key: String,
    step_name: String,
    agent_type: AgentType,
    started_at: String,
}

impl RoomTarget {
    fn same_as(&self, other: &Self) -> bool {
        self.disc_id == other.disc_id
            && self.workflow_id == other.workflow_id
            && self.step_key == other.step_key
            && self.step_name == other.step_name
            && self.agent_type == other.agent_type
    }
}

/// Steps currently running with a room, keyed by run id.
#[derive(Default)]
pub struct WorkflowStepRooms {
    live: Mutex<HashMap<String, LiveStep>>,
    /// Runs heading for a room, keyed by run id.
    targets: Mutex<HashMap<String, RoomTarget>>,
    /// Where a room is told its steps changed (KT-1252); `None` in unit tests.
    ws: Option<Arc<tokio::sync::broadcast::Sender<WsMessage>>>,
    /// Where a step's live progress is readable for a room opened mid-run.
    live_runs: Arc<LiveRuns>,
}

impl WorkflowStepRooms {
    /// Rooms that tell the local WebSocket when a step starts or ends in them
    /// and publish each step's live progress on `live_runs`.
    pub fn publishing(
        ws: Arc<tokio::sync::broadcast::Sender<WsMessage>>,
        live_runs: Arc<LiveRuns>,
    ) -> Self {
        Self {
            ws: Some(ws),
            live_runs,
            ..Self::default()
        }
    }

    /// Tell the open room of `disc_id` to re-read its steps.
    fn notify(&self, disc_id: &str) {
        if let Some(ws) = &self.ws {
            let _ = ws.send(WsMessage::WorkflowStepRoomChanged {
                discussion_id: disc_id.to_string(),
            });
        }
    }

    fn targets(&self) -> std::sync::MutexGuard<'_, HashMap<String, RoomTarget>> {
        self.targets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Record (or clear) the room `run_id` is heading for; the room it leaves
    /// and the one it reaches are told when something changed.
    fn set_target(&self, run_id: &str, next: Option<RoomTarget>) {
        let previous = {
            let mut targets = self.targets();
            let previous = targets.get(run_id).cloned();
            match (&previous, &next) {
                (Some(old), Some(new)) if old.same_as(new) => return,
                (None, None) => return,
                _ => {}
            }
            match &next {
                Some(target) => {
                    targets.insert(run_id.to_string(), target.clone());
                }
                None => {
                    targets.remove(run_id);
                }
            }
            previous
        };
        if let Some(old) = &previous {
            if next.as_ref().map(|new| new.disc_id.as_str()) != Some(old.disc_id.as_str()) {
                self.notify(&old.disc_id);
            }
        }
        if let Some(new) = &next {
            self.notify(&new.disc_id);
        }
    }

    /// Clear the room `run_id` is heading for, only while it is still the one
    /// announced for `step_key`: a step finishing late (its `Drop` spawns the
    /// write) must not erase what the run announced for a later step.
    fn clear_target_of(&self, run_id: &str, step_key: &str) {
        let removed = {
            let mut targets = self.targets();
            if targets
                .get(run_id)
                .is_some_and(|target| target.step_key == step_key)
            {
                targets.remove(run_id)
            } else {
                None
            }
        };
        if let Some(old) = removed {
            self.notify(&old.disc_id);
        }
    }

    /// `active` (the room's persisted steps) with each one's live-progress
    /// run, then the running runs that head for `disc_id` without an agent in
    /// it yet.
    pub fn project(
        &self,
        conn: &rusqlite::Connection,
        disc_id: &str,
        mut active: Vec<ActiveWorkflowStep>,
    ) -> Vec<ActiveWorkflowStep> {
        {
            let live = self.live();
            for step in &mut active {
                step.progress_run_id = live
                    .get(&step.identity.run_id)
                    .filter(|entry| entry.step_key == step.identity.step_key)
                    .map(|entry| entry.progress_run_id.clone());
            }
        }
        let mut waiting: Vec<(String, RoomTarget)> = self
            .targets()
            .iter()
            .filter(|(run_id, target)| {
                target.disc_id == disc_id
                    && !active.iter().any(|step| &step.identity.run_id == *run_id)
            })
            .map(|(run_id, target)| (run_id.clone(), target.clone()))
            .collect();
        waiting.sort_by(|a, b| (&a.1.started_at, &a.0).cmp(&(&b.1.started_at, &b.0)));
        for (run_id, target) in waiting {
            let running = conn
                .query_row(
                    "SELECT status FROM workflow_runs WHERE id = ?1",
                    [&run_id],
                    |row| row.get::<_, String>(0),
                )
                .map(|status| status == "Running")
                .unwrap_or(false);
            if !running {
                continue;
            }
            active.push(ActiveWorkflowStep {
                identity: WorkflowStepIdentity {
                    run_id,
                    workflow_id: target.workflow_id,
                    workflow_name: target.workflow_name,
                    step_key: target.step_key,
                    step_name: target.step_name,
                },
                agent_type: target.agent_type,
                started_at: target.started_at,
                progress_run_id: None,
                preparing: true,
            });
        }
        active
    }

    fn live(&self) -> std::sync::MutexGuard<'_, HashMap<String, LiveStep>> {
        self.live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn matches(entry: &LiveStep, context: &WorkflowStepBridgeContext) -> bool {
        entry.capability_hash == sha256_hex(&context.capability)
            && entry.step_key == context.step_key
            && entry.disc_id == context.discussion_id
    }

    /// Whether `context` is the capability of the step running now for its run.
    pub fn authorize(&self, context: &WorkflowStepBridgeContext) -> bool {
        self.live()
            .get(&context.run_id)
            .is_some_and(|entry| Self::matches(entry, context))
    }

    /// Record a session joined with `context` so it leaves with the step.
    /// `false` when the step ended in between: the caller must revoke it.
    pub fn attach(&self, context: &WorkflowStepBridgeContext, session_pk: i64) -> bool {
        match self.live().get_mut(&context.run_id) {
            Some(entry) if Self::matches(entry, context) => {
                if !entry.sessions.contains(&session_pk) {
                    entry.sessions.push(session_pk);
                }
                true
            }
            _ => false,
        }
    }

    fn insert(&self, run_id: &str, entry: LiveStep) -> Vec<i64> {
        self.live()
            .insert(run_id.to_string(), entry)
            .map(|previous| previous.sessions)
            .unwrap_or_default()
    }

    fn release(&self, run_id: &str, capability_hash: &str) -> Option<Vec<i64>> {
        let mut live = self.live();
        if live
            .get(run_id)
            .is_some_and(|entry| entry.capability_hash == capability_hash)
        {
            live.remove(run_id).map(|entry| entry.sessions)
        } else {
            None
        }
    }
}

/// One running step's capability. Dropping it (step end, cancellation) ends
/// the step's room membership.
pub struct StepRoomActivation {
    rooms: Arc<WorkflowStepRooms>,
    db: Arc<Database>,
    context: WorkflowStepBridgeContext,
    capability_hash: String,
    released: bool,
    /// The step's live progress in the room (KT-1252). Dropping the step
    /// without a verdict stops it as failed, so no bubble keeps counting.
    progress: RunProgress,
    _progress_stop: StopOnDrop,
}

impl StepRoomActivation {
    pub fn context(&self) -> &WorkflowStepBridgeContext {
        &self.context
    }

    /// What the step reports for the room's live bubble.
    pub fn progress(&self) -> &RunProgress {
        &self.progress
    }

    /// End the membership now and wait for it, rather than from `Drop`.
    pub async fn finish(self) {
        self.finish_as(AgentRunStop::Finished).await;
    }

    /// [`Self::finish`], saying how the step's live progress ended.
    pub async fn finish_as(mut self, stop: AgentRunStop) {
        self.released = true;
        self.progress.stop(stop);
        let released = self
            .rooms
            .release(&self.context.run_id, &self.capability_hash);
        if let Some(sessions) = released {
            finish_activity(self.rooms.clone(), &self.db, self.context.clone(), sessions).await;
        }
    }
}

impl Drop for StepRoomActivation {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let released = self
            .rooms
            .release(&self.context.run_id, &self.capability_hash);
        let Some(sessions) = released else {
            return;
        };
        let context = self.context.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(finish_activity(
                    self.rooms.clone(),
                    self.db.clone(),
                    context,
                    sessions,
                ));
            }
            Err(_) => tracing::warn!(
                run_id = %self.context.run_id,
                "workflow step room activity left active: no runtime to finish it"
            ),
        }
    }
}

async fn finish_activity(
    rooms: Arc<WorkflowStepRooms>,
    db: impl AsRef<Database>,
    context: WorkflowStepBridgeContext,
    sessions: Vec<i64>,
) {
    let run_id = context.run_id.clone();
    let step_key = context.step_key.clone();
    if let Err(error) = db
        .as_ref()
        .with_conn(move |conn| {
            crate::db::workflow_step_rooms::finish_activity(conn, &run_id, &step_key)?;
            crate::db::workflow_step_rooms::revoke(conn, &sessions)?;
            Ok(())
        })
        .await
    {
        tracing::warn!(%error, "could not finish a workflow step's room activity");
    }
    // The run's agent has been in the room: what it executes next is not the
    // room's preparation (the next step announces itself when a later Agent
    // step still has one). Told even when the write failed: the room re-reads
    // what is true. Only this step's own announcement goes: a later one, made
    // while this write was pending, stays.
    rooms.clear_target_of(&context.run_id, &context.step_key);
    rooms.notify(&context.discussion_id);
}

async fn revoke(db: impl AsRef<Database>, sessions: Vec<i64>) {
    if sessions.is_empty() {
        return;
    }
    if let Err(error) = db
        .as_ref()
        .with_conn(move |conn| crate::db::workflow_step_rooms::revoke(conn, &sessions))
        .await
    {
        tracing::warn!(%error, "could not revoke a workflow step's room sessions");
    }
}

fn step_key(step: &WorkflowStep) -> String {
    step.id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or(&step.name)
        .to_string()
}

/// Render `step.room_id` and register the step's capability for `run_id`.
/// `Ok(None)` for a step without a room; an unknown room refuses the launch.
pub async fn activate(
    rooms: &Arc<WorkflowStepRooms>,
    db: &Arc<Database>,
    run_id: &str,
    run_project: Option<&str>,
    step: &WorkflowStep,
    ctx: &TemplateContext,
) -> Result<Option<StepRoomActivation>, String> {
    let Some(template) = step
        .room_id
        .as_deref()
        .map(str::trim)
        .filter(|template| !template.is_empty())
    else {
        return Ok(None);
    };
    // Only their bridges receive the capability (direct CLI and ACP adapter).
    if !matches!(step.agent, AgentType::ClaudeCode | AgentType::Codex) {
        return Err(format!(
            "room_id: a {:?} step has no Kronn bridge to carry the room capability; use Claude Code or Codex",
            step.agent
        ));
    }
    let room = ctx
        .render_strict(template)
        .map_err(|error| format!("room_id: {error}"))?
        .trim()
        .to_string();
    if room.is_empty() {
        return Err("room_id rendered to an empty discussion id".into());
    }
    let found = {
        let room = room.clone();
        db.with_read_conn(move |conn| {
            Ok(crate::db::discussions::get_discussion(conn, &room)?
                .map(|discussion| discussion.project_id))
        })
        .await
        .map_err(|error| format!("room_id: {error}"))?
    };
    let Some(room_project) = found else {
        return Err(format!("room_id: discussion `{room}` not found"));
    };
    if !super::run_scope::target_allowed(
        room_project.as_deref(),
        run_project,
        super::run_scope::is_literal(template),
    ) {
        return Err(format!(
            "room_id: discussion `{room}` is outside this run's project"
        ));
    }
    let capability = format!(
        "kr-step-{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let capability_hash = sha256_hex(&capability);
    let context = WorkflowStepBridgeContext {
        discussion_id: room.clone(),
        run_id: run_id.to_string(),
        step_key: step_key(step),
        capability,
    };
    // The step's live progress in the room, in the schema of a reply's own
    // (KT-1108): a native reply's run has a dispatch and a trigger, a step's
    // has neither.
    let progress = RunProgress::new();
    let identity = RunIdentity {
        discussion_id: room.clone(),
        dispatch_id: None,
        trigger_message_id: None,
        agent_type: step.agent.clone(),
        run_id: uuid::Uuid::new_v4().to_string(),
        started_at: chrono::Utc::now(),
    };
    let replaced = rooms.insert(
        run_id,
        LiveStep {
            step_key: context.step_key.clone(),
            disc_id: room,
            capability_hash: capability_hash.clone(),
            sessions: Vec::new(),
            progress_run_id: identity.run_id.clone(),
        },
    );
    let activity = {
        let run_id = run_id.to_string();
        let step_key = context.step_key.clone();
        let step_name = step.name.clone();
        let disc_id = context.discussion_id.clone();
        let agent_type = format!("{:?}", step.agent);
        db.with_conn(move |conn| {
            crate::db::workflow_step_rooms::begin_activity(
                conn,
                &run_id,
                &step_key,
                &step_name,
                &disc_id,
                &agent_type,
            )
        })
        .await
    };
    if let Err(error) = activity {
        let _ = rooms.release(run_id, &capability_hash);
        revoke(db.clone(), replaced).await;
        return Err(format!("room_id: could not record step activity: {error}"));
    }
    revoke(db.clone(), replaced).await;
    // Readable by a room opened from now on, before the first frame is out.
    rooms.live_runs.insert(identity.clone(), progress.clone());
    let ws = rooms.ws.clone();
    tokio::spawn(crate::agents::run_progress::publish_registered(
        rooms.live_runs.clone(),
        identity,
        progress.clone(),
        move |frame| {
            if let Some(ws) = &ws {
                let _ = ws.send(frame);
            }
        },
    ));
    // The placeholder: the open room re-reads its steps now, not at its poll.
    rooms.notify(&context.discussion_id);
    Ok(Some(StepRoomActivation {
        rooms: rooms.clone(),
        db: db.clone(),
        context,
        capability_hash,
        released: false,
        _progress_stop: StopOnDrop(progress.clone()),
        progress,
    }))
}

/// Keeps a run's room — the one its next Agent step will enter — told which
/// step the run executes meanwhile (KT-1252). Dropping it ends the run's claim
/// on the room, whatever way the run ends.
pub struct RoomTargetWatch {
    rooms: Arc<WorkflowStepRooms>,
    run_id: String,
}

impl RoomTargetWatch {
    pub fn new(rooms: &Arc<WorkflowStepRooms>, run_id: &str) -> Self {
        Self {
            rooms: rooms.clone(),
            run_id: run_id.to_string(),
        }
    }

    /// The run is about to execute `workflow.steps[step_idx]`: the room of the
    /// first Agent step from there on whose `room_id` renders to a room of
    /// this run's scope shows the workflow and that step. A run with no such
    /// step left (the room's agent already ran) claims no room.
    pub async fn announce(
        &self,
        db: &Arc<Database>,
        run_project: Option<&str>,
        workflow: &Workflow,
        step_idx: usize,
        ctx: &TemplateContext,
    ) {
        let Some(current) = workflow.steps.get(step_idx) else {
            self.rooms.set_target(&self.run_id, None);
            return;
        };
        let mut target = None;
        for step in &workflow.steps[step_idx..] {
            let Some(template) = step
                .room_id
                .as_deref()
                .map(str::trim)
                .filter(|template| !template.is_empty())
            else {
                continue;
            };
            if !matches!(step.step_type, StepType::Agent)
                || !matches!(step.agent, AgentType::ClaudeCode | AgentType::Codex)
            {
                continue;
            }
            // Not known yet (a step that has not run): look at the next one.
            let Ok(rendered) = ctx.render_strict(template) else {
                continue;
            };
            let room = rendered.trim().to_string();
            if room.is_empty() {
                continue;
            }
            let lookup = {
                let room = room.clone();
                db.with_read_conn(move |conn| {
                    Ok(crate::db::discussions::get_discussion(conn, &room)?
                        .map(|discussion| discussion.project_id))
                })
                .await
            };
            let Ok(Some(room_project)) = lookup else {
                continue;
            };
            if !super::run_scope::target_allowed(
                room_project.as_deref(),
                run_project,
                super::run_scope::is_literal(template),
            ) {
                continue;
            }
            target = Some(RoomTarget {
                disc_id: room,
                workflow_id: workflow.id.clone(),
                workflow_name: workflow.name.clone(),
                step_key: step_key(current),
                step_name: current.name.clone(),
                agent_type: step.agent.clone(),
                started_at: chrono::Utc::now().to_rfc3339(),
            });
            break;
        }
        self.rooms.set_target(&self.run_id, target);
    }
}

impl Drop for RoomTargetWatch {
    fn drop(&mut self) {
        self.rooms.set_target(&self.run_id, None);
    }
}

/// A step whose room cannot be resolved fails before any agent starts.
pub fn refused_outcome(step: &WorkflowStep, error: String, duration_ms: u64) -> StepOutcome {
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Failed,
            output: error,
            tokens_used: Some(0),
            duration_ms,
            started_at: None,
            condition_result: None,
            envelope_detected: None,
            step_kind: Some("preflight_failed".into()),
            step_agent: Some(step.agent.clone()),
            step_model: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
            is_rollback: false,
            child_run_id: None,
            agent_provenance: None,
            native_tool_calls: Box::default(),
            cached_prompt_tokens: None,
            cache_write_prompt_tokens: None,
            last_activity: None,
            quota_wait: None,
            terminal_stop: None,
        },
        condition_action: None,
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::models::StepType;

    async fn db_with_room() -> Arc<Database> {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('room-a', 'A', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )?;
            conn.execute(
                "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
                 VALUES ('workflow-1', 'Implementation', '{}', '[]',
                         '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )?;
            conn.execute(
                "INSERT INTO workflow_runs (id, workflow_id, status, started_at)
                 VALUES ('run-1', 'workflow-1', 'Running', '2026-01-01T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        db
    }

    fn agent_step(room_id: Option<&str>) -> WorkflowStep {
        WorkflowStep {
            name: "orchestrate".into(),
            step_type: StepType::Agent,
            prompt_template: "go".into(),
            room_id: room_id.map(str::to_owned),
            ..Default::default()
        }
    }

    fn ctx_with_room(room: &str) -> TemplateContext {
        let mut ctx = TemplateContext::new();
        ctx.set("room", room);
        ctx
    }

    #[tokio::test]
    async fn only_the_running_step_of_its_run_holds_the_room_capability() {
        let db = db_with_room().await;
        let rooms = Arc::new(WorkflowStepRooms::default());
        let step = agent_step(Some("{{room}}"));
        let active = activate(&rooms, &db, "run-1", None, &step, &ctx_with_room("room-a"))
            .await
            .unwrap()
            .expect("a step with a room is activated");
        let context = active.context().clone();
        assert_eq!(context.discussion_id, "room-a");
        assert_eq!(context.step_key, "orchestrate");
        assert!(rooms.authorize(&context));
        let visible = db
            .with_read_conn(|conn| {
                crate::db::workflow_step_rooms::list_active_for_discussion(conn, "room-a")
            })
            .await
            .unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].identity.workflow_name, "Implementation");
        assert_eq!(visible[0].identity.step_name, "orchestrate");
        assert!(
            !format!("{context:?}").contains(&context.capability),
            "the capability never reaches a log line"
        );

        let forged = |edit: fn(&mut WorkflowStepBridgeContext)| {
            let mut forged = context.clone();
            edit(&mut forged);
            rooms.authorize(&forged)
        };
        assert!(!forged(|c| c.capability.push('x')), "a guessed capability");
        assert!(!forged(|c| c.run_id = "run-2".into()), "another run");
        assert!(!forged(|c| c.step_key = "review".into()), "another step");
        assert!(
            !forged(|c| c.discussion_id = "room-b".into()),
            "another room"
        );

        // The next activation of the run (a later step, a Goto, a replay)
        // invalidates the earlier capability.
        let next = activate(&rooms, &db, "run-1", None, &step, &ctx_with_room("room-a"))
            .await
            .unwrap()
            .unwrap();
        assert!(!rooms.authorize(&context));
        assert!(rooms.authorize(next.context()));
        // An outdated guard never releases its successor.
        drop(active);
        assert!(rooms.authorize(next.context()));
        let finished = next.context().clone();
        next.finish().await;
        assert!(!rooms.authorize(&finished), "a finished step holds nothing");
        assert!(!rooms.attach(&finished, 1), "nor can it attach a session");
        assert!(db
            .with_read_conn(|conn| {
                crate::db::workflow_step_rooms::list_active_for_discussion(conn, "room-a")
            })
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn a_room_that_does_not_resolve_refuses_the_launch() {
        let db = db_with_room().await;
        let rooms = Arc::new(WorkflowStepRooms::default());
        assert!(
            activate(
                &rooms,
                &db,
                "run",
                None,
                &agent_step(None),
                &ctx_with_room("room-a")
            )
            .await
            .unwrap()
            .is_none(),
            "no room, no capability"
        );
        let unknown = activate(
            &rooms,
            &db,
            "run",
            None,
            &agent_step(Some("{{room}}")),
            &ctx_with_room("room-z"),
        )
        .await;
        assert!(matches!(&unknown, Err(error) if error.contains("room-z")));
        let unrendered = activate(
            &rooms,
            &db,
            "run",
            None,
            &agent_step(Some("{{steps.missing.data}}")),
            &ctx_with_room("room-a"),
        )
        .await;
        assert!(matches!(&unrendered, Err(error) if error.starts_with("room_id:")));
        let mut http_step = agent_step(Some("{{room}}"));
        http_step.agent = AgentType::Ollama;
        let bridgeless = activate(
            &rooms,
            &db,
            "run",
            None,
            &http_step,
            &ctx_with_room("room-a"),
        )
        .await;
        assert!(matches!(&bridgeless, Err(error) if error.contains("Claude Code or Codex")));
        assert!(rooms.live().is_empty());
    }

    /// B4-01 — a rendered room outside the run's project refuses the launch
    /// and records no activity there.
    #[tokio::test]
    async fn a_rendered_room_outside_the_run_s_project_is_refused() {
        let db = db_with_room().await;
        db.with_conn(|conn| {
            let now = "2026-01-01T00:00:00Z";
            for id in ["p", "q"] {
                conn.execute(
                    "INSERT INTO projects(id, name, path, created_at, updated_at) \
                     VALUES (?1, ?1, ?1, ?2, ?2)",
                    rusqlite::params![id, now],
                )?;
            }
            conn.execute(
                "INSERT INTO discussions (id, title, project_id, created_at, updated_at) \
                 VALUES ('room-q', 'Q', 'q', ?1, ?1), ('room-p', 'P', 'p', ?1, ?1)",
                [now],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let rooms = Arc::new(WorkflowStepRooms::default());
        let step = agent_step(Some("{{room}}"));
        let refused = activate(
            &rooms,
            &db,
            "run-1",
            Some("p"),
            &step,
            &ctx_with_room("room-q"),
        )
        .await;
        assert!(
            matches!(&refused, Err(error) if error.contains("outside this run's project")),
            "{:?}",
            refused.as_ref().err()
        );
        let shared = activate(
            &rooms,
            &db,
            "run-1",
            Some("p"),
            &step,
            &ctx_with_room("room-a"),
        )
        .await;
        assert!(
            shared.is_err(),
            "a templated project-less room is not the run's"
        );
        let activities: i64 = db
            .with_read_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM workflow_step_room_activities",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(activities, 0);
        assert!(rooms.live().is_empty());
        let own = activate(
            &rooms,
            &db,
            "run-1",
            Some("p"),
            &step,
            &ctx_with_room("room-p"),
        )
        .await
        .unwrap();
        assert!(own.is_some());
        let literal = activate(
            &rooms,
            &db,
            "run-1",
            Some("p"),
            &agent_step(Some("room-a")),
            &TemplateContext::new(),
        )
        .await
        .unwrap();
        assert!(
            literal.is_some(),
            "a literal shared room is the author's choice"
        );
        let literal_foreign = activate(
            &rooms,
            &db,
            "run-1",
            Some("p"),
            &agent_step(Some("room-q")),
            &TemplateContext::new(),
        )
        .await
        .unwrap();
        assert!(
            literal_foreign.is_some(),
            "a literal room of another project is the author's choice"
        );
    }

    fn workflow_of(steps: Vec<WorkflowStep>) -> Workflow {
        Workflow {
            refs: Default::default(),
            retention: None,
            shadow: None,
            project_scope: None,
            pinned: false,
            id: "workflow-1".into(),
            name: "Implementation".into(),
            project_id: None,
            trigger: crate::models::WorkflowTrigger::Manual,
            steps,
            actions: vec![],
            safety: crate::models::WorkflowSafety {
                sandbox: false,
                max_files: None,
                max_lines: None,
                require_approval: false,
            },
            workspace_config: None,
            concurrency_limit: None,
            concurrency_key: None,
            guards: None,
            artifacts: Default::default(),
            on_failure: vec![],
            exec_allowlist: vec![],
            variables: vec![],
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn publishing_rooms() -> (
        Arc<WorkflowStepRooms>,
        tokio::sync::broadcast::Receiver<WsMessage>,
        Arc<LiveRuns>,
    ) {
        let (ws, events) = tokio::sync::broadcast::channel(64);
        let live_runs = Arc::new(LiveRuns::default());
        let rooms = Arc::new(WorkflowStepRooms::publishing(
            Arc::new(ws),
            live_runs.clone(),
        ));
        (rooms, events, live_runs)
    }

    async fn projected(
        db: &Arc<Database>,
        rooms: &Arc<WorkflowStepRooms>,
        disc_id: &str,
    ) -> Vec<ActiveWorkflowStep> {
        let (rooms, disc_id) = (rooms.clone(), disc_id.to_string());
        db.with_read_conn(move |conn| {
            let active =
                crate::db::workflow_step_rooms::list_active_for_discussion(conn, &disc_id)?;
            Ok(rooms.project(conn, &disc_id, active))
        })
        .await
        .unwrap()
    }

    /// The next `WorkflowStepRoomChanged` of `room`, skipping progress frames.
    async fn next_room_change(
        events: &mut tokio::sync::broadcast::Receiver<WsMessage>,
        room: &str,
    ) {
        let wait = async {
            loop {
                if let Ok(WsMessage::WorkflowStepRoomChanged { discussion_id }) =
                    events.recv().await
                {
                    if discussion_id == room {
                        return;
                    }
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), wait)
            .await
            .expect("the room was told");
    }

    async fn live_runs_empty(live_runs: &LiveRuns, room: &str) {
        let wait = async {
            while !live_runs.snapshot(room).is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), wait)
            .await
            .expect("the step's progress left the live runs");
    }

    fn room_step(name: &str, room: Option<&str>) -> WorkflowStep {
        WorkflowStep {
            name: name.into(),
            step_type: if room.is_some() {
                StepType::Agent
            } else {
                StepType::Exec
            },
            room_id: room.map(str::to_owned),
            ..Default::default()
        }
    }

    /// KT-1252 — the room is told at once when a step enters and when it
    /// leaves, and the step's live progress is readable meanwhile.
    #[tokio::test]
    async fn a_step_tells_its_room_and_publishes_its_live_progress() {
        use crate::agents::activity::ToolActivityUpdate;
        let db = db_with_room().await;
        let (rooms, mut events, live_runs) = publishing_rooms();
        let step = agent_step(Some("{{room}}"));
        let active = activate(&rooms, &db, "run-1", None, &step, &ctx_with_room("room-a"))
            .await
            .unwrap()
            .unwrap();
        next_room_change(&mut events, "room-a").await;

        let shown = projected(&db, &rooms, "room-a").await;
        assert_eq!(shown.len(), 1);
        assert!(!shown[0].preparing);
        let progress_run = shown[0].progress_run_id.clone().expect("a progress run");
        let frames = live_runs.snapshot("room-a");
        assert_eq!(frames.len(), 1, "a room opened now recovers it");
        match &frames[0] {
            WsMessage::AgentRunProgress {
                run_id,
                dispatch_id,
                trigger_message_id,
                progress,
                ..
            } => {
                assert_eq!(run_id, &progress_run);
                assert!(dispatch_id.is_none() && trigger_message_id.is_none());
                assert_eq!(progress.tool_calls, 0);
            }
            other => panic!("not a progress frame: {other:?}"),
        }

        active.progress().tool(&ToolActivityUpdate::of_category(
            None,
            crate::models::ActivityCategory::Read,
        ));
        match &live_runs.snapshot("room-a")[0] {
            WsMessage::AgentRunProgress { progress, .. } => {
                assert_eq!(progress.tool_calls, 1);
                assert_eq!(progress.phase, crate::models::AgentRunPhase::Tool);
            }
            other => panic!("not a progress frame: {other:?}"),
        }

        active.finish_as(AgentRunStop::Finished).await;
        next_room_change(&mut events, "room-a").await;
        assert!(projected(&db, &rooms, "room-a").await.is_empty());
        // The frame carrying the stop is the last: the run then leaves.
        live_runs_empty(&live_runs, "room-a").await;
    }

    /// KT-1252 — a step dropped before its verdict (cancellation, deadline)
    /// ends in the room too: the placeholder goes and the progress stops.
    #[tokio::test]
    async fn a_dropped_step_leaves_its_room_and_stops_its_progress() {
        let db = db_with_room().await;
        let (rooms, mut events, live_runs) = publishing_rooms();
        let step = agent_step(Some("{{room}}"));
        let active = activate(&rooms, &db, "run-1", None, &step, &ctx_with_room("room-a"))
            .await
            .unwrap()
            .unwrap();
        next_room_change(&mut events, "room-a").await;
        let progress = active.progress().clone();
        drop(active);
        next_room_change(&mut events, "room-a").await;
        assert!(projected(&db, &rooms, "room-a").await.is_empty());
        assert_eq!(
            progress.snapshot().stopped,
            Some(AgentRunStop::Failed),
            "a step dropped without a verdict did not succeed"
        );
        live_runs_empty(&live_runs, "room-a").await;
    }

    /// KT-1252 — until its Agent step enters, a run that heads for a room
    /// shows the workflow and the step it executes.
    #[tokio::test]
    async fn a_run_heading_for_a_room_shows_its_preparation_there() {
        let db = db_with_room().await;
        let (rooms, mut events, _) = publishing_rooms();
        let workflow = workflow_of(vec![
            room_step("clone", None),
            room_step("context", None),
            room_step("analyse", Some("{{room}}")),
        ]);
        let watch = RoomTargetWatch::new(&rooms, "run-1");
        let ctx = ctx_with_room("room-a");

        watch.announce(&db, None, &workflow, 0, &ctx).await;
        next_room_change(&mut events, "room-a").await;
        let shown = projected(&db, &rooms, "room-a").await;
        assert_eq!(shown.len(), 1);
        assert!(shown[0].preparing);
        assert!(shown[0].progress_run_id.is_none());
        assert_eq!(shown[0].identity.workflow_name, "Implementation");
        assert_eq!(shown[0].identity.step_name, "clone");
        assert_eq!(shown[0].identity.run_id, "run-1");

        watch.announce(&db, None, &workflow, 1, &ctx).await;
        let shown = projected(&db, &rooms, "room-a").await;
        assert_eq!(shown[0].identity.step_name, "context");

        // Its agent enters: the persisted step replaces the preparation.
        watch.announce(&db, None, &workflow, 2, &ctx).await;
        let step = workflow.steps[2].clone();
        let active = activate(&rooms, &db, "run-1", None, &step, &ctx)
            .await
            .unwrap()
            .unwrap();
        let shown = projected(&db, &rooms, "room-a").await;
        assert_eq!(shown.len(), 1);
        assert!(!shown[0].preparing);
        assert_eq!(shown[0].identity.step_name, "analyse");

        // Done: nothing is left to prepare for that room.
        active.finish().await;
        assert!(projected(&db, &rooms, "room-a").await.is_empty());
        watch.announce(&db, None, &workflow, 3, &ctx).await;
        assert!(projected(&db, &rooms, "room-a").await.is_empty());
    }

    /// KT-1252 — a step finishing late (its `Drop` spawns the write) leaves
    /// alone what the run announced for a later step in the meantime.
    #[tokio::test]
    async fn a_step_finishing_late_keeps_the_announcement_made_after_it() {
        let db = db_with_room().await;
        let (rooms, mut events, _) = publishing_rooms();
        let workflow = workflow_of(vec![
            room_step("analyse", Some("{{room}}")),
            room_step("review", Some("{{room}}")),
        ]);
        let watch = RoomTargetWatch::new(&rooms, "run-1");
        let ctx = ctx_with_room("room-a");
        watch.announce(&db, None, &workflow, 0, &ctx).await;
        let active = activate(&rooms, &db, "run-1", None, &workflow.steps[0], &ctx)
            .await
            .unwrap()
            .unwrap();
        // The step is dropped, its write is spawned and has not run yet: the
        // run announces the next step first.
        drop(active);
        rooms.set_target(
            "run-1",
            Some(RoomTarget {
                disc_id: "room-a".into(),
                workflow_id: workflow.id.clone(),
                workflow_name: workflow.name.clone(),
                step_key: "review".into(),
                step_name: "review".into(),
                agent_type: AgentType::ClaudeCode,
                started_at: chrono::Utc::now().to_rfc3339(),
            }),
        );
        while events.try_recv().is_ok() {}
        // The spawned write ends by telling the room.
        next_room_change(&mut events, "room-a").await;

        let shown = projected(&db, &rooms, "room-a").await;
        assert_eq!(shown.len(), 1);
        assert!(shown[0].preparing);
        assert_eq!(shown[0].identity.step_name, "review");
    }

    /// KT-1252 — a room not known yet, or a run that is over, is not claimed.
    #[tokio::test]
    async fn a_run_claims_no_room_it_cannot_name_or_after_it_ends() {
        let db = db_with_room().await;
        let (rooms, mut events, _) = publishing_rooms();
        let unknown = workflow_of(vec![
            room_step("clone", None),
            room_step("analyse", Some("{{steps.salle.data.room_id}}")),
        ]);
        let watch = RoomTargetWatch::new(&rooms, "run-1");
        watch
            .announce(&db, None, &unknown, 0, &TemplateContext::new())
            .await;
        assert!(projected(&db, &rooms, "room-a").await.is_empty());

        let literal = workflow_of(vec![
            room_step("clone", None),
            room_step("analyse", Some("room-a")),
        ]);
        watch
            .announce(&db, None, &literal, 0, &TemplateContext::new())
            .await;
        next_room_change(&mut events, "room-a").await;
        assert_eq!(projected(&db, &rooms, "room-a").await.len(), 1);

        // The run left Running (cancelled, failed): the room stops showing it.
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE workflow_runs SET status = 'Cancelled' WHERE id = 'run-1'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        assert!(projected(&db, &rooms, "room-a").await.is_empty());

        // Dropping the watch releases the claim and tells the room.
        drop(watch);
        next_room_change(&mut events, "room-a").await;
        assert!(rooms.targets().is_empty());
    }
}
