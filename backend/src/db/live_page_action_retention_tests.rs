use super::tests::{action_block, connection, insert_page, insert_target};
use super::*;

fn offers(conn: &Connection, page: &str, refs: &[&str]) -> Vec<LivePageAction> {
    let html = refs
        .iter()
        .map(|name| action_block(name, r#"{"kind":"quick_exec","target_id":"qe-1"}"#))
        .collect::<String>();
    let revision = format!("{page}-revision");
    insert_page(conn, page, &revision, &html);
    ingest_page_actions(conn, page, &revision, &html).unwrap();
    list_for_live_page(conn, page).unwrap()
}

fn seed_history(conn: &Connection, offer: &LivePageAction, count: i64, prefix: &str) {
    let tx = conn.unchecked_transaction().unwrap();
    let states = [
        DiscussionActionState::Succeeded,
        DiscussionActionState::Failed,
        DiscussionActionState::Cancelled,
        DiscussionActionState::PreflightFailed,
    ];
    for i in 0..count {
        insert_launch(
            &tx,
            &format!("page-launch:{prefix}-{i:04}"),
            offer,
            &format!("row-{i}"),
            states[(i % 4) as usize],
            "2026-09-21T00:00:00Z",
        )
        .unwrap();
    }
    tx.commit().unwrap();
}

fn count(conn: &Connection, offer: &LivePageAction) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM live_page_action_launches WHERE action_id=?1",
        [&offer.id],
        |row| row.get(0),
    )
    .unwrap()
}

fn stored_state(conn: &Connection, id: &str) -> Option<String> {
    conn.query_row(
        "SELECT state FROM live_page_action_launches WHERE id=?1",
        [id],
        |row| row.get(0),
    )
    .optional()
    .unwrap()
}

#[test]
fn retention_is_per_block_across_bindings_and_a_pruned_handle_never_relaunches() {
    let conn = connection();
    insert_target(&conn);
    let page = offers(&conn, "page-one", &["refresh", "another"]);
    let first = &page[0];
    let second = &page[1];
    let other_page = offers(&conn, "page-two", &[&first.action_ref]);
    seed_history(&conn, first, MAX_RETAINED_TERMINAL_LAUNCHES + 2, "first");
    seed_history(&conn, second, 4, "second");
    seed_history(&conn, &other_page[0], 5, "other");
    let current = cancel(&conn, &first.id).unwrap().unwrap();
    assert_eq!(count(&conn, first), MAX_RETAINED_TERMINAL_LAUNCHES);
    assert_eq!(count(&conn, second), 4);
    assert_eq!(count(&conn, &other_page[0]), 5);
    assert_eq!(
        stored_state(&conn, &current.id).as_deref(),
        Some("cancelled")
    );
    assert!(stored_state(&conn, "page-launch:first-0000").is_none());
    assert!(stored_state(
        &conn,
        &format!(
            "page-launch:first-{:04}",
            MAX_RETAINED_TERMINAL_LAUNCHES + 1
        )
    )
    .is_some());
    assert!(claim_launch(
        &conn,
        "page-launch:first-0000",
        &HashMap::new(),
        &HashMap::new()
    )
    .unwrap()
    .is_none());
    assert_eq!(count(&conn, first), MAX_RETAINED_TERMINAL_LAUNCHES);
    assert_eq!(
        declaration_by_id(&conn, &first.id).unwrap().unwrap().state,
        DiscussionActionState::Proposed
    );
    let jobs: i64 = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM agent_dispatch_jobs)+(SELECT COUNT(*) FROM shared_runs)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(jobs, 0);
}

#[test]
fn retention_reconciles_completed_async_runs_but_preserves_active_claims_and_results() {
    let conn = connection();
    insert_target(&conn);
    let offer = &offers(&conn, "page-async", &["refresh"])[0];
    seed_history(&conn, offer, MAX_RETAINED_TERMINAL_LAUNCHES, "history");
    for (id, status) in [("done", "success"), ("live", "running")] {
        conn.execute("INSERT INTO shared_runs(id,kind,source_id,status,result_json,diagnostic,finished_at,created_at,updated_at) VALUES (?1,'quick_exec','qe-1',?2,'{\"kept\":true}',NULL,'2026-09-20T00:00:00Z','2026-09-20T00:00:00Z','2026-09-20T00:00:00Z')",params![id,status]).unwrap();
    }
    let now = Utc::now().to_rfc3339();
    for (id, binding, state, run) in [
        (
            "page-launch:done",
            "old",
            DiscussionActionState::Running,
            Some("done"),
        ),
        (
            "page-launch:active",
            "",
            DiscussionActionState::Running,
            Some("live"),
        ),
        (
            "page-launch:starting",
            "starting",
            DiscussionActionState::Launching,
            None,
        ),
    ] {
        insert_launch(&conn, id, offer, binding, state, &now).unwrap();
        conn.execute(
            "UPDATE live_page_action_launches SET shared_run_id=?2,launched_at=?3 WHERE id=?1",
            params![id, run, now],
        )
        .unwrap();
    }
    // Persisted history says running even though the shared run is finished.
    assert_eq!(
        stored_state(&conn, "page-launch:done").as_deref(),
        Some("running")
    );
    cancel(&conn, &offer.id).unwrap();
    assert!(stored_state(&conn, "page-launch:done").is_none());
    assert_eq!(
        stored_state(&conn, "page-launch:active").as_deref(),
        Some("running")
    );
    assert_eq!(
        stored_state(&conn, "page-launch:starting").as_deref(),
        Some("launching")
    );
    assert_eq!(count(&conn, offer), MAX_RETAINED_TERMINAL_LAUNCHES + 2);
    let result: String = conn
        .query_row(
            "SELECT result_json FROM shared_runs WHERE id='done'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(result, r#"{"kept":true}"#);
    let replay = claim_launch(
        &conn,
        &offer.id,
        &HashMap::from([("service".into(), "api".into())]),
        &HashMap::new(),
    )
    .unwrap()
    .unwrap();
    assert!(
        matches!(replay,LivePageActionClaimOutcome::Existing(action) if action.id == "page-launch:active")
    );
}

fn failed_completion() -> kronn_action_engine::ActionCompletion {
    kronn_action_engine::ActionCompletion {
        state: DiscussionActionState::Failed,
        shared_run_id: None,
        result_discussion_id: None,
        deep_link: None,
        diagnostic: Some("test failure".into()),
    }
}

#[test]
fn retention_and_completion_roll_back_together_and_compose_with_an_outer_transaction() {
    let conn = connection();
    insert_target(&conn);
    let offer = &offers(&conn, "page-atomic", &["refresh"])[0];
    seed_history(&conn, offer, MAX_RETAINED_TERMINAL_LAUNCHES, "atomic");
    let LivePageActionClaimOutcome::Claimed { action, .. } = claim_launch(
        &conn,
        &offer.id,
        &HashMap::from([("service".into(), "api".into())]),
        &HashMap::new(),
    )
    .unwrap()
    .unwrap() else {
        panic!("new claim")
    };
    conn.execute_batch("CREATE TRIGGER refuse_retention BEFORE DELETE ON live_page_action_launches BEGIN SELECT RAISE(ABORT,'retention blocked'); END;").unwrap();
    assert!(complete(&conn, &action.id, failed_completion())
        .unwrap_err()
        .to_string()
        .contains("retention blocked"));
    assert!(conn.is_autocommit());
    assert_eq!(
        stored_state(&conn, &action.id).as_deref(),
        Some("launching")
    );
    assert_eq!(count(&conn, offer), MAX_RETAINED_TERMINAL_LAUNCHES + 1);
    assert!(cancel(&conn, &offer.id).is_err());
    assert_eq!(count(&conn, offer), MAX_RETAINED_TERMINAL_LAUNCHES + 1);
    conn.execute_batch("DROP TRIGGER refuse_retention").unwrap();
    let transaction = conn.unchecked_transaction().unwrap();
    complete(&transaction, &action.id, failed_completion()).unwrap();
    assert_eq!(count(&transaction, offer), MAX_RETAINED_TERMINAL_LAUNCHES);
    transaction.rollback().unwrap();
    assert_eq!(
        stored_state(&conn, &action.id).as_deref(),
        Some("launching")
    );
    assert_eq!(count(&conn, offer), MAX_RETAINED_TERMINAL_LAUNCHES + 1);
    complete(&conn, &action.id, failed_completion()).unwrap();
    assert_eq!(stored_state(&conn, &action.id).as_deref(), Some("failed"));
    assert_eq!(count(&conn, offer), MAX_RETAINED_TERMINAL_LAUNCHES);
}
