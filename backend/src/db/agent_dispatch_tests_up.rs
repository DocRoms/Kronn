use super::*;

use crate::db::migrations;

fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    migrations::run(&connection).unwrap();
    let now = Utc::now().to_rfc3339();
    connection
        .execute(
            "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('d1', 'Dispatch', ?1, ?1)",
            [&now],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at)
                 VALUES ('u1', 'd1', 'User', 'go', ?1, 1, ?1)",
            [&now],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE discussions SET next_message_seq = 2 WHERE id = 'd1'",
            [],
        )
        .unwrap();
    connection
}

// ── KT-1632 — the cap on local task workers ──

const CAP_TWO: &str = r#"{"__task_workers":2}"#;

/// One task worker: an execution, its sub-discussion run by `agent`, and
/// the worker's pending dispatch. `kind` is the execution's worker kind.
fn task_worker(connection: &Connection, name: &str, agent: &str, kind: &str) -> String {
    let now = Utc::now().to_rfc3339();
    let room = format!("room-{name}");
    let job = format!("job-{name}");
    connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .unwrap();
    connection
        .execute(
            "INSERT INTO discussions (id, title, agent, created_at, updated_at)
                 VALUES (?1, 'worker room', ?2, ?3, ?3)",
            params![room, agent, now],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at)
                 VALUES (?1, ?2, 'User', 'brief', ?3, 1, ?3)",
            params![format!("brief-{name}"), room, now],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO task_executions
                 (id, orchestration_run_id, task_id, parent_discussion_id,
                  sub_discussion_id, worker_target_kind, worker_agent_type,
                  created_at, updated_at)
                 VALUES (?1, 'run', ?1, 'd1', ?2, ?3, ?4, ?5, ?5)",
            params![format!("exec-{name}"), room, kind, agent, now],
        )
        .unwrap();
    enqueue(
        connection,
        NewAgentDispatchJob {
            id: &job,
            discussion_id: &room,
            trigger_message_id: &format!("brief-{name}"),
            trigger_sort_order: 1,
            dedupe_key: &format!("orch-dispatch:{name}:1"),
            agent_override: None,
            chain_prompt_ids: &[],
            batch_item: None,
            group_id: None,
            group_concurrency_limit: None,
        },
    )
    .unwrap();
    // Launch order is (created_at, rowid): jobs enqueued in turn keep it
    // even inside one clock tick, so no pause is needed here.
    connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    job
}

fn claim_capped(connection: &Connection, id: &str) -> bool {
    claim_with_limits(connection, id, Some(CAP_TWO))
        .unwrap()
        .is_some()
}

/// A quota escalation settles the worker's dispatch as failed and parks the
/// execution for a human: its slot goes to the next in line.
#[test]
fn a_quota_escalation_frees_the_slot_of_its_worker() {
    let connection = connection();
    let a = task_worker(&connection, "a", "ClaudeCode", "agent");
    let b = task_worker(&connection, "b", "ClaudeCode", "agent");
    let queued = task_worker(&connection, "c", "ClaudeCode", "agent");
    assert!(claim_capped(&connection, &a));
    assert!(claim_capped(&connection, &b));
    assert!(!claim_capped(&connection, &queued));

    crate::db::orchestration::attach_execution_dispatch(&connection, "exec-a", &a).unwrap();
    assert!(mark_failed(&connection, &a, "quota exhausted").unwrap());
    crate::db::orchestration::escalate_execution_for_dispatch_quota(
        &connection,
        &a,
        "ClaudeCode",
        None,
    )
    .unwrap();

    assert_eq!(running_task_workers(&connection).unwrap(), 1);
    assert!(claim_capped(&connection, &queued), "the slot is handed on");
}
