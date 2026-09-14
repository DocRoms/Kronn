use super::*;
use crate::models::{LaunchSingleTaskInput, TaskExecutionStatus};

const ORIGIN: &str = "binding-parent";
const CHILD: &str = "binding-child";
const THIRD: &str = "binding-third";
const AGENT: &str = "ClaudeCode";

fn actor() -> OrchestrationActor {
    OrchestrationActor {
        kind: PlanningActorKind::Backend,
        id: Some("test".into()),
        session_id: None,
        source_message_id: None,
    }
}

fn seed(conn: &Connection) -> String {
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    crate::db::migrations::run(conn).unwrap();
    for room in [ORIGIN, CHILD, THIRD] {
        conn.execute(
            "INSERT INTO discussions(id, title, created_at, updated_at) \
             VALUES (?1, 'Test', '2026-09-07', '2026-09-07')",
            [room],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO discussion_sessions \
         (id, disc_id, agent_type, session_id, role, status, joined_at) \
         VALUES (101, ?1, ?2, 'adhoc-worker', 'peer', 'active', '2026-09-07')",
        params![CHILD, AGENT],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO planning_tasks(id, task_number, title, created_at, updated_at) \
         VALUES ('t1', 1, 'Test', '2026-09-07', '2026-09-07')",
        [],
    )
    .unwrap();
    let exec = crate::db::orchestration::launch_single_task(
        conn,
        &LaunchSingleTaskInput::new("t1", ORIGIN),
        &actor(),
    )
    .unwrap()
    .execution
    .id;
    conn.execute(
        "UPDATE task_executions SET sub_discussion_id = ?2, worker_target_kind = 'cli', \
         worker_cli_session_id = 101, worker_agent_type = ?3, status = 'Working' WHERE id = ?1",
        params![exec, CHILD, AGENT],
    )
    .unwrap();
    crate::db::disc_source::bind_to_source(conn, CHILD, AGENT, "cli-worker").unwrap();
    exec
}

fn setup(pinned: bool) -> (Connection, String) {
    let conn = Connection::open_in_memory().unwrap();
    let exec = seed(&conn);
    if pinned {
        pin(&conn, &exec, 101, AGENT, "cli-worker").unwrap();
    }
    (conn, exec)
}

fn binding(conn: &Connection, key: &str) -> Option<String> {
    crate::db::disc_source::find_disc_by_source_session(conn, AGENT, key).unwrap()
}

fn membership(conn: &Connection) -> String {
    conn.query_row(
        "SELECT disc_id FROM discussion_sessions WHERE id = 101",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

fn cancel(conn: &Connection, exec: &str) -> Result<bool> {
    crate::db::orchestration::transition_execution(
        conn,
        exec,
        TaskExecutionStatus::Cancelled,
        &actor(),
        serde_json::json!({}),
    )
}

#[test]
fn pin_is_immutable_idempotent_and_audited_once() {
    let (conn, exec) = setup(true);
    pin(&conn, &exec, 101, AGENT, "cli-worker").unwrap();
    assert!(pin(&conn, &exec, 101, AGENT, "cli-other").is_err());
    assert!(pin(&conn, &exec, 101, "Codex", "cli-worker").is_err());
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_execution_events WHERE task_execution_id = ?1 \
         AND action = 'cli_worker_binding_pinned'",
            [&exec],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert!(cancel(&conn, &exec).unwrap());
    assert_eq!(binding(&conn, "cli-worker").as_deref(), Some(ORIGIN));
}

#[test]
fn pin_validates_empty_values_and_preserves_unicode_identity() {
    let (conn, exec) = setup(false);
    for (agent, key) in [("", "cli-key"), (AGENT, " \n\t")] {
        assert!(pin(&conn, &exec, 101, agent, key).is_err());
    }
    pin(&conn, &exec, 101, AGENT, "cli-é🦀").unwrap();
    crate::db::disc_source::bind_to_source(&conn, CHILD, AGENT, "cli-é🦀").unwrap();
    assert!(cancel(&conn, &exec).unwrap());
    assert_eq!(binding(&conn, "cli-é🦀").as_deref(), Some(ORIGIN));
    assert_eq!(binding(&conn, "cli-worker").as_deref(), Some(CHILD));
}

#[test]
fn pin_participates_in_acceptance_transaction_rollback() {
    let (conn, exec) = setup(false);
    {
        let tx = conn.unchecked_transaction().unwrap();
        pin(&tx, &exec, 101, AGENT, "cli-worker").unwrap();
    }
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_execution_cli_bindings",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_execution_events WHERE action = 'cli_worker_binding_pinned'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn return_survives_database_reopen_and_active_key_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("binding.db");
    let exec = {
        let conn = Connection::open(&path).unwrap();
        let exec = seed(&conn);
        pin(&conn, &exec, 101, AGENT, "cli-worker").unwrap();
        exec
    };
    let conn = Connection::open(path).unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn.execute(
        "UPDATE discussion_sessions SET session_id = 'adhoc-new' WHERE id = 101",
        [],
    )
    .unwrap();
    crate::db::disc_source::bind_to_source(&conn, CHILD, AGENT, "cli-other").unwrap();
    assert!(cancel(&conn, &exec).unwrap());
    assert!(
        cancel(&conn, &exec).is_err(),
        "terminal state-machine transitions stay illegal"
    );
    return_to_origin(&conn, &exec, 101, Some(AGENT), ORIGIN, CHILD).unwrap();
    assert_eq!(binding(&conn, "cli-worker").as_deref(), Some(ORIGIN));
    assert_eq!(binding(&conn, "cli-other").as_deref(), Some(CHILD));
    assert_eq!(membership(&conn), ORIGIN);
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE id = ?1",
            [format!("orch-return-origin:{exec}:Cancelled")],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn foreign_binding_or_membership_rolls_back_terminal_transition() {
    for move_binding in [true, false] {
        let (conn, exec) = setup(true);
        if move_binding {
            crate::db::disc_source::bind_to_source(&conn, THIRD, AGENT, "cli-worker").unwrap();
        } else {
            crate::db::discussion_sessions::move_session_to_discussion(&conn, 101, THIRD).unwrap();
        }
        assert!(cancel(&conn, &exec)
            .unwrap_err()
            .to_string()
            .contains("moved"));
        assert_eq!(
            crate::db::orchestration::get_task_execution(&conn, &exec)
                .unwrap()
                .unwrap()
                .status,
            TaskExecutionStatus::Working
        );
        assert_eq!(
            binding(&conn, "cli-worker").as_deref(),
            Some(if move_binding { THIRD } else { CHILD })
        );
        assert_eq!(membership(&conn), if move_binding { CHILD } else { THIRD });
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE id LIKE 'orch-return-%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}

#[test]
fn closed_binding_is_not_reopened_and_has_explicit_audit() {
    let (conn, exec) = setup(true);
    crate::db::disc_source::unbind_from_source(&conn, CHILD, Some((AGENT, "cli-worker"))).unwrap();
    assert!(cancel(&conn, &exec).unwrap());
    assert_eq!(binding(&conn, "cli-worker"), None);
    assert_eq!(membership(&conn), ORIGIN);
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_execution_events WHERE task_execution_id = ?1 \
         AND action = 'cli_worker_return_binding_missing'",
            [&exec],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn departed_active_row_does_not_erase_the_known_return_identity() {
    let (conn, exec) = setup(true);
    conn.execute(
        "UPDATE discussion_sessions SET status = 'left' WHERE id = 101",
        [],
    )
    .unwrap();
    assert!(cancel(&conn, &exec).unwrap());
    assert_eq!(binding(&conn, "cli-worker").as_deref(), Some(ORIGIN));
}

#[test]
fn legacy_exact_key_is_supported_but_ambiguous_child_is_never_guessed() {
    let (conn, exec) = setup(false);
    conn.execute(
        "UPDATE discussion_sessions SET session_id = 'cli-worker' WHERE id = 101",
        [],
    )
    .unwrap();
    assert!(cancel(&conn, &exec).unwrap());
    assert_eq!(binding(&conn, "cli-worker").as_deref(), Some(ORIGIN));

    for other in [false, true] {
        let (conn, exec) = setup(false);
        if other {
            crate::db::disc_source::bind_to_source(&conn, CHILD, AGENT, "cli-other").unwrap();
        }
        let error = cancel(&conn, &exec).unwrap_err();
        assert!(
            error.to_string().contains("no pinned durable identity"),
            "{error}"
        );
        assert_eq!(membership(&conn), CHILD);
        assert_eq!(binding(&conn, "cli-worker").as_deref(), Some(CHILD));
    }
}

#[test]
fn changed_agent_cannot_return_an_unrelated_session() {
    let (conn, exec) = setup(true);
    conn.execute(
        "UPDATE discussion_sessions SET agent_type = 'Codex' WHERE id = 101",
        [],
    )
    .unwrap();
    assert!(cancel(&conn, &exec)
        .unwrap_err()
        .to_string()
        .contains("agent changed"));
    assert_eq!(membership(&conn), CHILD);
    assert_eq!(binding(&conn, "cli-worker").as_deref(), Some(CHILD));
}

#[test]
fn authenticated_return_resume_rotates_exact_worker_back_in_origin() {
    let (conn, exec) = setup(true);
    let token = "kr-resume-11111111111111111111111111111111";
    conn.execute(
        "UPDATE discussion_sessions SET resume_token_hash = ?1 WHERE id = 101",
        [crate::db::discussion_sessions::sha256_hex(token)],
    )
    .unwrap();
    assert!(cancel(&conn, &exec).unwrap());

    let next = "kr-resume-22222222222222222222222222222222";
    let resumed = super::resume_after_orchestrator_return(
        &conn, AGENT, token, "adhoc-reloaded", Some(next), CHILD,
    )
    .unwrap();
    assert_eq!(resumed.disc_id, ORIGIN);
    assert_eq!(resumed.session_pk, 101);
    assert_eq!(resumed.resume_token, next);
    assert_eq!(membership(&conn), ORIGIN);

    let replay = super::resume_after_orchestrator_return(
        &conn, AGENT, token, "adhoc-response-loss", Some(next), CHILD,
    )
    .unwrap();
    assert_eq!(replay.session_pk, 101);
    assert_eq!(replay.resume_token, next);
}

#[test]
fn authenticated_return_resume_refuses_unproven_or_moved_state_without_mutation() {
    for case in ["active", "third-room", "closed-source", "wrong-child"] {
        let (conn, exec) = setup(true);
        let token = "kr-resume-33333333333333333333333333333333";
        let hash = crate::db::discussion_sessions::sha256_hex(token);
        conn.execute(
            "UPDATE discussion_sessions SET resume_token_hash = ?1 WHERE id = 101",
            [&hash],
        )
        .unwrap();
        if case != "active" {
            assert!(cancel(&conn, &exec).unwrap());
        }
        if case == "third-room" {
            crate::db::discussion_sessions::move_session_to_discussion(&conn, 101, THIRD).unwrap();
        } else if case == "closed-source" {
            crate::db::disc_source::unbind_from_source(&conn, ORIGIN, Some((AGENT, "cli-worker")))
                .unwrap();
        }
        let expected_child = if case == "wrong-child" { THIRD } else { CHILD };
        assert!(super::resume_after_orchestrator_return(
            &conn,
            AGENT,
            token,
            "must-not-win",
            Some("kr-resume-44444444444444444444444444444444"),
            expected_child,
        )
        .is_err());
        let hash_after: String = conn
            .query_row(
                "SELECT resume_token_hash FROM discussion_sessions WHERE id = 101",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hash_after, hash, "{case} rotated the credential");
        let active_id: String = conn
            .query_row(
                "SELECT session_id FROM discussion_sessions WHERE id = 101",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active_id, "adhoc-worker", "{case} mutated membership");
    }
}
