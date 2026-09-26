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

    fn release(&self, run_id: &str, capability_hash: &str) -> Vec<i64> {
        let mut live = self.live();
        if live
            .get(run_id)
            .is_some_and(|entry| entry.capability_hash == capability_hash)
        {
            live.remove(run_id)
                .map(|entry| entry.sessions)
                .unwrap_or_default()
        } else {
            Vec::new()
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
        let sessions = self
            .rooms
            .release(&self.context.run_id, &self.capability_hash);
        revoke(&self.db, sessions).await;
    }
}

impl Drop for StepRoomActivation {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let sessions = self
            .rooms
            .release(&self.context.run_id, &self.capability_hash);
        if sessions.is_empty() {
            return;
        }
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(revoke(self.db.clone(), sessions));
            }
            Err(_) => tracing::warn!(
                run_id = %self.context.run_id,
                "workflow step room sessions left active: no runtime to revoke them"
            ),
        }
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
    let exists = {
        let room = room.clone();
        db.with_read_conn(move |conn| {
            Ok(crate::db::discussions::get_discussion(conn, &room)?.is_some())
        })
        .await
        .map_err(|error| format!("room_id: {error}"))?
    };
    if !exists {
        return Err(format!("room_id: discussion `{room}` not found"));
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
        let active = activate(&rooms, &db, "run-1", &step, &ctx_with_room("room-a"))
            .await
            .unwrap()
            .expect("a step with a room is activated");
        let context = active.context().clone();
        assert_eq!(context.discussion_id, "room-a");
        assert_eq!(context.step_key, "orchestrate");
        assert!(rooms.authorize(&context));
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
        let next = activate(&rooms, &db, "run-1", &step, &ctx_with_room("room-a"))
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
            &agent_step(Some("{{room}}")),
            &ctx_with_room("room-z"),
        )
        .await;
        assert!(matches!(&unknown, Err(error) if error.contains("room-z")));
        let unrendered = activate(
            &rooms,
            &db,
            "run",
            &agent_step(Some("{{steps.missing.data}}")),
            &ctx_with_room("room-a"),
        )
        .await;
        assert!(matches!(&unrendered, Err(error) if error.starts_with("room_id:")));
        let mut http_step = agent_step(Some("{{room}}"));
        http_step.agent = AgentType::Ollama;
        let bridgeless = activate(&rooms, &db, "run", &http_step, &ctx_with_room("room-a")).await;
        assert!(matches!(&bridgeless, Err(error) if error.contains("Claude Code or Codex")));
        assert!(rooms.live().is_empty());
    }
}
