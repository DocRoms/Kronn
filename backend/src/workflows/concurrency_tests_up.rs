
use super::*;

fn run_for(id: &str, project: &str, status: crate::models::RunStatus) -> WorkflowRun {
    serde_json::from_value(serde_json::json!({
        "id": id, "workflow_id": "wf-scope", "status": status,
        "trigger_context": null, "step_results": [], "tokens_used": 0,
        "workspace_path": null, "started_at": "2026-01-01T00:00:00Z",
        "finished_at": null, "run_type": "linear", "batch_total": 0,
        "batch_completed": 0, "batch_failed": 0, "batch_no_response": 0,
        "batch_name": null, "parent_run_id": null, "state": {},
        "produced_branches": [], "concurrency_key": null,
        "project_id": project
    }))
    .unwrap()
}

/// A run parked on a provider quota, as KT-811 leaves it.
fn quota_run(id: &str, parent: Option<&str>, wake_at: Option<&str>) -> WorkflowRun {
    let mut run = run_for(id, "p", crate::models::RunStatus::WaitingQuota);
    run.parent_run_id = parent.map(String::from);
    run.step_results = vec![serde_json::from_value(serde_json::json!({
        "step_name": "Agent", "status": "WaitingQuota", "output": "", "duration_ms": 0,
        "quota_wait": {
            "id": "w1", "reset_at": "2026-11-11T02:00:00Z",
            "wake_at": wake_at, "attempt": 1
        }
    }))
    .unwrap()];
    run
}

fn limited_workflow(conn: &Connection) -> Workflow {
    crate::db::migrations::run(conn).unwrap();
    conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at) VALUES ('p', 'p', 'p', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    crate::db::resource_identities::tests::seed_workflow(conn, "wf-scope", "PR Review v3.4", None);
    let mut workflow = crate::db::workflows::get_workflow(conn, "wf-scope")
        .unwrap()
        .unwrap();
    workflow.concurrency_limit = Some(1);
    workflow
}

/// KT-1657 — the refusal names what holds the slot, not just a count.
#[test]
fn a_refused_launch_names_the_quota_paused_run_its_wake_time_and_the_actions() {
    let conn = Connection::open_in_memory().unwrap();
    let workflow = limited_workflow(&conn);
    // The child's parent belongs to another workflow, so it holds no slot here.
    crate::db::resource_identities::tests::seed_workflow(&conn, "wf-parent", "Parent", None);
    let mut parent = run_for("f50d2fe6-bbbb", "p", crate::models::RunStatus::WaitingQuota);
    parent.workflow_id = "wf-parent".into();
    crate::db::workflows::insert_run(&conn, &parent).unwrap();
    insert_run_within_limit(
        &conn,
        &workflow,
        &quota_run(
            "5d917cf0-aaaa",
            Some("f50d2fe6-bbbb"),
            Some("2026-11-11T02:02:00Z"),
        ),
    )
    .unwrap()
    .unwrap();

    let refused = insert_run_within_limit(
        &conn,
        &workflow,
        &run_for("r-new", "p", crate::models::RunStatus::Pending),
    )
    .unwrap()
    .unwrap_err();
    assert!(
        refused.starts_with("Concurrency limit reached (1/1)"),
        "{refused}"
    );
    assert!(refused.contains("run 5d917cf0"), "{refused}");
    assert!(refused.contains("(child of run f50d2fe6)"), "{refused}");
    assert!(refused.contains("\"PR Review v3.4\""), "{refused}");
    assert!(refused.contains("WaitingQuota"), "{refused}");
    assert!(
        refused.contains("resumes automatically at 2026-11-11 02:02 UTC"),
        "{refused}"
    );
    // A child is resumed through its parent; the agent switch targets the child.
    assert!(
        refused.contains("resume it now (POST /api/workflow-runs/f50d2fe6-bbbb/resume)"),
        "{refused}"
    );
    assert!(refused.contains("open the run"), "{refused}");
    assert!(refused.contains("cancel it"), "{refused}");
    assert!(
            refused.contains("switch the agent of its waiting step (POST /api/workflow-runs/5d917cf0-aaaa/quota-agent)"),
            "{refused}"
        );

    // A parked wait has no wake time: it says a human resumes it.
    let conn = Connection::open_in_memory().unwrap();
    let workflow = limited_workflow(&conn);
    insert_run_within_limit(&conn, &workflow, &quota_run("r-parked", None, None))
        .unwrap()
        .unwrap();
    let refused = insert_run_within_limit(
        &conn,
        &workflow,
        &run_for("r-new", "p", crate::models::RunStatus::Pending),
    )
    .unwrap()
    .unwrap_err();
    assert!(
        refused.contains("waiting for a human to resume it"),
        "{refused}"
    );
    assert!(
        refused.contains("resume it now (POST /api/workflow-runs/r-parked/resume)"),
        "{refused}"
    );
}

#[test]
fn a_refusal_names_a_gate_or_running_blocker_and_counts_the_rest() {
    use crate::models::RunStatus;
    let conn = Connection::open_in_memory().unwrap();
    let mut workflow = limited_workflow(&conn);
    workflow.concurrency_limit = Some(5);
    for (id, status) in [
        ("g1", RunStatus::WaitingApproval),
        ("r1", RunStatus::Running),
        ("r2", RunStatus::Running),
        ("r3", RunStatus::Pending),
        ("r4", RunStatus::Pending),
    ] {
        insert_run_within_limit(&conn, &workflow, &run_for(id, "p", status))
            .unwrap()
            .unwrap();
    }
    let refused =
        insert_run_within_limit(&conn, &workflow, &run_for("new", "p", RunStatus::Pending))
            .unwrap()
            .unwrap_err();
    assert!(refused.contains("(5/5)"), "{refused}");
    assert!(
        refused.contains("run g1 of \"PR Review v3.4\" is WaitingApproval"),
        "{refused}"
    );
    assert!(
        refused.contains("open the run to approve or reject it"),
        "{refused}"
    );
    assert!(
        refused.contains("run r1 of \"PR Review v3.4\" is Running"),
        "{refused}"
    );
    assert!(refused.contains("2 more run(s) hold a slot"), "{refused}");
    assert!(!refused.contains("run r4"), "{refused}");
}

#[test]
fn a_resume_refusal_also_names_its_blocker() {
    let conn = Connection::open_in_memory().unwrap();
    let workflow = limited_workflow(&conn);
    insert_run_within_limit(&conn, &workflow, &quota_run("r-held", None, None))
        .unwrap()
        .unwrap();
    let other = run_for("r-other", "p", crate::models::RunStatus::Interrupted);
    let refused = resume_within_limit(&conn, &workflow, &other)
        .unwrap()
        .unwrap_err();
    assert!(
        refused.contains("Resume this run once it has finished."),
        "{refused}"
    );
    assert!(refused.contains("run r-held"), "{refused}");
}

#[test]
fn the_active_board_lists_a_quota_paused_run_with_its_wake_time() {
    let conn = Connection::open_in_memory().unwrap();
    let workflow = limited_workflow(&conn);
    insert_run_within_limit(
        &conn,
        &workflow,
        &quota_run("r-paused", None, Some("2026-11-11T02:02:00Z")),
    )
    .unwrap()
    .unwrap();
    let mut finished = run_for("r-done", "p", crate::models::RunStatus::Success);
    finished.workflow_id = "wf-scope".into();
    crate::db::workflows::insert_run(&conn, &finished).unwrap();

    let board = crate::db::workflows::list_active_runs(&conn, None).unwrap();
    assert_eq!(board.len(), 1, "{board:?}");
    assert_eq!(board[0].run_id, "r-paused");
    assert_eq!(board[0].workflow_name, "PR Review v3.4");
    assert_eq!(board[0].status, crate::models::RunStatus::WaitingQuota);
    let quota = board[0].quota.as_ref().expect("the wait is listed");
    assert_eq!(quota.step_name, "Agent");
    assert_eq!(
        quota.wake_at.map(|at| at.to_rfc3339()),
        Some("2026-11-11T02:02:00+00:00".to_string())
    );
    assert_eq!(
        quota.reset_at.map(|at| at.to_rfc3339()),
        Some("2026-11-11T02:00:00+00:00".to_string())
    );

    // A bridge token sees its own project's runs only.
    let other = crate::db::workflows::RunVisibility {
        project: Some("elsewhere".into()),
        own_run: None,
    };
    assert!(crate::db::workflows::list_active_runs(&conn, Some(&other))
        .unwrap()
        .is_empty());
    let own = crate::db::workflows::RunVisibility {
        project: Some("p".into()),
        own_run: None,
    };
    assert_eq!(
        crate::db::workflows::list_active_runs(&conn, Some(&own))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_multi_project_limit_counts_each_projects_runs_apart() {
    use crate::models::RunStatus;
    let conn = Connection::open_in_memory().unwrap();
    crate::db::migrations::run(&conn).unwrap();
    for id in ["a", "b", "c"] {
        conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at) VALUES (?1, ?1, ?1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [id],
            )
            .unwrap();
    }
    crate::db::resource_identities::tests::seed_workflow(&conn, "wf-scope", "Scope", None);
    let mut workflow = crate::db::workflows::get_workflow(&conn, "wf-scope")
        .unwrap()
        .unwrap();
    workflow.concurrency_limit = Some(1);
    workflow.project_scope = Some(crate::models::WorkflowProjectScope::All);
    insert_run_within_limit(
        &conn,
        &workflow,
        &run_for("r-a1", "a", RunStatus::WaitingApproval),
    )
    .unwrap()
    .unwrap();

    assert!(
        insert_run_within_limit(&conn, &workflow, &run_for("r-b1", "b", RunStatus::Pending))
            .unwrap()
            .is_ok(),
        "another project has its own slot"
    );
    let refused =
        insert_run_within_limit(&conn, &workflow, &run_for("r-a2", "a", RunStatus::Pending))
            .unwrap()
            .unwrap_err();
    assert!(refused.contains("(1/1)"), "{refused}");
    let paused = run_for("r-a1", "a", RunStatus::WaitingApproval);
    assert!(resume_within_limit(&conn, &workflow, &paused)
        .unwrap()
        .is_ok());

    // A single-project workflow keeps one bucket.
    workflow.project_scope = None;
    assert!(
        insert_run_within_limit(&conn, &workflow, &run_for("r-c1", "c", RunStatus::Pending))
            .unwrap()
            .is_err()
    );
}
