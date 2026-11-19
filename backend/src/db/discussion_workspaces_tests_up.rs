use super::*;

use crate::db::migrations;

use crate::db::orchestration::launch_single_task;

use crate::models::{LaunchSingleTaskInput, OrchestrationActor, PlanningActorKind};

fn fixture() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    migrations::run(&conn).unwrap();
    conn.execute(
        "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('p1', 'Kronn', '/repo', 'now', 'now')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discussions (
                 id, project_id, title, created_at, updated_at, workspace_mode
             ) VALUES ('d1', 'p1', 'Room', 'now', 'now', 'Direct')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discussion_sessions (
                 id, disc_id, agent_type, session_id, role, status, joined_at
             ) VALUES (7, 'd1', 'Codex', 'sess-1', 'peer', 'active', 'now')",
        [],
    )
    .unwrap();
    conn
}

#[test]
fn external_session_workspace_is_updated_in_place() {
    let conn = fixture();
    let first = upsert_external(
        &conn,
        "d1",
        7,
        None,
        "p1",
        "/repo-wt",
        "/repo-wt",
        "feature/one",
        "abc",
    )
    .unwrap();
    let second = upsert_external(
        &conn,
        "d1",
        7,
        None,
        "p1",
        "/repo-wt",
        "/repo-wt",
        "feature/two",
        "def",
    )
    .unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(second.branch, "feature/two");
    assert_eq!(list_for_discussion(&conn, "d1").unwrap().len(), 1);
}

/// KT-1748 — the registry vouches for managed workspaces and workflow runs,
/// never for a workspace a user merely declared, nor for an unknown path.
#[test]
fn registry_vouches_for_managed_and_workflow_checkouts_only() {
    let conn = fixture();
    let path = std::path::Path::new;
    upsert_external(
        &conn,
        "d1",
        7,
        None,
        "p1",
        "/elsewhere/declared",
        "/elsewhere/declared",
        "feature/one",
        "abc",
    )
    .unwrap();
    assert!(!is_registered_managed_checkout(
        &conn,
        path("/elsewhere/declared")
    ));
    assert!(!is_registered_managed_checkout(&conn, path("/unknown")));

    let exec = seed_execution(&conn, "task-reg", 3);
    upsert_managed(
        &conn,
        &exec,
        "d1",
        "d-parent",
        Some("task-reg"),
        "p1",
        "/repo/.kronn/worktrees/lane",
        "/repo/.kronn/worktrees/lane",
        "kronn/task/KT-3-lane",
        "head",
        "base",
    )
    .unwrap();
    assert!(is_registered_managed_checkout(
        &conn,
        path("/repo/.kronn/worktrees/lane")
    ));

    conn.execute(
        "INSERT INTO workflows (id, name, project_id, trigger_json, steps_json, \
             actions_json, safety_json, workspace_config_json, concurrency_limit, enabled, \
             created_at, updated_at) VALUES ('w1', 'wf', NULL, '{}', '[]', '[]', '{}', NULL, \
             NULL, 1, 'now', 'now')",
        [],
    )
    .ok();
    conn.execute(
        "INSERT INTO workflow_runs (id, workflow_id, workspace_path, started_at) \
             VALUES ('r1', 'w1', '/wf/run/worktree', 'now')",
        [],
    )
    .unwrap();
    assert!(is_registered_managed_checkout(
        &conn,
        path("/wf/run/worktree")
    ));
}

/// KT-1748 — a registered worktree is busy while an execution or a workflow
/// run uses it, idle once they finish, and the integration's own execution
/// never counts against itself.
#[test]
fn active_holder_names_the_run_using_a_registered_worktree() {
    let conn = fixture();
    let lane = std::path::Path::new("/repo/.kronn/worktrees/lane");
    let exec = seed_execution(&conn, "task-busy", 4);
    upsert_managed(
        &conn,
        &exec,
        "d1",
        "d-parent",
        Some("task-busy"),
        "p1",
        "/repo/.kronn/worktrees/lane",
        "/repo/.kronn/worktrees/lane",
        "feat/EW-7704",
        "head",
        "base",
    )
    .unwrap();

    let holder = active_holder_of_checkout(&conn, lane, None, &[]).unwrap();
    assert!(holder.contains(&exec), "names the execution: {holder}");
    assert_eq!(
        active_holder_of_checkout(&conn, lane, Some(&exec), &[]),
        None
    );
    assert_eq!(
        active_holder_of_checkout(&conn, std::path::Path::new("/unknown"), None, &[]),
        None
    );

    conn.execute(
        "UPDATE task_executions SET status = 'Done' WHERE id = ?1",
        params![exec],
    )
    .unwrap();
    assert_eq!(active_holder_of_checkout(&conn, lane, None, &[]), None);

    conn.execute(
        "INSERT INTO workflows (id, name, project_id, trigger_json, steps_json, \
             actions_json, safety_json, workspace_config_json, concurrency_limit, enabled, \
             created_at, updated_at) VALUES ('w1', 'wf', NULL, '{}', '[]', '[]', '{}', NULL, \
             NULL, 1, 'now', 'now')",
        [],
    )
    .ok();
    conn.execute(
        "INSERT INTO workflow_runs (id, workflow_id, workspace_path, started_at, status) \
             VALUES ('r-live', 'w1', '/auto/wt', 'now', 'Running')",
        [],
    )
    .unwrap();
    let auto = std::path::Path::new("/auto/wt");
    let holder = active_holder_of_checkout(&conn, auto, None, &[]).unwrap();
    assert!(holder.contains("r-live"), "names the run: {holder}");
    conn.execute(
        "UPDATE workflow_runs SET status = 'Success' WHERE id = 'r-live'",
        [],
    )
    .unwrap();
    assert_eq!(active_holder_of_checkout(&conn, auto, None, &[]), None);
}

/// KT-1795 — a run the caller names as waiting on it does not hold its
/// worktree; any other live run on that path, or elsewhere, still does.
#[test]
fn a_run_waiting_on_the_integration_does_not_hold_its_worktree() {
    let conn = fixture();
    let exec = seed_execution(&conn, "task-delegated", 5);
    conn.execute(
        "INSERT INTO workflows (id, name, project_id, trigger_json, steps_json, \
             actions_json, safety_json, workspace_config_json, concurrency_limit, enabled, \
             created_at, updated_at) VALUES ('w1', 'wf', NULL, '{}', '[]', '[]', '{}', NULL, \
             NULL, 1, 'now', 'now')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO workflow_runs (id, workflow_id, workspace_path, started_at, status) \
             VALUES ('r-own', 'w1', '/wf/own', 'now', 'Running'), \
                    ('r-other', 'w1', '/wf/other', 'now', 'Running')",
        [],
    )
    .unwrap();
    let own = std::path::Path::new("/wf/own");
    let other = std::path::Path::new("/wf/other");
    let waiting = vec!["r-own".to_string()];
    assert!(active_holder_of_checkout(&conn, own, Some(&exec), &[]).is_some());
    assert_eq!(
        active_holder_of_checkout(&conn, own, Some(&exec), &waiting),
        None
    );
    let holder = active_holder_of_checkout(&conn, other, Some(&exec), &waiting).unwrap();
    assert!(
        holder.contains("r-other"),
        "another path still held: {holder}"
    );

    conn.execute(
        "INSERT INTO workflow_runs (id, workflow_id, workspace_path, started_at, status) \
             VALUES ('r-twin', 'w1', '/wf/own', 'now', 'Running')",
        [],
    )
    .unwrap();
    let holder = active_holder_of_checkout(&conn, own, Some(&exec), &waiting).unwrap();
    assert!(
        holder.contains("r-twin"),
        "a second run on the same path holds it: {holder}"
    );
}

#[test]
fn canonical_path_cannot_belong_to_two_discussions() {
    let conn = fixture();
    conn.execute(
        "INSERT INTO discussions (
                 id, project_id, title, created_at, updated_at, workspace_mode
             ) VALUES ('d2', 'p1', 'Other', 'now', 'now', 'Direct')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discussion_sessions (
                 id, disc_id, agent_type, session_id, role, status, joined_at
             ) VALUES (8, 'd2', 'ClaudeCode', 'sess-2', 'peer', 'active', 'now')",
        [],
    )
    .unwrap();
    upsert_external(
        &conn, "d1", 7, None, "p1", "/repo-wt", "/repo-wt", "a", "abc",
    )
    .unwrap();

    let error = upsert_external(
        &conn, "d2", 8, None, "p1", "/repo-wt", "/repo-wt", "a", "abc",
    )
    .unwrap_err();
    assert!(error.to_string().contains("another discussion"));
}

#[test]
fn missing_external_workspace_keeps_its_path_and_ownership() {
    let conn = fixture();
    let declared = upsert_external(
        &conn,
        "d1",
        7,
        None,
        "p1",
        "/repo-wt",
        "/repo-wt",
        "feature/missing",
        "abc",
    )
    .unwrap();

    let missing = mark_missing(&conn, "d1", 7).unwrap().unwrap();

    assert_eq!(missing.id, declared.id);
    assert_eq!(missing.state, "missing");
    assert_eq!(missing.ownership, "external");
    assert_eq!(missing.workspace_path.as_deref(), Some("/repo-wt"));
}

#[test]
fn same_room_sessions_share_a_workspace_but_another_room_is_rejected() {
    let conn = fixture();
    conn.execute(
        "INSERT INTO discussion_sessions (
                 id, disc_id, agent_type, session_id, role, status, joined_at
             ) VALUES (8, 'd1', 'ClaudeCode', 'sess-2', 'peer', 'active', 'now')",
        [],
    )
    .unwrap();
    upsert_external(
        &conn, "d1", 7, None, "p1", "/repo-wt", "/repo-wt", "shared", "abc",
    )
    .unwrap();
    upsert_external(
        &conn, "d1", 8, None, "p1", "/repo-wt", "/repo-wt", "shared", "abc",
    )
    .expect("the same discussion may intentionally share one checkout");
    assert_eq!(list_for_discussion(&conn, "d1").unwrap().len(), 2);

    conn.execute(
        "INSERT INTO discussions (
                 id, project_id, title, created_at, updated_at, workspace_mode
             ) VALUES ('d2', 'p1', 'Other', 'now', 'now', 'Direct')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discussion_sessions (
                 id, disc_id, agent_type, session_id, role, status, joined_at
             ) VALUES (9, 'd2', 'Gemini', 'sess-3', 'peer', 'active', 'now')",
        [],
    )
    .unwrap();
    let error = upsert_external(
        &conn, "d2", 9, None, "p1", "/repo-wt", "/repo-wt", "shared", "abc",
    )
    .unwrap_err();
    assert!(error.to_string().contains("another discussion"));
}

#[test]
fn history_rewrite_lease_is_exclusive_renewable_and_releasable() {
    let conn = fixture();
    conn.execute(
        "INSERT INTO discussion_sessions (
                 id, disc_id, agent_type, session_id, role, status, joined_at
             ) VALUES (8, 'd1', 'ClaudeCode', 'sess-2', 'peer', 'active', 'now')",
        [],
    )
    .unwrap();
    for session_pk in [7, 8] {
        upsert_external(
            &conn,
            "d1",
            session_pk,
            None,
            "p1",
            "/repo-wt",
            "/repo-wt",
            "release/0.9.7",
            "abc",
        )
        .unwrap();
    }

    let first = acquire_history_lease(
        &conn,
        "d1",
        7,
        "refs/kronn-backup/release-0.9.7-before-squash",
    )
    .unwrap();
    let HistoryLeaseAcquire::Acquired(first) = first else {
        panic!("first holder must acquire")
    };
    let renewed = acquire_history_lease(
        &conn,
        "d1",
        7,
        "refs/kronn-backup/release-0.9.7-before-squash",
    )
    .unwrap();
    let HistoryLeaseAcquire::Acquired(renewed) = renewed else {
        panic!("same holder must renew idempotently")
    };
    assert_eq!(renewed.id, first.id);

    let blocked = acquire_history_lease(
        &conn,
        "d1",
        8,
        "refs/kronn-backup/release-0.9.7-before-squash-2",
    )
    .unwrap();
    let HistoryLeaseAcquire::Blocked(owner) = blocked else {
        panic!("the second session must be refused")
    };
    assert_eq!(owner.session_pk, 7);
    assert_eq!(owner.session_agent_type, "Codex");

    assert!(!release_history_lease(&conn, "d1", 8).unwrap());
    assert!(release_history_lease(&conn, "d1", 7).unwrap());
    assert!(matches!(
        acquire_history_lease(
            &conn,
            "d1",
            8,
            "refs/kronn-backup/release-0.9.7-before-squash-2"
        )
        .unwrap(),
        HistoryLeaseAcquire::Acquired(_)
    ));
}

#[test]
fn expired_history_lease_never_blocks_the_next_session() {
    let conn = fixture();
    conn.execute(
        "INSERT INTO discussion_sessions (
                 id, disc_id, agent_type, session_id, role, status, joined_at
             ) VALUES (8, 'd1', 'ClaudeCode', 'sess-2', 'peer', 'active', 'now')",
        [],
    )
    .unwrap();
    for session_pk in [7, 8] {
        upsert_external(
            &conn, "d1", session_pk, None, "p1", "/repo-wt", "/repo-wt", "shared", "abc",
        )
        .unwrap();
    }
    acquire_history_lease(&conn, "d1", 7, "refs/kronn-backup/first").unwrap();
    conn.execute(
        "UPDATE discussion_workspace_history_leases SET expires_at = '2000-01-01T00:00:00Z'",
        [],
    )
    .unwrap();
    assert!(matches!(
        acquire_history_lease(&conn, "d1", 8, "refs/kronn-backup/second").unwrap(),
        HistoryLeaseAcquire::Acquired(_)
    ));
}

// ─── KT-318 socle: backend-owned managed workspaces ──────────────────────

/// Seed the parent discussion + a planning task, then launch a REAL execution
/// in it (Pending) so the managed-workspace FK `task_execution_id →
/// task_executions(id)` is satisfied. Returns the real execution id — the
/// managed writer is exercised against a genuine execution row, not a literal.
fn seed_execution(conn: &Connection, task_id: &str, number: i64) -> String {
    conn.execute(
        "INSERT INTO discussions (id, project_id, title, created_at, updated_at, workspace_mode) \
             VALUES ('d-parent', 'p1', 'Principal', 'now', 'now', 'Direct')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO planning_tasks (id, task_number, title, created_at, updated_at) \
             VALUES (?1, ?2, 'T', 'now', 'now')",
        params![task_id, number],
    )
    .unwrap();
    launch_single_task(
        conn,
        &LaunchSingleTaskInput::new(task_id, "d-parent"),
        &OrchestrationActor {
            kind: PlanningActorKind::Backend,
            id: Some("orchestrator".into()),
            session_id: None,
            source_message_id: None,
            token_declared: false,
        },
    )
    .unwrap()
    .execution
    .id
}

#[test]
fn managed_workspace_is_backend_owned_and_idempotent_by_execution() {
    let conn = fixture();
    let exec = seed_execution(&conn, "task-1", 1);

    // No joined CLI session at all — the backend provisioner owns the row.
    let ws = upsert_managed(
        &conn,
        &exec,
        "d1",
        "d-parent",
        Some("task-1"),
        "p1",
        "/repo/.kronn/worktrees/child",
        "/repo/.kronn/worktrees/child",
        "kronn/task/KT-1-abcdef",
        "headsha",
        "basesha",
    )
    .unwrap();
    assert_eq!(ws.ownership, "managed");
    assert_eq!(ws.state, "attached");
    assert_eq!(ws.session_pk, None, "a managed row carries no CLI session");
    assert_eq!(ws.task_execution_id.as_deref(), Some(exec.as_str()));
    assert_eq!(ws.parent_discussion_id.as_deref(), Some("d-parent"));
    assert_eq!(ws.base_sha.as_deref(), Some("basesha"));

    // Idempotent by task_execution_id: a compensable retry re-attaches its OWN
    // row (same id) and refreshes the mutable fields, never a duplicate.
    let again = upsert_managed(
        &conn,
        &exec,
        "d1",
        "d-parent",
        Some("task-1"),
        "p1",
        "/repo/.kronn/worktrees/child",
        "/repo/.kronn/worktrees/child",
        "kronn/task/KT-1-abcdef",
        "newhead",
        "basesha",
    )
    .unwrap();
    assert_eq!(
        again.id, ws.id,
        "same execution reuses the same managed row"
    );
    assert_eq!(again.head_sha.as_deref(), Some("newhead"));
    assert_eq!(
        get_managed_for_execution(&conn, &exec).unwrap().unwrap().id,
        ws.id
    );

    // Mark-missing keeps the auditable row (compensation-friendly, no orphan).
    let missing = mark_missing_for_execution(&conn, &exec).unwrap().unwrap();
    assert_eq!(missing.state, "missing");
    assert_eq!(missing.ownership, "managed");

    // Successful physical cleanup retires the checkout without erasing the
    // room ↔ execution ↔ branch/commit evidence.
    let retired = retire_managed_for_execution(&conn, &exec, Some("delivered-head"))
        .unwrap()
        .unwrap();
    assert_eq!(retired.state, "detached");
    assert_eq!(retired.canonical_path, None);
    assert_eq!(
        retired.workspace_path.as_deref(),
        Some("/repo/.kronn/worktrees/child")
    );
    assert_eq!(retired.head_sha.as_deref(), Some("delivered-head"));
    assert_eq!(retired.task_execution_id.as_deref(), Some(exec.as_str()));

    let principal_rows = list_visible_for_discussion(&conn, "d-parent").unwrap();
    assert_eq!(principal_rows.len(), 1);
    assert_eq!(principal_rows[0].disc_id, "d1");
    assert_eq!(principal_rows[0].state, "detached");
    assert_eq!(
        get_visible_for_discussion(&conn, "d-parent", &ws.id)
            .unwrap()
            .unwrap()
            .id,
        ws.id
    );

    // Compensation still has an ownership-aware destructive primitive for
    // workspaces which never reached a deliverable execution.
    assert!(delete_managed_for_execution(&conn, &exec).unwrap());
    assert!(get_managed_for_execution(&conn, &exec).unwrap().is_none());
}

#[test]
fn managed_workspace_set_null_orphan_is_explicitly_collectable() {
    let conn = fixture();
    let exec = seed_execution(&conn, "task-orphan", 2);
    let workspace = upsert_managed(
        &conn,
        &exec,
        "d1",
        "d-parent",
        Some("task-orphan"),
        "p1",
        "/repo/.kronn/worktrees/orphan",
        "/repo/.kronn/worktrees/orphan",
        "kronn/task/KT-2-orphan",
        "head",
        "base",
    )
    .unwrap();

    conn.execute("DELETE FROM task_executions WHERE id = ?1", [&exec])
        .unwrap();
    let orphans = list_orphaned_managed(&conn).unwrap();
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].id, workspace.id);
    assert_eq!(orphans[0].task_execution_id, None);
    assert!(delete_orphaned_managed(&conn, &workspace.id).unwrap());
    assert!(list_orphaned_managed(&conn).unwrap().is_empty());
}

#[test]
fn one_managed_row_per_execution_and_managed_coexists_with_external() {
    let conn = fixture();
    let exec = seed_execution(&conn, "task-1", 1);
    upsert_managed(
        &conn,
        &exec,
        "d1",
        "d-parent",
        None,
        "p1",
        "/wt/a",
        "/wt/a",
        "kronn/task/KT-1-a",
        "h",
        "b",
    )
    .unwrap();

    // A SECOND managed row for the SAME execution (different physical path) is
    // rejected by the unique partial index — a retry cannot fork a second row.
    let dup = conn.execute(
        "INSERT INTO discussion_workspaces \
             (id, disc_id, session_pk, project_id, workspace_path, canonical_path, branch, \
              head_sha, ownership, state, created_at, updated_at, task_execution_id) \
             VALUES ('other', 'd1', NULL, 'p1', '/wt/b', '/wt/b', 'br', 'h', 'managed', \
                     'attached', 'now', 'now', ?1)",
        params![exec],
    );
    assert!(
        dup.is_err(),
        "a second managed row for one execution must be rejected"
    );

    // A managed row (session_pk NULL) and an external row (session_pk set)
    // coexist: the external unique index is (disc_id, session_pk) WHERE
    // session_pk IS NOT NULL, so a NULL-session managed row never collides.
    // Session id 9 (fixture already holds id 7) avoids a PK/uniqueness clash.
    conn.execute(
        "INSERT INTO discussion_sessions \
             (id, disc_id, agent_type, session_id, role, status, joined_at) \
             VALUES (9, 'd1', 'Codex', 'sess-9', 'peer', 'active', 'now')",
        [],
    )
    .unwrap();
    upsert_external(
        &conn, "d1", 9, None, "p1", "/wt/ext", "/wt/ext", "br2", "h2",
    )
    .unwrap();
    assert_eq!(list_for_discussion(&conn, "d1").unwrap().len(), 2);
}

#[test]
fn get_managed_for_discussion_detects_backend_owned_row_regardless_of_path() {
    let conn = fixture();
    let exec = seed_execution(&conn, "task-1", 1);

    // No managed row yet → nothing to refuse.
    assert!(get_managed_for_discussion(&conn, "d1").unwrap().is_none());

    upsert_managed(
        &conn,
        &exec,
        "d1",
        "d-parent",
        Some("task-1"),
        "p1",
        "/repo/.kronn/worktrees/child",
        "/repo/.kronn/worktrees/child",
        "kronn/task/KT-1-abcdef",
        "h",
        "b",
    )
    .unwrap();

    // The refusal keys on (disc_id, ownership='managed'), so it fires whether a
    // CLI re-declares the SAME canonical path or a DIFFERENT one — both resolve
    // to "this room already has a backend-owned worktree, read it".
    let found = get_managed_for_discussion(&conn, "d1").unwrap().unwrap();
    assert_eq!(found.ownership, "managed");
    assert_eq!(found.task_execution_id.as_deref(), Some(exec.as_str()));

    // An external-only room is NOT refused — the detection must not over-fire.
    conn.execute(
        "INSERT INTO discussions (id, project_id, title, created_at, updated_at, workspace_mode) \
             VALUES ('d-ext', 'p1', 'Ext', 'now', 'now', 'Direct')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discussion_sessions \
             (id, disc_id, agent_type, session_id, role, status, joined_at) \
             VALUES (11, 'd-ext', 'Codex', 'sess-11', 'peer', 'active', 'now')",
        [],
    )
    .unwrap();
    upsert_external(
        &conn, "d-ext", 11, None, "p1", "/wt/ext", "/wt/ext", "br", "h",
    )
    .unwrap();
    assert!(get_managed_for_discussion(&conn, "d-ext")
        .unwrap()
        .is_none());
}
