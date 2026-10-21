use super::*;
use crate::models::PlanningActorKind;

fn conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn.execute(
        "INSERT INTO projects (id, name, path, created_at, updated_at)
         VALUES ('p1', 'P1', '/repo', '2026-10-11T00:00:00Z', '2026-10-11T00:00:00Z')",
        [],
    )
    .unwrap();
    conn
}

fn sheet(id: &str, severity: &str, status: &str, extra: &str) -> TdTaskSource {
    let content = format!(
        "---\nname: {}\ndescription: Summary of {id}\nmetadata:\n  type: tech-debt\n  audit_history:\n    - date: 2026-10-01\n      status: {status}\n      reviewer: Full audit\n---\n\n# {id}\n\n- **Area**: Backend\n- **Severity**: {severity}\n- **Status**: {status}\n{extra}\n## Problem (fact)\nThe handler of {id} panics\non empty input.\n\n## Impact\nCrash.\n",
        id.to_lowercase()
    );
    source_from(id, &content, format!("docs/tech-debt/{id}.md"))
}

fn actor() -> PlanningActor {
    PlanningActor {
        kind: PlanningActorKind::Backend,
        id: Some("audit:test".into()),
        session_id: None,
        source_message_id: None,
    }
}

fn task_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM planning_tasks", [], |r| r.get(0))
        .unwrap()
}

fn sample() -> Vec<TdTaskSource> {
    vec![
        sheet(
            "TD-20261001-a",
            "High",
            "Verified in source",
            "- **Category**: security\n",
        ),
        sheet(
            "TD-20261001-b",
            "Low",
            "Verified in source",
            "- **Category**: security\n",
        ),
        sheet("TD-20261001-c", "Medium", "Inferred", ""),
        sheet("TD-20261001-rej", "Critical", "Rejected", ""),
        sheet("TD-20261001-adr", "High", "Accepted decision", ""),
    ]
}

#[test]
fn second_conversion_creates_nothing_and_reports_links() {
    let conn = conn();
    let sources = sample();
    let first = convert(&conn, "p1", &sources, &actor()).unwrap();
    assert_eq!(
        (first.created, first.already_linked, first.skipped),
        (3, 0, 2)
    );
    let after_first = task_count(&conn);
    assert_eq!(after_first, 5, "3 sheets + 2 theme parents");

    let second = convert(&conn, "p1", &sources, &actor()).unwrap();
    assert_eq!(
        (second.created, second.already_linked, second.skipped),
        (0, 3, 2)
    );
    assert_eq!(task_count(&conn), after_first);
    assert!(second.themes.iter().all(|t| !t.created));
    let linked: Vec<_> = second
        .items
        .iter()
        .filter(|i| i.state == TdTaskState::AlreadyLinked)
        .map(|i| i.task_reference.clone().unwrap())
        .collect();
    let created: Vec<_> = first
        .items
        .iter()
        .filter(|i| i.state == TdTaskState::Created)
        .map(|i| i.task_reference.clone().unwrap())
        .collect();
    assert_eq!(linked.len(), 3);
    assert!(linked.iter().all(|r| created.contains(r)));
}

#[test]
fn a_changed_sheet_is_still_reported_as_linked() {
    let conn = conn();
    convert(&conn, "p1", &sample()[..1], &actor()).unwrap();
    let mut edited = sample();
    edited[0].sheet.title = "Reworded after a re-audit".into();
    let again = convert(&conn, "p1", &edited[..1], &actor()).unwrap();
    assert_eq!((again.created, again.already_linked), (0, 1));
}

#[test]
fn a_new_sheet_joins_the_existing_theme_parent() {
    let conn = conn();
    let sources = sample();
    convert(&conn, "p1", &sources[..1], &actor()).unwrap();
    let outcome = convert(&conn, "p1", &sources[..2], &actor()).unwrap();
    assert_eq!((outcome.created, outcome.already_linked), (1, 1));
    let parents: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM planning_tasks WHERE parent_id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(parents, 1);
    let children: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM planning_tasks WHERE parent_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(children, 2);
}

#[test]
fn sheets_are_grouped_under_one_parent_per_theme() {
    let conn = conn();
    let outcome = convert(&conn, "p1", &sample(), &actor()).unwrap();
    let mut themes: Vec<_> = outcome.themes.iter().map(|t| t.theme.as_str()).collect();
    themes.sort();
    assert_eq!(themes, vec!["backend", "security"]);

    let security = outcome
        .themes
        .iter()
        .find(|t| t.theme == "security")
        .unwrap();
    let parent = crate::db::planning::get_task(&conn, &security.task_reference)
        .unwrap()
        .unwrap();
    assert_eq!(parent.summary.total_subtasks, 2);
    assert_eq!(
        parent.summary.priority,
        PlanningTaskPriority::High,
        "highest child"
    );
    assert_eq!(parent.summary.project_ids, vec!["p1".to_string()]);
    assert!(parent.summary.tags.iter().any(|t| t == TECH_DEBT_TAG));
}

#[test]
fn rejected_and_adr_covered_sheets_get_no_task() {
    let conn = conn();
    let outcome = convert(&conn, "p1", &sample(), &actor()).unwrap();
    let skipped: Vec<_> = outcome
        .items
        .iter()
        .filter(|i| i.state == TdTaskState::Skipped)
        .map(|i| (i.td_id.as_str(), i.reason.as_deref()))
        .collect();
    assert_eq!(
        skipped,
        vec![
            ("TD-20261001-rej", Some("rejected")),
            ("TD-20261001-adr", Some("accepted_decision")),
        ]
    );
    for id in ["TD-20261001-rej", "TD-20261001-adr"] {
        let key = td_task_key("p1", id);
        assert!(crate::db::planning::task_by_idempotency_key(&conn, &key)
            .unwrap()
            .is_none());
    }
}

#[test]
fn only_skipped_sheets_create_no_parent() {
    let conn = conn();
    let outcome = convert(&conn, "p1", &sample()[3..], &actor()).unwrap();
    assert_eq!((outcome.created, outcome.skipped), (0, 2));
    assert!(outcome.themes.is_empty());
    assert_eq!(task_count(&conn), 0);
}

#[test]
fn task_carries_the_sheet_content() {
    let conn = conn();
    let outcome = convert(&conn, "p1", &sample()[..1], &actor()).unwrap();
    let item = outcome
        .items
        .iter()
        .find(|i| i.state == TdTaskState::Created)
        .unwrap();
    let task = crate::db::planning::get_task(&conn, item.task_reference.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        task.summary.title,
        "Summary of TD-20261001-a (TD-20261001-a)"
    );
    assert_eq!(task.summary.priority, PlanningTaskPriority::High);
    assert_eq!(task.summary.project_ids, vec!["p1".to_string()]);
    assert!(task.description.contains("docs/tech-debt/TD-20261001-a.md"));
    assert!(task.description.contains("- Severity: High"));
    assert!(task.description.contains("- Theme: security"));
    assert!(task
        .description
        .contains("The handler of TD-20261001-a panics on empty input."));
    assert_eq!(task.definition_of_done.len(), 1);
    assert!(task.definition_of_done[0]
        .sentence
        .contains("no longer reproduces"));
}

#[test]
fn keys_are_scoped_per_project() {
    let conn = conn();
    conn.execute(
        "INSERT INTO projects (id, name, path, created_at, updated_at)
         VALUES ('p2', 'P2', '/repo2', '2026-10-11T00:00:00Z', '2026-10-11T00:00:00Z')",
        [],
    )
    .unwrap();
    convert(&conn, "p1", &sample()[..1], &actor()).unwrap();
    let other = convert(&conn, "p2", &sample()[..1], &actor()).unwrap();
    assert_eq!(other.created, 1);
}

#[test]
fn severity_maps_to_priority() {
    assert_eq!(priority_for("Critical"), PlanningTaskPriority::Critical);
    assert_eq!(priority_for("high"), PlanningTaskPriority::High);
    assert_eq!(priority_for("Medium"), PlanningTaskPriority::Normal);
    assert_eq!(priority_for("Low"), PlanningTaskPriority::Low);
    assert_eq!(priority_for("Unknown"), PlanningTaskPriority::Normal);
}

#[test]
fn theme_prefers_category_then_area() {
    let with_category = sheet(
        "TD-1-x",
        "Low",
        "Inferred",
        "- **Category**: Code-Quality\n",
    );
    assert_eq!(theme_of(&with_category.sheet).0, "code-quality");
    let mut area_only = sheet("TD-1-y", "Low", "Inferred", "");
    area_only.sheet.area = "Infra (Docker gateway, desktop)".into();
    assert_eq!(theme_of(&area_only.sheet), ("infra".into(), "Infra".into()));
    area_only.sheet.area = "Unknown".into();
    assert_eq!(theme_of(&area_only.sheet).0, "other");
}

#[test]
fn list_style_sheet_yields_problem_and_verification() {
    let content = "# TD-20260314-no-tls\n\n- **ID**: TD-20260314-no-tls\n- **Area**: Infra\n- **Problem (fact)**: Kronn serves plain HTTP.\n  The gateway listens on port 80.\n- **Verification**: `curl -I https://localhost` answers 200.\n- **Next step**: create ticket.\n";
    let source = source_from("TD-20260314-no-tls", content, "docs/tech-debt/x.md".into());
    assert_eq!(
        source.problem.as_deref(),
        Some("Kronn serves plain HTTP. The gateway listens on port 80.")
    );
    assert_eq!(
        source.verification.as_deref(),
        Some("`curl -I https://localhost` answers 200.")
    );
}

#[test]
fn load_sources_reads_only_td_sheets() {
    let dir = tempfile::tempdir().unwrap();
    let td = dir.path().join("docs/tech-debt");
    std::fs::create_dir_all(&td).unwrap();
    std::fs::write(
        td.join("TD-20261001-a.md"),
        "# TD-20261001-a\n- **Severity**: High\n",
    )
    .unwrap();
    std::fs::write(td.join("README.md"), "# Readme\n").unwrap();
    std::fs::write(td.join("_reconciliation.md"), "# Report\n").unwrap();
    let sources = load_sources(&dir.path().join("docs"));
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_path, "docs/tech-debt/TD-20261001-a.md");
}

#[test]
fn briefing_option_line_is_recognised() {
    assert!(briefing_wants_td_tasks(&format!(
        "# Project Briefing\n\n{BRIEFING_OPTIONS_HEADING}\n{BRIEFING_OPTION_LINE}\n"
    )));
    assert!(!briefing_wants_td_tasks(
        "# Project Briefing\n## Purpose\nTasks: yes\n"
    ));
}

fn task_status(conn: &Connection, project: &str, td_id: &str) -> (PlanningTaskStatus, String) {
    let (id, _) = crate::db::planning::task_by_idempotency_key(conn, &td_task_key(project, td_id))
        .unwrap()
        .unwrap();
    let task = crate::db::planning::get_task(conn, &id).unwrap().unwrap();
    (task.summary.status, task.description)
}

#[test]
fn a_sheet_that_leaves_the_active_list_closes_its_task() {
    let conn = conn();
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    let td = docs.join("tech-debt");
    std::fs::create_dir_all(&td).unwrap();
    let ids = [
        "TD-20261001-a",
        "TD-20261001-b",
        "TD-20261001-c",
        "TD-20261001-d",
    ];
    let sources: Vec<TdTaskSource> = ids
        .iter()
        .map(|id| sheet(id, "High", "Verified in source", ""))
        .collect();
    for id in ids {
        std::fs::write(
            td.join(format!("{id}.md")),
            "- **Status**: Verified in source\n",
        )
        .unwrap();
    }
    convert(&conn, "p1", &sources, &actor()).unwrap();
    // a rejected, b accepted, c removed, d still active.
    std::fs::write(td.join("TD-20261001-a.md"), "- **Status**: Rejected\n").unwrap();
    std::fs::write(
        td.join("TD-20261001-b.md"),
        "- **Status**: Accepted decision\n",
    )
    .unwrap();
    std::fs::remove_file(td.join("TD-20261001-c.md")).unwrap();
    // Work on c has started: it is annotated, never archived under the worker.
    let (c_id, _) =
        crate::db::planning::task_by_idempotency_key(&conn, &td_task_key("p1", "TD-20261001-c"))
            .unwrap()
            .unwrap();
    conn.execute(
        "UPDATE planning_tasks SET status='in_progress' WHERE id=?1",
        [&c_id],
    )
    .unwrap();

    let closed = close_inactive(&conn, "p1", &docs, &actor()).unwrap();
    let summary: Vec<(&str, InactiveReason, bool)> = closed
        .iter()
        .map(|c| (c.td_id.as_str(), c.reason, c.archived))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("TD-20261001-a", InactiveReason::Rejected, true),
            ("TD-20261001-b", InactiveReason::AcceptedDecision, true),
            ("TD-20261001-c", InactiveReason::SheetRemoved, false),
        ]
    );
    let (a, a_desc) = task_status(&conn, "p1", "TD-20261001-a");
    assert_eq!(a, PlanningTaskStatus::Archived);
    assert!(a_desc.contains("`TD-20261001-a` was rejected"), "{a_desc}");
    let (c, c_desc) = task_status(&conn, "p1", "TD-20261001-c");
    assert_eq!(c, PlanningTaskStatus::InProgress);
    assert!(c_desc.contains("no longer on file"), "{c_desc}");
    assert_eq!(
        task_status(&conn, "p1", "TD-20261001-d").0,
        PlanningTaskStatus::Todo
    );

    // A second pass changes nothing: closed tasks stay closed, notes are not repeated.
    assert!(close_inactive(&conn, "p1", &docs, &actor())
        .unwrap()
        .is_empty());
    assert_eq!(task_status(&conn, "p1", "TD-20261001-c").1, c_desc);
}
