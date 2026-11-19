//! KT-317 — persistence + state-machine tests (DoD-6): constraints, forbidden
//! transitions, idempotence, concurrency, saga-checkpoint resume, lineage. No
//! Git integration (that is KT-320).

use super::*;
use rusqlite::{params, Connection};

use crate::models::{
    saga_resume_action, AgentType, BlockedReasonCode, CampaignWorkerSelection,
    CancellationCleanupPolicy, ExecutionRecoveryAction, ExecutionTimeoutKind,
    LaunchSingleTaskInput, MessageTarget, MessageTargetKind, ModelTier, OrchestrationActor,
    OrchestrationControlState, OrchestrationResiliencePolicy, OrchestrationRunInput,
    OrchestrationRunKind, PlanningActorKind, SagaResumeAction, TaskExecutionStatus,
    TaskWorkerScope, ValidationSpec,
};

const DISC: &str = "disc-parent";

fn setup() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, name, path, created_at, updated_at) \
         VALUES ('p1', 'P', '/p', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO discussions (id, title, created_at, updated_at) \
         VALUES (?1, 'D', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        params![DISC],
    )
    .unwrap();
    conn
}

fn seed_task(conn: &Connection, task_id: &str, number: i64) {
    conn.execute(
        "INSERT INTO planning_tasks (id, task_number, title, created_at, updated_at) \
         VALUES (?1, ?2, 'T', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        params![task_id, number],
    )
    .unwrap();
}

fn backend_actor() -> OrchestrationActor {
    OrchestrationActor {
        kind: PlanningActorKind::Backend,
        id: Some("orchestrator".into()),
        session_id: None,
        source_message_id: None,
        token_declared: false,
    }
}

/// Sets a process-wide timeout variable for one test and puts it back on drop
/// (also on panic). Only used under `serial(task_timeout_env)`.
struct TimeoutEnv(&'static str);

impl TimeoutEnv {
    fn set(name: &'static str, value: &str) -> Self {
        crate::core::child_env::set_var(name, value);
        TimeoutEnv(name)
    }
}

impl Drop for TimeoutEnv {
    fn drop(&mut self) {
        crate::core::child_env::remove_var(self.0);
    }
}

// ── KT-1433: Docker elevation card ───────────────────────────────────────

fn elevation_fixture(conn: &Connection) -> (String, String) {
    seed_task(conn, "task-1433", 1433);
    let mut input = LaunchSingleTaskInput::new("task-1433", DISC);
    input.project_id = Some("p1".into());
    let exec = launch_single_task(conn, &input, &backend_actor())
        .unwrap()
        .execution
        .id;
    post_docker_elevation_card(conn, &exec).unwrap();
    let card: String = conn
        .query_row(
            "SELECT id FROM messages WHERE id LIKE 'orch-elevation:docker:%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    (exec, card)
}

#[test]
fn kronn_posts_a_docker_elevation_card_with_three_scopes() {
    let conn = setup();
    let (_exec, card) = elevation_fixture(&conn);
    let (key, payload): (String, String) = conn
        .query_row(
            "SELECT question_key, payload_json FROM discussion_questions WHERE source_message_id = ?1",
            [&card],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(key, "kronn-elevation-docker");
    let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    let ids: Vec<&str> = payload["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["deny", "discussion", "project"]);
    let kind: String = conn
        .query_row("SELECT kind FROM messages WHERE id = ?1", [&card], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(kind, "orchestration");
}

#[test]
fn a_project_elevation_opens_docker_to_the_project_and_never_narrows_full_access() {
    let conn = setup();
    let (exec, card) = elevation_fixture(&conn);
    assert_eq!(
        record_docker_elevation(&conn, &card, "project", "Romu").unwrap(),
        Some(exec.clone())
    );
    assert_eq!(
        crate::db::projects::validations_access(&conn, "p1").unwrap(),
        crate::models::ValidationsAccess::Docker
    );
    crate::db::projects::set_validations_access(
        &conn,
        "p1",
        crate::models::ValidationsAccess::Full,
    )
    .unwrap();
    record_docker_elevation(&conn, &card, "project", "Romu").unwrap();
    assert_eq!(
        crate::db::projects::validations_access(&conn, "p1").unwrap(),
        crate::models::ValidationsAccess::Full
    );
}

#[test]
fn a_discussion_elevation_grants_only_that_discussion() {
    let conn = setup();
    let (exec, card) = elevation_fixture(&conn);
    assert_eq!(
        record_docker_elevation(&conn, &card, "discussion", "Romu").unwrap(),
        Some(exec)
    );
    assert!(
        crate::db::projects::discussion_capability_granted(&conn, DISC, "validations_docker")
            .unwrap()
    );
    assert_eq!(
        crate::db::projects::validations_access(&conn, "p1").unwrap(),
        crate::models::ValidationsAccess::Sandbox,
        "the project is untouched"
    );
}

#[test]
fn a_refusal_or_a_look_alike_card_grants_nothing() {
    let conn = setup();
    let (exec, card) = elevation_fixture(&conn);
    assert_eq!(
        record_docker_elevation(&conn, &card, "deny", "Romu").unwrap(),
        None
    );
    // A message with the prefix but without the orchestration kind is not
    // Kronn's request.
    conn.execute("UPDATE messages SET kind = NULL WHERE id = ?1", [&card])
        .unwrap();
    assert_eq!(
        record_docker_elevation(&conn, &card, "project", "Romu").unwrap(),
        None
    );
    assert_eq!(
        record_docker_elevation(&conn, &format!("question:{exec}:0"), "project", "Romu").unwrap(),
        None
    );
    assert!(
        !crate::db::projects::discussion_capability_granted(&conn, DISC, "validations_docker")
            .unwrap()
    );
    assert_eq!(
        crate::db::projects::validations_access(&conn, "p1").unwrap(),
        crate::models::ValidationsAccess::Sandbox
    );
}
