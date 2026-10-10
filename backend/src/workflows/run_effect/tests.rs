use super::*;
use crate::models::{StepType, WorkflowStep};
use crate::workflows::step_output_format::format_step_output_simple;
use crate::workflows::template::TemplateContext;
use serde_json::json;

fn step(kind: &str, output: &str, tokens: Option<u64>) -> StepResult {
    serde_json::from_value(json!({
        "step_name": format!("{kind}-step"),
        "status": "Success",
        "output": output,
        "tokens_used": tokens,
        "duration_ms": 1,
        "step_kind": kind,
    }))
    .unwrap()
}

fn run(id: &str, steps: Vec<StepResult>) -> WorkflowRun {
    let mut run: WorkflowRun = serde_json::from_value(json!({
        "id": id,
        "workflow_id": "wf",
        "status": "Success",
        "trigger_context": null,
        "step_results": [],
        "tokens_used": 0,
        "workspace_path": null,
        "started_at": "2026-10-01T00:00:00Z",
        "finished_at": "2026-10-01T00:01:00Z",
    }))
    .unwrap();
    run.step_results = steps;
    run
}

fn json_data() -> StepResult {
    step(
        "JsonData",
        &format_step_output_simple(json!({"a": 1}), "OK", "JSON data"),
        Some(0),
    )
}

fn api_call(method: &str) -> StepResult {
    let summary = format!("{method} https://api.example.test/items → 3 items");
    step(
        "ApiCall",
        &format_step_output_simple(json!([1, 2, 3]), "OK", &summary),
        Some(0),
    )
}

fn agent(envelope: serde_json::Value, tokens: Option<u64>) -> StepResult {
    let output =
        format!("J'ai vérifié la file 🚀.\n\n---STEP_OUTPUT---\n{envelope}\n---END_STEP_OUTPUT---");
    step("Agent", &output, tokens)
}

fn exec_step(args: &[&str]) -> WorkflowStep {
    WorkflowStep {
        name: "lit".into(),
        step_type: StepType::Exec,
        exec_command: Some("printf".into()),
        exec_args: args.iter().map(|arg| arg.to_string()).collect(),
        exec_timeout_secs: Some(10),
        ..WorkflowStep::default()
    }
}

/// Run a real Exec step and stamp it as the runner does.
async fn exec_result(args: &[&str]) -> StepResult {
    let step = exec_step(args);
    let mut outcome = crate::workflows::exec_step::execute_exec_step(
        &step,
        &["printf".to_string()],
        "/tmp",
        &TemplateContext::new(),
    )
    .await;
    crate::workflows::runner::apply_step_snapshot(&step, &mut outcome.result, None);
    outcome.result
}

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn.execute(
        "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
         VALUES ('wf', 'wf', '\"Manual\"', '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn
}

fn insert(conn: &Connection, run: &WorkflowRun) {
    crate::db::workflows::insert_run(conn, run).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn an_exec_step_declares_no_op_with_its_marker_line() {
    let declared = exec_result(&["{\"items\": []}\nKRONN_NOOP\n"]).await;
    assert_eq!(
        classify_steps(&run("r", vec![declared])),
        WorkflowRunOutcome::NoOp
    );

    let silent = exec_result(&["{\"items\": []}\n"]).await;
    assert_eq!(
        classify_steps(&run("r", vec![silent])),
        WorkflowRunOutcome::Changed
    );

    // The marker must be a line of its own, not a word in the output.
    let quoted = exec_result(&["no KRONN_NOOP here\n"]).await;
    assert_eq!(
        classify_steps(&run("r", vec![quoted])),
        WorkflowRunOutcome::Changed
    );
}

#[test]
fn an_agent_is_no_op_only_when_its_envelope_says_so() {
    let declared = agent(
        json!({"data": [], "status": "OK", "summary": "rien", "no_change": true}),
        Some(1200),
    );
    assert_eq!(
        classify_steps(&run("r", vec![declared])),
        WorkflowRunOutcome::NoOp
    );

    for envelope in [
        json!({"data": [], "status": "NO_RESULTS", "summary": "rien"}),
        json!({"data": [], "status": "OK", "summary": "rien", "no_change": "true"}),
        json!({"data": [], "status": "OK", "summary": "rien", "no_change": false}),
    ] {
        let spent = agent(envelope.clone(), Some(1200));
        assert_eq!(
            classify_steps(&run("r", vec![spent])),
            WorkflowRunOutcome::Changed,
            "{envelope}"
        );
    }
    let free_text = step("Agent", "Rien à faire.", Some(300));
    assert_eq!(
        classify_steps(&run("r", vec![free_text])),
        WorkflowRunOutcome::Changed
    );
}

#[test]
fn only_a_get_or_head_call_is_read_only() {
    for method in ["GET", "HEAD"] {
        assert_eq!(
            classify_steps(&run("r", vec![api_call(method)])),
            WorkflowRunOutcome::NoOp,
            "{method}"
        );
    }
    for method in ["POST", "PUT", "PATCH", "DELETE", "get"] {
        assert_eq!(
            classify_steps(&run("r", vec![json_data(), api_call(method)])),
            WorkflowRunOutcome::Changed,
            "{method}"
        );
    }
}

#[test]
fn a_collect_step_is_no_op_only_when_every_source_is_a_read() {
    let collect = |sources: serde_json::Value| {
        step(
            "CollectApiData",
            &format_step_output_simple(
                json!({"sources": {}, "meta": {"sources": sources}}),
                "OK",
                "Collected",
            ),
            Some(0),
        )
    };
    let read =
        json!({"kind": "quick_api", "summary": "GET https://x.test → object", "error": null});
    let cli = json!({"kind": "quick_exec", "summary": "exit 0 — 3 ms", "error": null});
    let post =
        json!({"kind": "quick_api", "summary": "POST https://x.test → object", "error": null});
    assert_eq!(
        classify_steps(&run("r", vec![collect(json!([read.clone()]))])),
        WorkflowRunOutcome::NoOp
    );
    for sources in [json!([read.clone(), cli]), json!([post]), json!([])] {
        assert_eq!(
            classify_steps(&run("r", vec![collect(sources.clone())])),
            WorkflowRunOutcome::Changed,
            "{sources}"
        );
    }
}

#[test]
fn any_doubt_reads_as_changed() {
    let mut failed = json_data();
    failed.status = RunStatus::Failed;
    let mut rollback = json_data();
    rollback.is_rollback = true;
    let mut child = json_data();
    child.child_run_id = Some("child".into());
    let mut unknown_kind = json_data();
    unknown_kind.step_kind = None;
    // KT-1043 — a terminal stop fails its step; even a stray marker is no proof.
    let mut stopped = json_data();
    stopped.terminal_stop = Some("safety limit".into());
    let mut unknown_tokens = json_data();
    unknown_tokens.tokens_used = None;
    let notify = step(
        "Notify",
        &format_step_output_simple(json!({}), "OK", "sent"),
        Some(0),
    );
    for (label, steps) in [
        ("failed step", vec![failed]),
        ("rollback", vec![rollback]),
        ("child run", vec![child]),
        ("no kind", vec![unknown_kind]),
        ("unknown tokens", vec![unknown_tokens]),
        ("terminal stop", vec![stopped]),
        ("notify", vec![notify]),
        ("no step", vec![]),
    ] {
        assert_eq!(
            classify_steps(&run("r", steps)),
            WorkflowRunOutcome::Changed,
            "{label}"
        );
    }

    let mut worktree = run("r", vec![json_data()]);
    worktree.workspace_path = Some("/repo/.kronn/worktrees/r".into());
    assert_eq!(classify_steps(&worktree), WorkflowRunOutcome::Changed);
    let mut branch = run("r", vec![json_data()]);
    branch
        .produced_branches
        .push(crate::models::ProducedBranch {
            branch_name: "kronn/r".into(),
            head_sha: "abc".into(),
            ahead: 1,
            pushed_upstream: false,
        });
    assert_eq!(classify_steps(&branch), WorkflowRunOutcome::Changed);
    let mut failed_run = run("r", vec![json_data()]);
    failed_run.status = RunStatus::Partial;
    assert_eq!(classify_steps(&failed_run), WorkflowRunOutcome::Changed);
}

fn page(conn: &Connection) {
    conn.execute(
        "INSERT INTO live_pages(id, title, slug, created_at, updated_at)
         VALUES ('page', 'Page', 'page', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    crate::db::live_pages::add_live_page_dataset(
        conn,
        "page",
        &crate::models::CreateLivePageDataset {
            name: "etat".into(),
            kind: crate::models::LivePageDatasetKind::Snapshot,
            initial: Some(json!({"v": 1})),
            schema: None,
            max_points: None,
            max_age_days: None,
        },
    )
    .unwrap();
}

fn publish(conn: &Connection, run_id: &str, value: serde_json::Value) -> serde_json::Value {
    let result = crate::db::live_pages::publish_live_page(
        conn,
        "page",
        &crate::models::PublishLivePageRequest {
            workflow_id: Some("wf".into()),
            workflow_run_id: Some(run_id.into()),
            writes: vec![crate::models::LivePageWrite {
                dataset: "etat".into(),
                operation: crate::models::LivePageWriteOperation::Replace,
                value,
                observed_at: None,
                dedupe_key: None,
                key_field: None,
            }],
        },
    )
    .unwrap();
    serde_json::to_value(result).unwrap()
}

#[test]
fn a_changed_dataset_write_is_never_no_op_and_an_identical_one_is() {
    let conn = db();
    page(&conn);
    let changed_run = run("changed", vec![]);
    let same_run = run("same", vec![]);
    insert(&conn, &changed_run);
    insert(&conn, &same_run);

    let changed = publish(&conn, "changed", json!({"v": 2}));
    let same = publish(&conn, "same", json!({"v": 2}));
    let publish_step = |payload: serde_json::Value| {
        step(
            "PublishPageData",
            &format_step_output_simple(payload, "OK", "published"),
            Some(0),
        )
    };

    let changed_run = run("changed", vec![publish_step(changed.clone())]);
    assert_eq!(classify_steps(&changed_run), WorkflowRunOutcome::Changed);
    assert_eq!(
        classify(&conn, &changed_run).unwrap(),
        WorkflowRunOutcome::Changed
    );
    let same_run = run("same", vec![publish_step(same)]);
    assert_eq!(
        classify(&conn, &same_run).unwrap(),
        WorkflowRunOutcome::NoOp
    );

    // A step that claims nothing changed is overruled by the recorded write.
    let liar = run("changed", vec![json_data()]);
    assert_eq!(classify_steps(&liar), WorkflowRunOutcome::NoOp);
    assert_eq!(classify(&conn, &liar).unwrap(), WorkflowRunOutcome::Changed);
}

#[test]
fn a_recorded_non_get_call_or_reference_makes_the_run_changed() {
    let conn = db();
    for id in ["get", "post", "discussion"] {
        insert(&conn, &run(id, vec![]));
    }
    let log = |run_id: &str, method: &str| {
        conn.execute(
            "INSERT INTO api_call_logs (id, source, run_id, plugin_slug, endpoint_path, method,
                 status, duration_ms)
             VALUES (?1, 'agent_broker', ?2, 'api-x', '/items', ?3, 'OK', 1)",
            params![format!("log-{run_id}"), run_id, method],
        )
        .unwrap();
    };
    log("get", "get");
    log("post", "POST");
    conn.execute(
        "INSERT INTO discussions (id, title, agent, language, created_at, updated_at, workflow_run_id)
         VALUES ('d', 'd', 'ClaudeCode', 'fr', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'discussion')",
        [],
    )
    .unwrap();

    assert!(!has_recorded_effect(&conn, "get").unwrap());
    assert!(has_recorded_effect(&conn, "post").unwrap());
    assert!(has_recorded_effect(&conn, "discussion").unwrap());
}

fn stored(conn: &Connection, id: &str) -> (Option<String>, String) {
    conn.query_row(
        "SELECT outcome, step_results_json FROM workflow_runs WHERE id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

#[test]
fn a_no_op_run_keeps_a_short_head_of_each_output_once() {
    let conn = db();
    let long = "é".repeat(5_000);
    let mut steps = vec![json_data(), json_data()];
    steps[0].output = long.clone();
    steps[1].output = "court".into();
    let no_op = run("noop", steps.clone());
    insert(&conn, &no_op);

    assert!(store(&conn, "noop", WorkflowRunOutcome::NoOp).unwrap());
    let (outcome, json) = stored(&conn, "noop");
    assert_eq!(outcome.as_deref(), Some("no_op"));
    let kept: Vec<StepResult> = serde_json::from_str(&json).unwrap();
    assert_eq!(kept.len(), 2);
    let head: String = long.chars().take(NO_OP_OUTPUT_CHARS as usize).collect();
    assert_eq!(kept[0].output, format!("{head}{NO_OP_OUTPUT_SUFFIX}"));
    assert_eq!(kept[1].output, "court");
    assert!(
        !store(&conn, "noop", WorkflowRunOutcome::Changed).unwrap(),
        "an outcome is set once"
    );

    let changed = run("changed", steps);
    insert(&conn, &changed);
    assert!(store(&conn, "changed", WorkflowRunOutcome::Changed).unwrap());
    let (outcome, json) = stored(&conn, "changed");
    assert_eq!(outcome.as_deref(), Some("changed"));
    assert!(
        json.contains(&long),
        "a run with an effect keeps every output"
    );

    let mut failed = run("failed", vec![json_data()]);
    failed.status = RunStatus::Failed;
    insert(&conn, &failed);
    assert!(!store(&conn, "failed", WorkflowRunOutcome::NoOp).unwrap());
    assert_eq!(stored(&conn, "failed").0, None);
}

#[test]
fn only_top_level_linear_successes_are_candidates() {
    assert!(is_candidate(&run("r", vec![json_data()])));
    let mut child = run("r", vec![json_data()]);
    child.parent_run_id = Some("parent".into());
    let mut batch = run("r", vec![json_data()]);
    batch.run_type = "batch".into();
    let mut done = run("r", vec![json_data()]);
    done.outcome = Some(WorkflowRunOutcome::Changed);
    for other in [child, batch, done] {
        assert!(!is_candidate(&other));
    }
}

#[test]
fn the_run_list_can_leave_no_op_runs_out() {
    let conn = db();
    let mut no_op = run("noop", vec![json_data()]);
    no_op.outcome = Some(WorkflowRunOutcome::NoOp);
    let mut changed = run("changed", vec![json_data()]);
    changed.outcome = Some(WorkflowRunOutcome::Changed);
    let legacy = run("legacy", vec![json_data()]);
    for row in [&no_op, &changed, &legacy] {
        insert(&conn, row);
        if let Some(outcome) = row.outcome {
            store(&conn, &row.id, outcome).unwrap();
        }
    }
    let ids = |hide| {
        let mut ids: Vec<String> =
            crate::db::workflows::list_runs_paginated_visible(&conn, "wf", None, None, None, hide)
                .unwrap()
                .into_iter()
                .map(|run| run.id)
                .collect();
        ids.sort();
        ids
    };
    assert_eq!(ids(true), ["changed", "legacy"]);
    assert_eq!(ids(false), ["changed", "legacy", "noop"]);
    assert_eq!(
        crate::db::workflows::count_runs_filtered(&conn, "wf", true).unwrap(),
        2
    );
    assert_eq!(crate::db::workflows::count_runs(&conn, "wf").unwrap(), 3);
    let listed = crate::db::workflows::get_run(&conn, "noop")
        .unwrap()
        .unwrap();
    assert_eq!(
        listed.outcome,
        Some(WorkflowRunOutcome::NoOp),
        "the API returns it"
    );
}

async fn task_board_result(operation: crate::models::TaskBoardOperation) -> StepResult {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    let state = AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
    let step = WorkflowStep {
        name: "board".into(),
        step_type: StepType::TaskBoard,
        task_board: Some(crate::models::TaskBoardConfig {
            tag: "todo".into(),
            operation,
            title: "A card".into(),
            ..Default::default()
        }),
        ..WorkflowStep::default()
    };
    let mut outcome = crate::workflows::task_board_step::execute_task_board_step(
        &step,
        "wf",
        &state,
        &TemplateContext::new(),
    )
    .await;
    assert_eq!(
        outcome.result.status,
        RunStatus::Success,
        "{}",
        outcome.result.output
    );
    crate::workflows::runner::apply_step_snapshot(&step, &mut outcome.result, None);
    outcome.result
}

#[tokio::test]
async fn a_task_board_read_is_neutral_and_a_write_is_a_change() {
    use crate::models::TaskBoardOperation;
    let read = task_board_result(TaskBoardOperation::Read).await;
    assert_eq!(read.step_kind.as_deref(), Some("TaskBoard"));
    assert_eq!(
        classify_steps(&run("r-read", vec![read])),
        WorkflowRunOutcome::NoOp
    );
    let add = task_board_result(TaskBoardOperation::Add).await;
    assert_eq!(
        classify_steps(&run("r-add", vec![json_data(), add])),
        WorkflowRunOutcome::Changed
    );
}

#[test]
fn a_delegate_subtasks_step_is_never_neutral() {
    let delegated = step(
        "DelegateSubtasks",
        &format_step_output_simple(json!({"subtasks": []}), "OK", "nothing to delegate"),
        Some(0),
    );
    assert_eq!(
        classify_steps(&run("r-delegate", vec![delegated])),
        WorkflowRunOutcome::Changed
    );
}
