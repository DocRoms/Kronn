use super::*;

#[tokio::test]
async fn boot_repairs_a_database_that_lost_a_table_and_campaign_control_works_again() {
    use crate::models::{
        OrchestrationActor, OrchestrationControlState, OrchestrationRunInput, PlanningActorKind,
    };

    fn actor() -> OrchestrationActor {
        OrchestrationActor {
            kind: PlanningActorKind::Backend,
            id: Some("orchestrator".into()),
            session_id: None,
            source_message_id: None,
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kronn.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn.execute(
        "INSERT INTO discussions (id, title, created_at, updated_at) \
             VALUES ('disc-1', 'D', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    let run = crate::db::orchestration::create_orchestration_run(
        &conn,
        &OrchestrationRunInput::single_task("disc-1"),
    )
    .unwrap();
    conn.execute_batch("DROP TABLE orchestration_run_events;")
        .unwrap();
    // The failure of the issue: the write path hits the missing table.
    let broken = crate::db::orchestration::set_orchestration_control_state(
        &conn,
        &run.id,
        OrchestrationControlState::Cancelled,
        Some("cancel"),
        &actor(),
    );
    assert!(format!("{:#}", broken.unwrap_err()).contains("no such table"));
    drop(conn);

    // A restart: the boot check recreates the table.
    let db = crate::db::Database::open_path(&path).unwrap();
    let report = last_report().expect("the boot check recorded a report");
    assert!(report
        .repaired
        .iter()
        .any(|entry| entry.starts_with("table orchestration_run_events")));
    assert!(report.unrepaired.is_empty(), "{report:?}");

    let run_id = run.id.clone();
    let cancelled = db
        .with_conn(move |conn| {
            crate::db::orchestration::set_orchestration_control_state(
                conn,
                &run_id,
                OrchestrationControlState::Cancelled,
                Some("cancel"),
                &actor(),
            )
        })
        .await
        .unwrap();
    assert_eq!(
        cancelled.control_state,
        OrchestrationControlState::Cancelled
    );
}
