use super::*;
use crate::api::audit::briefing::with_kronn_options;

const SHEET: &str = "# TD-20261001-a\n\n- **Area**: Backend\n- **Severity**: High\n- **Status**: Verified in source\n\n## Problem (fact)\nIt panics.\n";

async fn fixture() -> (Arc<crate::db::Database>, tempfile::TempDir) {
    let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('p1', 'P1', '/repo', '2026-10-11T00:00:00Z', '2026-10-11T00:00:00Z')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let td = dir.path().join("docs/tech-debt");
    std::fs::create_dir_all(&td).unwrap();
    std::fs::write(td.join("TD-20261001-a.md"), SHEET).unwrap();
    (db, dir)
}

async fn tasks(db: &Arc<crate::db::Database>) -> i64 {
    db.with_conn(
        |conn| Ok(conn.query_row("SELECT COUNT(*) FROM planning_tasks", [], |r| r.get(0))?),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn briefing_option_converts_after_a_successful_full_audit() {
    let (db, dir) = fixture().await;
    let outcome = convert_after_audit(&db, AuditKind::Full, true, true, "p1", dir.path(), "run-1")
        .await
        .expect("conversion applies")
        .unwrap();
    assert_eq!(outcome.created, 1);
    assert_eq!(tasks(&db).await, 2, "the sheet and its theme parent");
    let again = convert_after_audit(&db, AuditKind::Full, true, true, "p1", dir.path(), "run-2")
        .await
        .unwrap()
        .unwrap();
    assert_eq!((again.created, again.already_linked), (0, 1));
    assert_eq!(tasks(&db).await, 2);
}

#[tokio::test]
async fn no_conversion_without_the_option_or_a_complete_full_audit() {
    let (db, dir) = fixture().await;
    for (kind, succeeded, option) in [
        (AuditKind::Full, true, false),
        (AuditKind::Full, false, true),
        (AuditKind::Security, true, true),
    ] {
        let result =
            convert_after_audit(&db, kind, succeeded, option, "p1", dir.path(), "run").await;
        assert!(result.is_none(), "{kind:?} {succeeded} {option}");
    }
    assert_eq!(tasks(&db).await, 0);
}

#[test]
fn checkbox_round_trips_through_the_briefing() {
    let body = "# Project Briefing\n\n## Purpose\nX\n".to_string();
    let checked = with_kronn_options(body.clone(), true);
    assert!(briefing_option_set(Some(&checked), None));
    assert_eq!(with_kronn_options(body.clone(), false), body);
    assert!(!briefing_option_set(Some(&body), None));
    assert!(!briefing_option_set(Some(&body), Some(&checked)));
    assert!(!briefing_option_set(None, None));
}
