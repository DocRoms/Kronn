//! KT-1658 — the attention center's aggregation: rows derived from the state
//! that exists, the stalled rule, and a new source with no change elsewhere.

use super::*;
use crate::models::{AuditActivityEntry, AuditRecentActivity};
use rusqlite::params;

const NOW: &str = "2026-11-10T12:00:00Z";

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(NOW)
        .unwrap()
        .with_timezone(&Utc)
}

fn at(minutes_ago: i64) -> String {
    (now() - Duration::minutes(minutes_ago)).to_rfc3339()
}

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn.execute(
        "INSERT INTO projects (id, name, path, created_at, updated_at)
         VALUES ('p1', 'Kronn', '/p', ?1, ?1)",
        [at(1000)],
    )
    .unwrap();
    conn
}

fn overview(conn: &Connection, audits: &[AuditProgress]) -> ActivityOverview {
    let ctx = ActivityContext {
        conn,
        audits,
        now: now(),
    };
    collect_activity(&ctx, &default_sources())
}

fn row<'a>(overview: &'a ActivityOverview, id: &str) -> &'a ActivityRow {
    overview
        .rows
        .iter()
        .find(|row| row.id == id)
        .unwrap_or_else(|| panic!("no row {id} in {:#?}", overview.rows))
}

fn seed_discussion(conn: &Connection, id: &str, title: &str, project: Option<&str>) {
    conn.execute(
        "INSERT INTO discussions (id, title, project_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![id, title, project, at(500)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order)
         VALUES (?1, ?2, 'User', 'go', ?3, 1)",
        params![format!("m-{id}"), id, at(500)],
    )
    .unwrap();
}

fn seed_job(
    conn: &Connection,
    id: &str,
    discussion: &str,
    status: &str,
    claimed_minutes_ago: i64,
    progress_minutes_ago: Option<i64>,
) {
    conn.execute(
        "INSERT INTO agent_dispatch_jobs
           (id, discussion_id, trigger_message_id, trigger_sort_order, dedupe_key,
            chain_prompt_ids_json, status, available_at, claimed_at, last_progress_at,
            created_at, updated_at)
         VALUES (?1, ?2, ?3, 1, ?4, '[]', ?5, ?6, ?6, ?7, ?6, ?6)",
        params![
            id,
            discussion,
            format!("m-{discussion}"),
            format!("dedupe-{id}"),
            status,
            at(claimed_minutes_ago),
            progress_minutes_ago.map(at),
        ],
    )
    .unwrap();
}

fn seed_execution(conn: &Connection, id: &str, number: i64, status: &str, sub: Option<&str>) {
    seed_discussion(conn, &format!("parent-{id}"), "principal", Some("p1"));
    conn.execute(
        "INSERT INTO orchestration_runs (id, discussion_id, project_id, created_at, updated_at)
         VALUES (?1, ?2, 'p1', ?3, ?3)",
        params![format!("run-{id}"), format!("parent-{id}"), at(60)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO planning_tasks (id, task_number, title, created_at, updated_at)
         VALUES (?1, ?2, 'Ship the bell', ?3, ?3)",
        params![format!("task-{id}"), number, at(60)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO task_executions
           (id, orchestration_run_id, task_id, parent_discussion_id, sub_discussion_id,
            status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![
            id,
            format!("run-{id}"),
            format!("task-{id}"),
            format!("parent-{id}"),
            sub,
            status,
            at(45),
        ],
    )
    .unwrap();
}

fn seed_run(conn: &Connection, id: &str, status: &str, parent: Option<&str>) {
    let mut run: crate::models::WorkflowRun = serde_json::from_value(serde_json::json!({
        "id": id, "workflow_id": "wf-1", "status": status,
        "trigger_context": null, "step_results": [], "tokens_used": 0,
        "workspace_path": null, "started_at": at(30),
        "finished_at": null, "run_type": "linear", "batch_total": 0,
        "batch_completed": 0, "batch_failed": 0, "batch_no_response": 0,
        "batch_name": null, "parent_run_id": parent, "state": {},
        "produced_branches": [], "concurrency_key": null, "project_id": "p1"
    }))
    .unwrap();
    if status == "WaitingQuota" {
        run.step_results = vec![serde_json::from_value(serde_json::json!({
            "step_name": "Review", "status": "WaitingQuota", "output": "", "duration_ms": 0,
            "started_at": at(20),
            "quota_wait": {
                "id": "w1", "reset_at": "2026-11-10T14:00:00Z",
                "wake_at": "2026-11-10T14:05:00Z", "attempt": 1
            }
        }))
        .unwrap()];
    }
    crate::db::workflows::insert_run(conn, &run).unwrap();
}

fn seed_workflow(conn: &Connection) {
    crate::db::resource_identities::tests::seed_workflow(
        conn,
        "wf-1",
        "Nightly review",
        Some("p1"),
    );
}

fn audit(project: &str, phase: &str, last_tool_minutes_ago: Option<i64>) -> AuditProgress {
    AuditProgress {
        project_id: project.into(),
        phase: phase.into(),
        step_index: 3,
        total_steps: 10,
        current_file: Some("repo-map.md".into()),
        started_at: now() - Duration::minutes(12),
        kind: "full".into(),
        step_tokens: None,
        total_tokens_so_far: None,
        current_tool: None,
        current_tool_call_count: None,
        agent: None,
        tier: None,
        connection_id: None,
        recent_activity: last_tool_minutes_ago.map(|minutes| AuditRecentActivity {
            entries: vec![AuditActivityEntry {
                category: crate::models::ActivityCategory::Read,
                at: now() - Duration::minutes(minutes),
            }],
        }),
        step_breakdown: None,
    }
}

#[test]
fn an_empty_instance_has_no_rows_and_every_source_answers() {
    let conn = db();
    let overview = overview(&conn, &[]);
    assert!(overview.rows.is_empty());
    assert!(overview.unavailable_sources.is_empty());
    assert_eq!(overview.stall_after_secs, STALL_AFTER_SECS);
}

#[test]
fn a_running_agent_turn_is_a_row_with_its_stop_action_and_a_link() {
    let conn = db();
    seed_discussion(&conn, "d1", "Fix the login", Some("p1"));
    seed_job(&conn, "j1", "d1", "Running", 4, Some(1));
    // Queued and finished turns are not "running".
    seed_discussion(&conn, "d2", "Queued", None);
    seed_job(&conn, "j2", "d2", "Pending", 4, None);
    seed_discussion(&conn, "d3", "Finished", None);
    seed_job(&conn, "j3", "d3", "Completed", 4, None);

    let overview = overview(&conn, &[]);
    assert_eq!(overview.rows.len(), 1, "{:#?}", overview.rows);
    let turn = row(&overview, "agent_dispatch_jobs:j1");
    assert_eq!(turn.row_type, "agent_turn");
    assert_eq!(turn.compartment, ActivityCompartment::Running);
    assert_eq!(turn.state, ActivityState::Running);
    assert_eq!(turn.source, "agent_dispatch_jobs");
    assert_eq!(turn.title, "Fix the login");
    assert_eq!(turn.project_name.as_deref(), Some("Kronn"));
    assert_eq!(turn.started_at, now() - Duration::minutes(4));
    assert!(!turn.stalled);
    let link = turn.link.as_ref().unwrap();
    assert_eq!(
        (link.target, link.id.as_str()),
        (ActivityLinkTarget::Discussion, "d1")
    );
    assert_eq!(turn.actions.len(), 1);
    assert_eq!(turn.actions[0].kind, ActivityActionKind::Stop);
    assert_eq!(turn.actions[0].args["dispatch_id"], "j1");
    assert_eq!(turn.actions[0].args["discussion_id"], "d1");
}

#[test]
fn silence_past_the_threshold_is_stalled_but_no_signal_at_all_is_not() {
    let conn = db();
    seed_discussion(&conn, "quiet", "Quiet", None);
    seed_job(&conn, "j-quiet", "quiet", "Running", 40, Some(16));
    seed_discussion(&conn, "fresh", "Fresh", None);
    seed_job(&conn, "j-fresh", "fresh", "Running", 40, Some(14));
    seed_discussion(&conn, "blind", "Blind", None);
    seed_job(&conn, "j-blind", "blind", "Running", 40, None);

    let overview = overview(&conn, &[]);
    assert!(row(&overview, "agent_dispatch_jobs:j-quiet").stalled);
    assert!(!row(&overview, "agent_dispatch_jobs:j-fresh").stalled);
    let blind = row(&overview, "agent_dispatch_jobs:j-blind");
    assert!(blind.last_activity_at.is_none());
    assert!(!blind.stalled, "no continuous signal is not a stall");
}

#[test]
fn a_quota_paused_run_is_paused_with_its_wake_time_and_resumes_through_its_parent() {
    let conn = db();
    seed_workflow(&conn);
    seed_run(&conn, "run-running", "Running", None);
    seed_run(&conn, "run-paused", "WaitingQuota", None);
    // Left out: a child shows through its parent, an approval is a decision,
    // a finished run is history.
    seed_run(&conn, "run-child", "Running", Some("run-running"));
    seed_run(&conn, "run-approval", "WaitingApproval", None);
    seed_run(&conn, "run-done", "Success", None);

    let overview = overview(&conn, &[]);
    let ids: Vec<_> = overview.rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(
        ids.len(),
        2,
        "only the top-level running and paused runs: {ids:?}"
    );

    let running = row(&overview, "workflow_runs:run-running");
    assert_eq!(running.state, ActivityState::Running);
    assert_eq!(running.title, "Nightly review");
    assert_eq!(running.project_name.as_deref(), Some("Kronn"));
    assert_eq!(running.actions[0].id, "cancel_workflow_run");
    assert_eq!(running.actions[0].args["workflow_id"], "wf-1");
    assert_eq!(running.actions[0].args["run_id"], "run-running");
    let link = running.link.as_ref().unwrap();
    assert_eq!(link.run_id.as_deref(), Some("run-running"));

    let paused = row(&overview, "workflow_runs:run-paused");
    assert_eq!(paused.state, ActivityState::Paused);
    assert_eq!(
        paused.wake_at.map(|at| at.to_rfc3339()),
        Some("2026-11-10T14:05:00+00:00".to_string())
    );
    assert_eq!(paused.detail.as_deref(), Some("Review"));
    assert_eq!(paused.actions.len(), 1);
    assert_eq!(paused.actions[0].kind, ActivityActionKind::ResumeNow);
    assert_eq!(paused.actions[0].args["run_id"], "run-paused");
    assert!(!paused.stalled, "a paused run is waiting, not stuck");
}

#[test]
fn a_task_worker_is_queued_while_provisioning_and_gone_once_a_person_is_needed() {
    let conn = db();
    seed_execution(&conn, "e-warm", 11, "Provisioning", None);
    seed_execution(&conn, "e-work", 12, "Working", None);
    seed_execution(&conn, "e-review", 13, "AwaitingReview", None);
    seed_execution(&conn, "e-done", 14, "Done", None);

    let overview = overview(&conn, &[]);
    assert_eq!(overview.rows.len(), 2, "{:#?}", overview.rows);
    assert_eq!(
        row(&overview, "task_executions:e-warm").state,
        ActivityState::Queued
    );
    let working = row(&overview, "task_executions:e-work");
    assert_eq!(working.state, ActivityState::Running);
    assert_eq!(working.row_type, "task_worker");
    assert_eq!(working.title, "KT-12 · Ship the bell");
    assert_eq!(working.project_name.as_deref(), Some("Kronn"));
    assert_eq!(working.actions[0].kind, ActivityActionKind::Cancel);
    assert_eq!(working.actions[0].args["execution_id"], "e-work");
    assert_eq!(
        working.link.as_ref().unwrap().target,
        ActivityLinkTarget::PlanningTask
    );
}

#[test]
fn a_task_workers_own_turn_is_not_listed_twice() {
    let conn = db();
    seed_discussion(&conn, "worker-disc", "worker", Some("p1"));
    seed_job(&conn, "j-worker", "worker-disc", "Running", 10, Some(1));
    seed_execution(&conn, "e1", 20, "Working", Some("worker-disc"));
    // The principal's own turn in its discussion is an ordinary agent turn.
    seed_job(&conn, "j-principal", "parent-e1", "Running", 3, Some(1));

    let overview = overview(&conn, &[]);
    let ids: Vec<_> = overview.rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(
        ids,
        ["task_executions:e1", "agent_dispatch_jobs:j-principal"],
        "{ids:?}"
    );
}

#[test]
fn an_audit_row_comes_from_the_tracker_and_the_done_entry_is_ignored() {
    let conn = db();
    let audits = [audit("p1", "auditing", Some(2)), audit("p2", "done", None)];
    let overview = overview(&conn, &audits);
    assert_eq!(overview.rows.len(), 1);
    let audit_row = row(&overview, "audit_tracker:p1");
    assert_eq!(audit_row.row_type, "audit");
    assert_eq!(audit_row.title, "Kronn");
    assert_eq!(audit_row.detail.as_deref(), Some("3/10 · repo-map.md"));
    assert_eq!(
        audit_row.last_activity_at,
        Some(now() - Duration::minutes(2))
    );
    assert!(!audit_row.stalled);
    assert_eq!(audit_row.actions[0].id, "cancel_audit");
    assert_eq!(audit_row.actions[0].args["project_id"], "p1");

    let silent = [audit("p1", "auditing", Some(30))];
    assert!(row(&self::overview(&conn, &silent), "audit_tracker:p1").stalled);
}

/// DoD 1 — a new kind of row is one new source: the fold, the stalled rule and
/// the ordering apply to it with no change anywhere else.
struct InjectedSource {
    fail: bool,
}

impl ActivitySource for InjectedSource {
    fn name(&self) -> &'static str {
        "injected_jobs"
    }

    fn collect(&self, ctx: &ActivityContext<'_>) -> Result<Vec<ActivityRow>> {
        if self.fail {
            anyhow::bail!("the source is down");
        }
        Ok(vec![running_row(
            "injected_jobs",
            "42",
            "media_job",
            ActivityState::Running,
            "Render the clip".into(),
            ctx.now - Duration::minutes(90),
            None,
            Vec::new(),
        )])
    }
}

#[test]
fn a_new_row_type_is_one_new_source_and_nothing_else() {
    let conn = db();
    seed_discussion(&conn, "d1", "Fix the login", None);
    seed_job(&conn, "j1", "d1", "Running", 4, Some(1));

    let ctx = ActivityContext {
        conn: &conn,
        audits: &[],
        now: now(),
    };
    let mut sources = default_sources();
    sources.push(Box::new(InjectedSource { fail: false }));
    let overview = collect_activity(&ctx, &sources);

    let injected = row(&overview, "injected_jobs:42");
    assert_eq!(injected.row_type, "media_job");
    assert_eq!(injected.source, "injected_jobs");
    assert_eq!(injected.compartment, ActivityCompartment::Running);
    // Oldest first, across sources.
    assert_eq!(overview.rows[0].id, "injected_jobs:42");
    assert_eq!(overview.rows.len(), 2);
}

#[test]
fn a_source_that_fails_is_reported_and_leaves_the_others_intact() {
    let conn = db();
    seed_discussion(&conn, "d1", "Fix the login", None);
    seed_job(&conn, "j1", "d1", "Running", 4, Some(1));

    let ctx = ActivityContext {
        conn: &conn,
        audits: &[],
        now: now(),
    };
    let mut sources = default_sources();
    sources.push(Box::new(InjectedSource { fail: true }));
    let overview = collect_activity(&ctx, &sources);

    assert_eq!(
        overview.unavailable_sources,
        vec!["injected_jobs".to_string()]
    );
    assert_eq!(overview.rows.len(), 1);
}

#[test]
fn the_row_model_serializes_the_type_under_its_own_name() {
    let conn = db();
    seed_discussion(&conn, "d1", "Fix the login", None);
    seed_job(&conn, "j1", "d1", "Running", 4, Some(1));
    let json = serde_json::to_value(overview(&conn, &[])).unwrap();
    let first = &json["rows"][0];
    assert_eq!(first["type"], "agent_turn");
    assert_eq!(first["compartment"], "running");
    assert_eq!(first["state"], "running");
    assert_eq!(first["actions"][0]["kind"], "stop");
}
