//! A workflow Agent step with a `room_id` (KT-793): the runner mints a
//! capability for the step's run, hands it to the agent's bridge through the
//! environment, and the bridge exchanges it for a membership of that room.
//!
//! The capability lives only in this process, so it is valid exactly while
//! the step runs here: a finished step, another step, another run or a
//! restarted backend holds nothing the join endpoint accepts.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::agents::runner::WorkflowStepBridgeContext;
use crate::db::discussion_sessions::sha256_hex;
use crate::db::Database;
use crate::models::{AgentType, RunStatus, StepResult, WorkflowStep};
use crate::workflows::steps::StepOutcome;
use crate::workflows::template::TemplateContext;

struct LiveStep {
    step_key: String,
    disc_id: String,
    capability_hash: String,
    sessions: Vec<i64>,
}

/// Steps currently running with a room, keyed by run id.
#[derive(Default)]
pub struct WorkflowStepRooms {
    live: Mutex<HashMap<String, LiveStep>>,
}

impl WorkflowStepRooms {
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
}

impl StepRoomActivation {
    pub fn context(&self) -> &WorkflowStepBridgeContext {
        &self.context
    }

    /// End the membership now and wait for it, rather than from `Drop`.
    pub async fn finish(mut self) {
        self.released = true;
        let released = self
            .rooms
            .release(&self.context.run_id, &self.capability_hash);
        if let Some(sessions) = released {
            finish_activity(&self.db, self.context.clone(), sessions).await;
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
                handle.spawn(finish_activity(self.db.clone(), context, sessions));
            }
            Err(_) => tracing::warn!(
                run_id = %self.context.run_id,
                "workflow step room activity left active: no runtime to finish it"
            ),
        }
    }
}

async fn finish_activity(
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
    let replaced = rooms.insert(
        run_id,
        LiveStep {
            step_key: context.step_key.clone(),
            disc_id: room,
            capability_hash: capability_hash.clone(),
            sessions: Vec::new(),
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
    Ok(Some(StepRoomActivation {
        rooms: rooms.clone(),
        db: db.clone(),
        context,
        capability_hash,
        released: false,
    }))
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

#[cfg(test)]
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
}
