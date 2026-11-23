use super::*;

// ─── a project prepares its own worktrees ──────────────────

fn hooks(after_create: Option<&str>, before_run: Option<&str>) -> crate::models::WorkspaceHooks {
    crate::models::WorkspaceHooks {
        after_create: after_create.map(str::to_string),
        before_run: before_run.map(str::to_string),
        after_run: None,
        before_remove: None,
    }
}

// ─── next_step_index_for_resume — Goto-loop bug fix (0.7.0) ─────────

fn fake_step(name: &str) -> crate::models::WorkflowStep {
    crate::models::WorkflowStep {
        exec_approved_by_trust: None,
        id: None,
        name: name.into(),
        step_type: crate::models::StepType::Agent,
        description: None,
        agent: crate::models::AgentType::ClaudeCode,
        prompt_template: String::new(),
        mode: crate::models::StepMode::Normal,
        output_format: crate::models::StepOutputFormat::FreeText,
        on_result: vec![],
        on_timeout: None,
        agent_settings: None,
        stall_timeout_secs: None,
        retry: None,
        delay_after_secs: None,
        mcp_config_ids: vec![],
        skill_ids: vec![],
        external_effect: false,
        shadows: Vec::new(),
        shadow_merge: None,
        profile_ids: vec![],
        directive_ids: vec![],
        batch_quick_prompt_id: None,
        batch_items_from: None,
        batch_wait_for_completion: None,
        batch_max_items: None,
        batch_workspace_mode: None,
        batch_chain_prompt_ids: vec![],
        batch_concurrent_limit: None,
        quick_api_id: None,
        notify_config: None,
        api_plugin_slug: None,
        api_config_id: None,
        api_endpoint_path: None,
        api_method: None,
        api_query: None,
        api_path_params: None,
        api_headers: None,
        api_body: None,
        api_extract: None,
        api_pagination: None,
        api_timeout_ms: None,
        api_max_retries: None,
        api_output_var: None,
        api_response: None,
        gate_message: None,
        gate_request_changes_target: None,
        gate_notify_url: None,
        gate_checkpoint_before: None,
        gate_auto_approve_after_secs: None,
        exec_command: None,
        exec_args: vec![],
        exec_timeout_secs: None,
        exec_setup_command: None,
        exec_setup_args: vec![],
        exec_stdin: None,
        quick_prompt_id: None,
        quick_prompt_version: None,
        quick_prompt_variables: std::collections::HashMap::new(),
        json_data_payload: None,
        collect_api_data: None,
        transform_data: None,
        page_publish: None,
        task_board: None,
        decide: None,
        sub_workflow_id: None,
        sub_workflow_foreach_file: None,
        multi_agent_review: None,
        room_id: None,
        read_only_repos: vec![],
        delegate_subtasks: None,
        exec_script_files: vec![],
        exec_unmodelled_args_approved: None,
        exec_agent_written: None,
        exec_agent_lines: vec![],
        sub_workflow_variables: std::collections::HashMap::new(),
    }
}

fn fake_result(name: &str) -> crate::models::StepResult {
    crate::models::StepResult {
        step_name: name.into(),
        status: crate::models::RunStatus::Success,
        output: String::new(),
        tokens_used: Some(0),
        duration_ms: 0,
        started_at: None,
        condition_result: None,
        envelope_detected: None,
        step_kind: None,
        step_agent: None,
        step_model: None,
        step_api_plugin_slug: None,
        step_api_endpoint_path: None,
        is_rollback: false,
        child_run_id: None,
        agent_provenance: None,
        native_tool_calls: Box::default(),
        cached_prompt_tokens: None,
        cache_write_prompt_tokens: None,
        last_activity: None,
        quota_wait: None,
        terminal_stop: None,
        output_bytes: None,
    }
}

// ─── Artifacts persistence (0.7.0 Phase 3) ────────────────────────────────

fn make_workflow_with_artifacts(
    artifacts: ::std::collections::HashMap<String, ArtifactSpec>,
) -> Workflow {
    Workflow {
        refs: Default::default(),
        retention: None,
        project_scope: None,
        pinned: false,
        id: "test".into(),
        name: "test".into(),
        project_id: None,
        trigger: WorkflowTrigger::Manual,
        steps: vec![],
        actions: vec![],
        safety: WorkflowSafety {
            sandbox: false,
            max_files: None,
            max_lines: None,
            require_approval: false,
        },
        workspace_config: None,
        concurrency_limit: None,
        concurrency_key: None,
        guards: None,
        artifacts,
        on_failure: vec![],
        exec_allowlist: vec![],
        variables: vec![],
        enabled: true,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

// ─── execute_run end-to-end (orchestrator drive + DB persist) ──────────
//
// P0-1 (test-quality audit, 2026-06-17): the runner's *pure* helpers are
// densely unit-tested (resume index, budget, artifact persist, gate
// feedback) but NOTHING drove the real `execute_run` orchestrator loop:
// status transitions, the per-step dispatch, and the DB write-back. The
// Agent path needs a live CLI (no injection seam above
// `run_agent_with_timeout`), but the DETERMINISTIC step types don't —
// `JsonData` emits a literal payload with zero tokens / network /
// subprocess. A project-less workflow skips worktree creation entirely
// (`project_path` empty → no `Workspace::create`), so this is a clean,
// hermetic drive of the whole loop.

fn json_data_step(name: &str, payload: serde_json::Value) -> crate::models::WorkflowStep {
    let mut s = fake_step(name);
    s.step_type = crate::models::StepType::JsonData;
    s.json_data_payload = Some(payload);
    s
}

fn notify_step(name: &str, url: String, body_template: &str) -> WorkflowStep {
    let mut step = fake_step(name);
    step.step_type = StepType::Notify;
    step.notify_config = Some(NotifyConfig {
        url,
        method: "POST".into(),
        headers: std::collections::HashMap::from([(
            "Content-Type".into(),
            "application/json".into(),
        )]),
        body_template: body_template.into(),
    });
    step
}

fn test_state_and_configs() -> (
    crate::AppState,
    crate::models::TokensConfig,
    crate::models::AgentsConfig,
) {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
    let cfg = crate::core::config::default_config();
    let tokens = cfg.tokens.clone();
    let agents = cfg.agents.clone();
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(cfg));
    let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
    (state, tokens, agents)
}

fn pending_run(id: &str, workflow_id: &str) -> WorkflowRun {
    WorkflowRun {
        outcome: None,
        id: id.into(),
        workflow_id: workflow_id.into(),
        status: RunStatus::Pending,
        trigger_context: None,
        step_results: vec![],
        tokens_used: 0,
        shadow_tokens_used: 0,
        workspace_path: None,
        started_at: chrono::Utc::now(),
        finished_at: None,
        run_type: "linear".into(),
        batch_total: 0,
        batch_completed: 0,
        batch_failed: 0,
        batch_no_response: 0,
        batch_name: None,
        parent_run_id: None,
        state: ::std::collections::HashMap::new(),
        produced_branches: vec![],
        concurrency_key: None,
        triggered_by_run_id: None,
        project_id: None,
        parent_workflow_id: None,
        parent_workflow_name: None,
        parent_run_started_at: None,
    }
}

/// A project on a real directory with one non-isolated workflow whose
/// only step sleeps, so two concurrent runs would visibly overlap.
async fn main_tree_fixture(
    state: &crate::AppState,
    suffix: &str,
    read_only: bool,
) -> (tempfile::TempDir, Workflow) {
    let repo = tempfile::TempDir::new().unwrap();
    let now = chrono::Utc::now();
    let project: Project = serde_json::from_value(serde_json::json!({
        "id": format!("proj-main-tree-{suffix}"), "name": "Main tree fixture",
        "path": repo.path().to_string_lossy(),
        "repo_url": null, "token_override": null,
        "ai_config": {"detected": false, "configs": []},
        "created_at": now.to_rfc3339(), "updated_at": now.to_rfc3339(),
    }))
    .unwrap();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = format!("wf-main-tree-{suffix}");
    workflow.project_id = Some(project.id.clone());
    workflow.workspace_config = read_only.then(|| WorkspaceConfig {
        hooks: WorkspaceHooks::default(),
        require_isolation: false,
        main_tree_read_only: true,
        base_ref: None,
    });
    workflow.exec_allowlist = vec!["sleep".into()];
    let mut pause = fake_step("pause");
    pause.step_type = StepType::Exec;
    pause.exec_command = Some("sleep".into());
    pause.exec_args = vec!["0.4".into()];
    workflow.steps = vec![pause];
    state
        .db
        .with_conn(move |conn| crate::db::projects::insert_project(conn, &project))
        .await
        .unwrap();
    (repo, workflow)
}

async fn git_in(cwd: &std::path::Path, args: &[&str]) -> String {
    let out = crate::core::cmd::async_git_cmd()
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn step_window(run: &WorkflowRun) -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
    let step = &run.step_results[0];
    let start = step.started_at.expect("step start recorded");
    (
        start,
        start + chrono::Duration::milliseconds(step.duration_ms as i64),
    )
}

#[cfg(unix)] // uses a POSIX FIFO; relies on POSIX process groups and signals
#[tokio::test]
async fn two_non_isolated_runs_of_one_project_started_together_both_succeed_in_turn() {
    let (state, tokens, agents) = test_state_and_configs();
    let (_repo, workflow) = main_tree_fixture(&state, "turns", false).await;
    let mut first = pending_run("run-main-tree-first", &workflow.id);
    let mut second = pending_run("run-main-tree-second", &workflow.id);
    insert_wf_and_run(&state, &workflow, &first).await;
    let second_db = second.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_run(conn, &second_db))
        .await
        .unwrap();

    let (a, b) = tokio::join!(
        execute_run(
            state.clone(),
            &workflow,
            &mut first,
            &tokens,
            &agents,
            None,
            None,
            None
        ),
        execute_run(
            state.clone(),
            &workflow,
            &mut second,
            &tokens,
            &agents,
            None,
            None,
            None
        ),
    );
    a.unwrap();
    b.unwrap();

    for run in [&first, &second] {
        assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
        assert_eq!(run.step_results.len(), 1, "no __workspace__ refusal");
    }
    let (first_start, first_end) = step_window(&first);
    let (second_start, second_end) = step_window(&second);
    assert!(
            first_end <= second_start || second_end <= first_start,
            "the runs took the checkout one after the other: {first_start}..{first_end} vs {second_start}..{second_end}"
        );
}

#[tokio::test]
async fn a_run_that_outwaits_the_main_tree_fails_naming_the_holder() {
    let (state, tokens, agents) = test_state_and_configs();
    let (repo, workflow) = main_tree_fixture(&state, "timeout", false).await;
    let holder = crate::workflows::workspace::MainTreeGuard::try_acquire(
        &repo.path().to_string_lossy(),
        "run-holding-the-tree",
    )
    .unwrap();
    let mut run = pending_run("run-main-tree-late", &workflow.id);
    insert_wf_and_run(&state, &workflow, &run).await;

    let started = std::time::Instant::now();
    crate::workflows::workspace::MAIN_TREE_WAIT_FOR_TESTS
        .scope(
            std::time::Duration::from_millis(200),
            execute_run(
                state.clone(),
                &workflow,
                &mut run,
                &tokens,
                &agents,
                None,
                None,
                None,
            ),
        )
        .await
        .unwrap();
    drop(holder);

    assert!(
        started.elapsed() >= std::time::Duration::from_millis(200),
        "it waited first"
    );
    assert_eq!(run.status, RunStatus::Failed);
    let refusal = &run.step_results[0];
    assert_eq!(refusal.step_name, "__workspace__");
    assert!(
        refusal.output.contains("run-holding-the-tree"),
        "the reason names the holder: {}",
        refusal.output
    );
    let persisted = state
        .db
        .with_conn(|conn| crate::db::workflows::get_run(conn, "run-main-tree-late"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.status, RunStatus::Failed);
    assert!(persisted.step_results[0]
        .output
        .contains("run-holding-the-tree"));
}

#[cfg(unix)] // its Exec step runs `sleep 0.4` as a program, which Windows lacks
#[tokio::test]
async fn a_workflow_declared_read_only_on_the_main_tree_skips_the_lock() {
    let (state, tokens, agents) = test_state_and_configs();
    let (repo, workflow) = main_tree_fixture(&state, "read-only", true).await;
    let tree = repo.path().to_string_lossy().to_string();
    let _holder =
        crate::workflows::workspace::MainTreeGuard::try_acquire(&tree, "run-writer").unwrap();
    let mut run = pending_run("run-main-tree-reader", &workflow.id);
    insert_wf_and_run(&state, &workflow, &run).await;

    crate::workflows::workspace::MAIN_TREE_WAIT_FOR_TESTS
        .scope(
            std::time::Duration::ZERO,
            execute_run(
                state.clone(),
                &workflow,
                &mut run,
                &tokens,
                &agents,
                None,
                None,
                None,
            ),
        )
        .await
        .unwrap();

    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert_eq!(
        crate::workflows::workspace::MainTreeGuard::try_acquire(&tree, "probe").err(),
        Some("run-writer".to_string()),
        "the writer kept the lock; the reader never took it"
    );
}

#[tokio::test]
async fn cancelling_a_run_that_waits_for_the_main_tree_stops_the_wait() {
    let (state, tokens, agents) = test_state_and_configs();
    let (repo, workflow) = main_tree_fixture(&state, "cancel", false).await;
    let _holder = crate::workflows::workspace::MainTreeGuard::try_acquire(
        &repo.path().to_string_lossy(),
        "run-holding-the-tree",
    )
    .unwrap();
    let mut run = pending_run("run-main-tree-cancelled", &workflow.id);
    insert_wf_and_run(&state, &workflow, &run).await;

    let canceller = {
        let state = state.clone();
        async move {
            // Wait for the runner to register its token, then cancel.
            for _ in 0..200 {
                let registered = state
                    .cancel_registry
                    .lock()
                    .unwrap()
                    .contains_key("run-main-tree-cancelled");
                if registered {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            crate::workflows::cancellation::cancel_run_tree(
                &state,
                "run-main-tree-cancelled",
                crate::workflows::cancellation::CancellationScope::RunTree,
                "cancelled_by_operator",
            )
            .await
            .unwrap();
        }
    };
    let started = std::time::Instant::now();
    let (result, ()) = tokio::join!(
        crate::workflows::workspace::MAIN_TREE_WAIT_FOR_TESTS.scope(
            std::time::Duration::from_secs(30),
            execute_run(
                state.clone(),
                &workflow,
                &mut run,
                &tokens,
                &agents,
                None,
                None,
                None,
            ),
        ),
        canceller,
    );
    result.unwrap();

    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "no full wait"
    );
    assert_eq!(run.status, RunStatus::Cancelled);
    let persisted = state
        .db
        .with_conn(|conn| crate::db::workflows::get_run(conn, "run-main-tree-cancelled"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.status, RunStatus::Cancelled);
}

async fn wait_until_settled(state: &crate::AppState, run_id: &str) -> WorkflowRun {
    for _ in 0..500 {
        let id = run_id.to_string();
        let run = state
            .db
            .with_conn(move |conn| crate::db::workflows::get_run(conn, &id))
            .await
            .unwrap()
            .expect("run row");
        if !matches!(run.status, RunStatus::Pending | RunStatus::Running) {
            return run;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("run {run_id} never left Running");
}

#[tokio::test]
async fn a_background_run_whose_execute_run_fails_ends_failed_with_the_reason() {
    // The KT-786 shape: variables declared, no snapshot prepared.
    let (state, _, _) = test_state_and_configs();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-snapshot-missing".into();
    workflow.variables = vec![PromptVariable {
        name: "ticket".into(),
        label: "Ticket".into(),
        placeholder: String::new(),
        description: None,
        required: true,
        pattern: None,
        source: Default::default(),
        source_ref: None,
        allow_manual_override: false,
        control: None,
    }];
    workflow.steps = vec![json_data_step("first", serde_json::json!({ "n": 1 }))];
    let run = pending_run("run-snapshot-missing", &workflow.id);
    let (wf_db, run_db) = (workflow.clone(), run.clone());
    state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::insert_workflow(conn, &wf_db)?;
            crate::db::workflows::insert_run(conn, &run_db)
        })
        .await
        .unwrap();

    crate::api::workflows::spawn_manual_run(&state, workflow, run, None, false);
    let settled = wait_until_settled(&state, "run-snapshot-missing").await;

    assert_eq!(settled.status, RunStatus::Failed);
    assert!(settled.finished_at.is_some());
    let reason = settled.step_results.last().expect("failure step");
    assert_eq!(reason.step_name, "__run_error__");
    assert!(
        reason.output.contains("snapshot unavailable or expired"),
        "the reason is kept: {}",
        reason.output
    );
    let shared = state
        .db
        .with_conn(|conn| crate::db::shared_runs::get(conn, "run-snapshot-missing"))
        .await
        .unwrap()
        .expect("shared run projected");
    assert!(matches!(
        shared.status,
        crate::models::SharedRunStatus::Failed
    ));
}

#[tokio::test]
async fn settling_an_errored_gate_resume_fails_the_run_claimed_running() {
    let (state, _, _) = test_state_and_configs();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-settle-resume".into();
    let mut run = pending_run("run-settle-resume", &workflow.id);
    let (wf_db, mut run_db) = (workflow.clone(), run.clone());
    run_db.status = RunStatus::Running;
    state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::insert_workflow(conn, &wf_db)?;
            crate::db::workflows::insert_run(conn, &run_db)
        })
        .await
        .unwrap();
    run.status = RunStatus::WaitingApproval;

    settle_errored_run(
        &state,
        &workflow,
        &mut run,
        &anyhow::anyhow!("resume broke"),
    )
    .await;

    let persisted = wait_until_settled(&state, "run-settle-resume").await;
    assert_eq!(persisted.status, RunStatus::Failed);
    assert!(persisted.step_results[0].output.contains("resume broke"));
}

#[tokio::test]
async fn settling_an_errored_run_keeps_a_cancel_that_won_the_race() {
    let (state, _, _) = test_state_and_configs();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-settle-cancelled".into();
    let mut run = pending_run("run-settle-cancelled", &workflow.id);
    let (wf_db, mut run_db) = (workflow.clone(), run.clone());
    run_db.status = RunStatus::Cancelled;
    state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::insert_workflow(conn, &wf_db)?;
            crate::db::workflows::insert_run(conn, &run_db)
        })
        .await
        .unwrap();
    run.status = RunStatus::Running;

    settle_errored_run(&state, &workflow, &mut run, &anyhow::anyhow!("late error")).await;

    assert_eq!(run.status, RunStatus::Cancelled, "no failure is reported");
    assert!(run.step_results.is_empty());
    let persisted = wait_until_settled(&state, "run-settle-cancelled").await;
    assert_eq!(persisted.status, RunStatus::Cancelled);
    assert!(persisted.step_results.is_empty());
}

#[tokio::test]
async fn execute_run_finishes_when_the_sse_receiver_stays_full() {
    let (state, tokens, agents) = test_state_and_configs();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-full-sse".into();
    workflow.steps = vec![
        json_data_step("first", serde_json::json!({ "n": 1 })),
        json_data_step("second", serde_json::json!({ "n": 2 })),
    ];
    let mut run = pending_run("run-full-sse", &workflow.id);
    insert_wf_and_run(&state, &workflow, &run).await;

    // Capacity one and an intentionally alive, non-consuming receiver:
    // the first event fills the channel and every later event must drop.
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(1);
    execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        Some(events_tx),
        None,
        None,
    )
    .await
    .expect("SSE backpressure must not affect workflow execution");

    assert_eq!(run.status, RunStatus::Success);
    assert!(matches!(
        events_rx.try_recv(),
        Ok(RunEvent::StepStart { .. })
    ));
    assert!(
        events_rx.try_recv().is_err(),
        "events after the full buffer are best-effort and may be dropped"
    );
    let persisted = state
        .db
        .with_conn(|conn| crate::db::workflows::get_run(conn, "run-full-sse"))
        .await
        .expect("read persisted run after saturated SSE")
        .expect("persisted run exists");
    assert_eq!(
        persisted.status,
        RunStatus::Success,
        "the database remains the source of truth when SSE events drop"
    );
}

#[test]
fn emitting_to_a_dropped_sse_receiver_is_a_noop() {
    let (events_tx, events_rx) = tokio::sync::mpsc::channel(1);
    drop(events_rx);

    try_emit_run_event(
        Some(&events_tx),
        RunEvent::RunDone {
            status: RunStatus::Success,
        },
    );

    assert!(events_tx.is_closed());
}

#[tokio::test]
async fn execute_run_drives_notify_chain_against_test_only_loopback_policy() {
    let received = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<(String, String)>::new()));
    let received_by_server = received.clone();
    let app = axum::Router::new().route(
        "/notify/{stage}",
        axum::routing::post(
            move |axum::extract::Path(stage): axum::extract::Path<String>, body: String| {
                let received = received_by_server.clone();
                async move {
                    received.lock().await.push((stage, body));
                    axum::Json(serde_json::json!({ "received": true }))
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local webhook");
    let port = listener.local_addr().expect("local address").port();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve local webhook");
    });

    let (state, tokens, agents) = test_state_and_configs();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-notify-hermetic".into();
    workflow.steps = vec![
        json_data_step(
            "seed",
            serde_json::json!({ "ticket": "EW-9999", "priority": "P1", "count": 3 }),
        ),
        notify_step(
            "notify_a",
            format!("http://127.0.0.1:{port}/notify/a"),
            r#"{"phase":"a","ticket":"{{steps.seed.data.ticket}}"}"#,
        ),
        notify_step(
            "notify_b",
            format!("http://127.0.0.1:{port}/notify/b"),
            r#"{"phase":"b","previous_status":{{steps.notify_a.data.http_status}}}"#,
        ),
    ];
    let mut run = pending_run("run-notify-hermetic", &workflow.id);
    insert_wf_and_run(&state, &workflow, &run).await;

    execute_run_with_notify_policy(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
        NotifySecurityPolicy::AllowLoopbackForTests,
    )
    .await
    .expect("hermetic Notify chain must execute");
    server.abort();

    assert_eq!(run.status, RunStatus::Success);
    assert_eq!(run.step_results.len(), 3);
    assert!(
        run.step_results
            .iter()
            .all(|result| result.status == RunStatus::Success),
        "all runner-dispatched steps succeed: {:?}",
        run.step_results,
    );

    let captured = received.lock().await.clone();
    assert_eq!(captured.len(), 2, "both Notify requests reached the sink");
    assert_eq!(captured[0].0, "a");
    assert!(
        captured[0].1.contains(r#""ticket":"EW-9999""#),
        "JsonData output is interpolated into Notify A: {}",
        captured[0].1,
    );
    assert_eq!(captured[1].0, "b");
    assert!(
        captured[1].1.contains(r#""previous_status":200"#),
        "Notify A output is interpolated into Notify B: {}",
        captured[1].1,
    );

    let persisted = state
        .db
        .with_conn(|conn| crate::db::workflows::get_run(conn, "run-notify-hermetic"))
        .await
        .expect("read persisted run")
        .expect("persisted run exists");
    assert_eq!(persisted.status, RunStatus::Success);
    assert_eq!(persisted.step_results.len(), 3);
}

#[tokio::test]
async fn execute_run_stops_an_in_flight_agent_at_the_active_time_deadline() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap()
        .success());
    let repo_path = repo.path().to_string_lossy().into_owned();
    let project: crate::models::Project = serde_json::from_value(serde_json::json!({
        "id":"proj-timeout-in-flight", "name":"timeout-in-flight", "path":repo_path,
        "repo_url":null, "token_override":null, "ai_config":{"detected":false,"configs":[]},
        "created_at":chrono::Utc::now().to_rfc3339(), "updated_at":chrono::Utc::now().to_rfc3339()
    }))
    .unwrap();
    state
        .db
        .with_conn(move |conn| crate::db::projects::insert_project(conn, &project))
        .await
        .unwrap();
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let work_dir =
        crate::agents::runner::resolve_agent_work_dir(Some(&repo_path), &repo_path).unwrap();
    let _route = crate::agents::runner::test_acp_routes::route(
        &work_dir,
        std::sync::Arc::new(PausedClaudeTurn { release }),
    );

    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-timeout-in-flight".into();
    workflow.project_id = Some("proj-timeout-in-flight".into());
    workflow.guards = Some(WorkflowGuards {
        timeout_seconds: Some(1),
        ..WorkflowGuards::default()
    });
    let mut agent = fake_step("slow_agent");
    agent.prompt_template = "Wait for the timeout".into();
    workflow.steps = vec![agent];
    let mut run = pending_run("run-timeout-in-flight", &workflow.id);
    insert_wf_and_run(&state, &workflow, &run).await;

    let started = tokio::time::Instant::now();
    execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("timeout is a terminal run outcome, not an executor error");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the runner must drop the in-flight agent future at its deadline"
    );
    assert_eq!(run.status, RunStatus::StoppedByGuard);
    assert_eq!(run.step_results.len(), 1);
    assert_eq!(run.step_results[0].status, RunStatus::StoppedByGuard);
    assert_eq!(run.step_results[0].step_name, "slow_agent");

    let persisted = state
        .db
        .with_conn(|conn| crate::db::workflows::get_run(conn, "run-timeout-in-flight"))
        .await
        .expect("read persisted timeout run")
        .expect("persisted timeout run exists");
    assert_eq!(persisted.status, RunStatus::StoppedByGuard);
    assert_eq!(persisted.step_results.len(), 1);
    assert_eq!(persisted.step_results[0].status, RunStatus::StoppedByGuard);
}

#[tokio::test]
async fn execute_run_stops_an_in_flight_notify_at_the_active_time_deadline() {
    let app = axum::Router::new().route(
        "/slow",
        axum::routing::post(|| async {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            axum::Json(serde_json::json!({ "too_late": true }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind slow local webhook");
    let port = listener.local_addr().expect("local address").port();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve slow local webhook");
    });

    let (state, tokens, agents) = test_state_and_configs();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-timeout-in-flight-notify".into();
    workflow.guards = Some(WorkflowGuards {
        timeout_seconds: Some(1),
        ..WorkflowGuards::default()
    });
    workflow.steps = vec![notify_step(
        "slow_notify",
        format!("http://127.0.0.1:{port}/slow"),
        "{}",
    )];
    let mut run = pending_run("run-timeout-in-flight-notify", &workflow.id);
    insert_wf_and_run(&state, &workflow, &run).await;

    let started = tokio::time::Instant::now();
    execute_run_with_notify_policy(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
        NotifySecurityPolicy::AllowLoopbackForTests,
    )
    .await
    .expect("timeout is a terminal run outcome, not an executor error");
    server.abort();

    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the runner must drop the in-flight HTTP future at its deadline"
    );
    assert_eq!(run.status, RunStatus::StoppedByGuard);
    assert_eq!(run.step_results.len(), 1);
    assert_eq!(run.step_results[0].status, RunStatus::StoppedByGuard);
    assert_eq!(run.step_results[0].step_name, "slow_notify");

    let persisted = quota_wait_runs::stored(&state, "run-timeout-in-flight-notify").await;
    assert_eq!(persisted.status, RunStatus::StoppedByGuard);
    assert_eq!(persisted.step_results.len(), 1);
    assert_eq!(persisted.step_results[0].status, RunStatus::StoppedByGuard);
}

/// KT-1563 — a backend crash mid-step keeps the active time that step
/// spent, so the resumed run still meets its Timeout guard.
#[tokio::test]
async fn a_crash_mid_step_keeps_the_active_time_the_step_spent() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap()
        .success());
    let repo_path = repo.path().to_string_lossy().into_owned();
    let project: crate::models::Project = serde_json::from_value(serde_json::json!({
        "id":"proj-timeout-crash", "name":"timeout-crash", "path":repo_path,
        "repo_url":null, "token_override":null, "ai_config":{"detected":false,"configs":[]},
        "created_at":chrono::Utc::now().to_rfc3339(), "updated_at":chrono::Utc::now().to_rfc3339()
    }))
    .unwrap();
    state
        .db
        .with_conn(move |conn| crate::db::projects::insert_project(conn, &project))
        .await
        .unwrap();
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let work_dir =
        crate::agents::runner::resolve_agent_work_dir(Some(&repo_path), &repo_path).unwrap();
    let _route = crate::agents::runner::test_acp_routes::route(
        &work_dir,
        std::sync::Arc::new(PausedClaudeTurn {
            release: release.clone(),
        }),
    );

    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-timeout-crash".into();
    workflow.project_id = Some("proj-timeout-crash".into());
    workflow.guards = Some(WorkflowGuards {
        timeout_seconds: Some(5),
        ..WorkflowGuards::default()
    });
    let mut agent = fake_step("slow_agent");
    agent.prompt_template = "Run until the backend dies".into();
    workflow.steps = vec![agent];
    let run = pending_run("run-timeout-crash", &workflow.id);
    insert_wf_and_run(&state, &workflow, &run).await;

    let segment = {
        let (state, workflow) = (state.clone(), workflow.clone());
        let (tokens, agents) = (tokens.clone(), agents.clone());
        tokio::spawn(async move {
            let mut run = run;
            let _ = execute_run(
                state, &workflow, &mut run, &tokens, &agents, None, None, None,
            )
            .await;
        })
    };
    // 3 s into the 5 s step, the backend dies.
    loop {
        let live = quota_wait_runs::stored(&state, "run-timeout-crash").await;
        assert!(
            matches!(live.status, RunStatus::Pending | RunStatus::Running),
            "the step must still be in flight: {:?}",
            live.step_results
        );
        let active_ms = live
            .state
            .get(TIMEOUT_ACTIVE_MS_STATE_KEY)
            .and_then(|raw| raw.parse::<u64>().ok())
            .unwrap_or(0);
        if active_ms >= 3_000 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    segment.abort();
    let _ = segment.await;
    state
        .db
        .with_conn(|conn| {
            crate::db::workflows::claim_run_status(
                conn,
                "run-timeout-crash",
                &RunStatus::Running,
                &RunStatus::Interrupted,
            )
        })
        .await
        .unwrap();

    let state = quota_wait_runs::restarted(&state);
    let mut run = quota_wait_runs::stored(&state, "run-timeout-crash").await;
    claim_interrupted_run(&state, &mut run, false)
        .await
        .expect("a human resumes the interrupted run");
    // The replayed step would answer after 3 s; under 2 s of budget is left.
    let answer = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        release.notify_one();
    });
    resume_interrupted_run(state.clone(), &workflow, &mut run, &tokens, &agents, None)
        .await
        .expect("the guard stop is a run outcome");
    answer.abort();

    assert_eq!(
        run.status,
        RunStatus::StoppedByGuard,
        "{:?}",
        run.step_results
    );
    assert_eq!(
        run.step_results.last().map(|r| r.status.clone()),
        Some(RunStatus::StoppedByGuard)
    );
    let active_ms = run
        .state
        .get(TIMEOUT_ACTIVE_MS_STATE_KEY)
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap();
    assert!(active_ms >= 5_000, "active_ms={active_ms}");
}

#[tokio::test]
async fn document_audit_stops_the_run_and_persists_paths_without_erasing_files() {
    check_document_audit_in_full_run(false, true).await;
}

#[tokio::test]
async fn document_audit_preserves_preexisting_files_after_agent_launch_failure() {
    check_document_audit_in_full_run(true, true).await;
}

/// KT-1796 — a possible secret fails nothing and is never silent.
#[tokio::test]
async fn a_possible_secret_lets_the_run_go_on_and_is_recorded_for_its_reader() {
    check_document_audit_in_full_run(false, false).await;
}

/// KT-1796 — a child run is a row nobody opens: its parent says it.
#[test]
fn a_possible_secret_of_a_child_run_is_recorded_on_its_parent() {
    gate_guard_runs::on_big_stack(check_document_audit(false, false, true));
}

async fn check_document_audit_in_full_run(agent_launch_fails: bool, known_secret: bool) {
    check_document_audit(agent_launch_fails, known_secret, false).await;
}

async fn check_document_audit(agent_launch_fails: bool, known_secret: bool, as_child: bool) {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let (state, tokens, mut agents) = test_state_and_configs();
    let repo = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap()
        .success());
    std::fs::create_dir(repo.path().join("docs")).unwrap();
    let existing_path = repo.path().join("docs/preexisting.md");
    let existing = "preexisting human documentation\n".repeat(1000);
    std::fs::write(&existing_path, &existing).unwrap();
    let rejected_path = repo.path().join("docs/new report.md");
    let write_path = rejected_path.clone();
    let fake_secret = if known_secret {
        "sk-1234567890abcdefghijklmnopqrstuvwxyz"
    } else {
        "q8Z3vL0xT5mR2nB7kW9cY4hJ1pF6dS3gA8eU0iO2"
    };
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
            .respond_with(move |_: &wiremock::Request| {
                if agent_launch_fails {
                    return ResponseTemplate::new(400).set_body_json(serde_json::json!({
                        "error": {"message": "fixture provider refused the request"}
                    }));
                }
                // A concurrent writer is indistinguishable from the agent here.
                // The audit must stop the run while preserving these bytes.
                std::fs::write(&write_path, fake_secret).unwrap();
                ResponseTemplate::new(200).set_body_string(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"GO_PUBLISH\"}}]}\n\ndata: [DONE]\n\n"
                )
            }).expect(1).mount(&server).await;
    agents.lite_llm.base_url = Some(server.uri());
    agents.model_tiers.lite_llm.default = Some("test-model".into());
    let project: crate::models::Project = serde_json::from_value(serde_json::json!({
        "id":"proj-doc-audit", "name":"document audit", "path":repo.path().to_string_lossy(),
        "repo_url":null, "token_override":null, "ai_config":{"detected":false,"configs":[]},
        "created_at":chrono::Utc::now().to_rfc3339(), "updated_at":chrono::Utc::now().to_rfc3339()
    }))
    .unwrap();
    state
        .db
        .with_conn(move |conn| crate::db::projects::insert_project(conn, &project))
        .await
        .unwrap();
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-doc-audit".into();
    wf.project_id = Some("proj-doc-audit".into());
    let mut agent = fake_step("advise");
    agent.agent = AgentType::LiteLlm;
    agent.prompt_template = "Return GO_PUBLISH".into();
    agent.on_result = vec![StepConditionRule {
        contains: "GO_PUBLISH".into(),
        action: ConditionAction::Goto {
            step_name: "publish".into(),
            max_iterations: None,
        },
    }];
    wf.steps = vec![
        agent,
        json_data_step("publish", serde_json::json!({"unexpected":true})),
    ];
    if as_child {
        let child = wf.clone();
        state
            .db
            .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &child))
            .await
            .unwrap();
        let mut parent = make_workflow_with_artifacts(Default::default());
        parent.id = "wf-doc-audit-parent".into();
        // Its own project: the child, pinned to the audited one, runs there.
        let parent_repo = tempfile::TempDir::new().unwrap();
        git_in(parent_repo.path(), &["init", "-q", "-b", "main"]).await;
        insert_project_at(&state, "proj-doc-audit-parent", parent_repo.path()).await;
        parent.project_id = Some("proj-doc-audit-parent".into());
        let mut call = fake_step("call_child");
        call.step_type = StepType::SubWorkflow;
        call.sub_workflow_id = Some(wf.id.clone());
        parent.steps = vec![call];
        let mut run = pending_run("run-doc-audit-parent", &parent.id);
        run.project_id = parent.project_id.clone();
        insert_wf_and_run(&state, &parent, &run).await;
        execute_run(
            state.clone(),
            &parent,
            &mut run,
            &tokens,
            &agents,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
        let persisted = state
            .db
            .with_conn(|conn| crate::db::workflows::get_run(conn, "run-doc-audit-parent"))
            .await
            .unwrap()
            .unwrap();
        let child_run = persisted.step_results[0]
            .child_run_id
            .clone()
            .expect("a child");
        let key = format!("__kronn.docs_child_possible_secrets.advise (sub-run {child_run})");
        let report = persisted
            .state
            .get(&key)
            .unwrap_or_else(|| panic!("no {key} in {:?}", persisted.state));
        assert!(report.contains("docs/new report.md"), "{report}");
        assert!(!report.contains(fake_secret), "{report}");
        return;
    }
    let mut run = pending_run("run-doc-audit", &wf.id);
    insert_wf_and_run(&state, &wf, &run).await;
    execute_run(
        state.clone(),
        &wf,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    if !known_secret {
        assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
        assert_eq!(run.step_results.len(), 2, "the run went on to publish");
        let output = &run.step_results[0].output;
        assert!(!output.contains("Document audit failed"), "{output}");
        let persisted = state
            .db
            .with_conn(|conn| crate::db::workflows::get_run(conn, "run-doc-audit"))
            .await
            .unwrap()
            .unwrap();
        let report = persisted
            .state
            .get("__kronn.docs_possible_secrets.advise")
            .expect("the possible secret is recorded");
        assert!(!report.contains(fake_secret), "{report}");
        let report: serde_json::Value = serde_json::from_str(report).unwrap();
        assert_eq!(
            report["documents"],
            serde_json::json!([{"path": "docs/new report.md", "lines": [1], "more_lines": 0}])
        );
        assert!(!persisted.state.contains_key("docs_write_rejections.advise"));
        return;
    }
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(
        run.step_results.len(),
        1,
        "publish must never execute, even with a Goto condition"
    );
    if agent_launch_fails {
        assert!(
            !rejected_path.exists(),
            "failed launch made no document writes"
        );
    } else {
        assert_eq!(std::fs::read_to_string(rejected_path).unwrap(), fake_secret);
    }
    assert_eq!(std::fs::read_to_string(existing_path).unwrap(), existing);
    let persisted = state
        .db
        .with_conn(|conn| crate::db::workflows::get_run(conn, "run-doc-audit"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.status, RunStatus::Failed);
    assert_eq!(persisted.step_results[0].status, RunStatus::Failed);
    if agent_launch_fails {
        assert!(!persisted.step_results[0]
            .output
            .contains("Document audit failed"));
        assert!(!persisted.state.contains_key("docs_write_rejections.advise"));
        return;
    }
    assert!(persisted.step_results[0]
        .output
        .contains("docs/new report.md"));
    assert!(!persisted.step_results[0].output.contains(fake_secret));
    assert_eq!(
        persisted
            .state
            .get("docs_write_rejections.advise")
            .map(String::as_str),
        Some("1")
    );
}

/// A Claude Code turn that reports one tool call, then waits for the test
/// before answering, so the step can be observed while it is running.
struct PausedClaudeTurn {
    release: std::sync::Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl crate::acp::AcpTransport for PausedClaudeTurn {
    async fn initialize(
        &self,
        _: crate::acp::AcpInitialize,
    ) -> Result<crate::acp::AcpNegotiatedCapabilities, crate::acp::AcpError> {
        Ok(crate::acp::AcpNegotiatedCapabilities {
            protocol_version: 1,
            capabilities: std::collections::BTreeSet::from([
                crate::acp::AcpCapability::Sessions,
                crate::acp::AcpCapability::Streaming,
                crate::acp::AcpCapability::Cancellation,
                crate::acp::AcpCapability::McpInjection,
            ]),
        })
    }
    async fn create_session(&self) -> Result<crate::acp::AcpSessionTarget, crate::acp::AcpError> {
        crate::acp::AcpSessionTarget::new(crate::acp::AcpAgent::ClaudeCode, "paused-turn")
    }
    async fn config_options(&self) -> Vec<crate::acp::AcpConfigOption> {
        Vec::new()
    }
    async fn set_config_option(
        &self,
        _: &crate::acp::AcpSessionTarget,
        _: &str,
        _: &str,
    ) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn resume_session(
        &self,
        _: &crate::acp::AcpSessionTarget,
    ) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn prompt(
        &self,
        _: &crate::acp::AcpSessionTarget,
        _: &str,
        events: tokio::sync::mpsc::Sender<crate::acp::AcpSessionEvent>,
    ) -> Result<(), crate::acp::AcpError> {
        use crate::acp::AcpSessionEvent;
        let _ = events
            .send(AcpSessionEvent::ToolCall {
                id: None,
                name: "Bash".into(),
            })
            .await;
        // A secret in the title and the raw input: neither is ever stored.
        let sentinel = crate::agents::activity::tests::SENTINEL;
        let _ = events
            .send(AcpSessionEvent::ToolActivity(
                crate::agents::activity::ToolActivityUpdate::from_acp(&serde_json::json!({
                    "toolCallId": "1", "kind": "execute",
                    "title": format!("mysql -p{sentinel}"),
                    "rawInput": {"command": format!("mysql -p{sentinel}")}
                }))
                .unwrap(),
            ))
            .await;
        self.release.notified().await;
        let _ = events.send(AcpSessionEvent::TextDelta("done".into())).await;
        let _ = events.send(AcpSessionEvent::Completed).await;
        Ok(())
    }
    async fn cancel(&self, _: &crate::acp::AcpSessionTarget) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
}

/// KT-795 — a running Agent step's latest tool call is readable through the
/// API, not only the starter's SSE stream, and gone once the step ends.
#[tokio::test]
async fn a_running_agent_step_exposes_its_latest_tool_call_through_the_api() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap()
        .success());
    let repo_path = repo.path().to_string_lossy().into_owned();
    let project: crate::models::Project = serde_json::from_value(serde_json::json!({
        "id":"proj-activity", "name":"activity", "path":repo_path,
        "repo_url":null, "token_override":null, "ai_config":{"detected":false,"configs":[]},
        "created_at":chrono::Utc::now().to_rfc3339(), "updated_at":chrono::Utc::now().to_rfc3339()
    }))
    .unwrap();
    state
        .db
        .with_conn(move |conn| crate::db::projects::insert_project(conn, &project))
        .await
        .unwrap();
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let work_dir =
        crate::agents::runner::resolve_agent_work_dir(Some(&repo_path), &repo_path).unwrap();
    let _route = crate::agents::runner::test_acp_routes::route(
        &work_dir,
        std::sync::Arc::new(PausedClaudeTurn {
            release: release.clone(),
        }),
    );
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-activity".into();
    wf.project_id = Some("proj-activity".into());
    let mut agent = fake_step("orchestrateur");
    agent.prompt_template = "Orchestrate".into();
    wf.steps = vec![agent];
    let mut run = pending_run("run-activity", &wf.id);
    insert_wf_and_run(&state, &wf, &run).await;

    let runner_state = state.clone();
    let runner = tokio::spawn(async move {
        execute_run(
            runner_state,
            &wf,
            &mut run,
            &tokens,
            &agents,
            None,
            None,
            None,
        )
        .await
        .map(|_| run)
    });

    // The paused turn holds the step open, so the readable activity is the event;
    // a run that ends first stops the wait too. The ceiling only bounds a hang.
    let status = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let axum::Json(response) = crate::api::mcp_remote::workflow_run_status(
                axum::extract::State(state.clone()),
                None,
                axum::extract::Path("run-activity".to_string()),
            )
            .await;
            if let Some(status) = response
                .data
                .filter(|s| s.current_activity.is_some() || s.finished_at.is_some())
            {
                return status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the run neither exposed its activity nor ended");
    let activity = status.current_activity.clone().unwrap_or_else(|| {
        panic!(
            "the running step's activity must become readable, run ended {}",
            status.status
        )
    });
    assert_eq!(status.current_step.as_deref(), Some("orchestrateur"));
    assert_eq!(activity.category, crate::models::ActivityCategory::Execute);
    let axum::Json(detail) = crate::api::workflows::get_run(
        axum::extract::State(state.clone()),
        axum::extract::Path(("wf-activity".to_string(), "run-activity".to_string())),
    )
    .await;
    let detail = detail.data.unwrap();
    let served = format!(
        "{} {}",
        serde_json::to_string(&detail).unwrap(),
        serde_json::to_string(&status).unwrap()
    );
    assert!(
        !served.contains("SENTINEL") && !served.contains("hunter2"),
        "{served}"
    );
    let in_flight = detail.step_results.clone().pop().unwrap();
    assert_eq!(in_flight.status, RunStatus::Running);
    assert_eq!(in_flight.last_activity, Some(activity));

    release.notify_one();
    let run = runner.await.unwrap().expect("run completes");
    assert_eq!(run.status, RunStatus::Success);
    let persisted = state
        .db
        .with_conn(|conn| crate::db::workflows::get_run(conn, "run-activity"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.step_results[0].status, RunStatus::Success);
    assert_eq!(
        persisted.step_results[0].last_activity, None,
        "the terminal result replaces the in-flight one"
    );
}

/// KT-926 — the prompt of a workflow Agent step goes to the model provider, so it
/// names no Kronn project but the one the workflow is bound to: with three
/// other projects registered, neither their names nor their paths reach the
/// agent, whether or not the project has a linked repo. The step is run by
/// the real `execute_run` on a scripted `claude`, and the test reads the
/// prompt that agent was handed on stdin.
#[cfg(unix)] // uses a POSIX FIFO; relies on POSIX process groups and signals
#[tokio::test]
async fn an_agent_step_prompt_names_no_unlinked_kronn_project() {
    use crate::api::other_projects_fixture as fixture;
    const STEP_MARKER: &str = "WF-STEP-MARKER-KT926";
    for linked in [false, true] {
        let tools = tempfile::tempdir().unwrap();
        let (program, log) = fixture::recording_claude(tools.path());
        let (state, tokens, agents) = test_state_and_configs();
        let repo = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success());
        fixture::project_among_others(&state, repo.path(), linked).await;
        let _route = fixture::route_claude(repo.path(), &program);

        let mut wf = make_workflow_with_artifacts(Default::default());
        wf.id = "wf-kt926".into();
        wf.project_id = Some(fixture::PROJECT_ID.into());
        let mut agent = fake_step("lead");
        agent.prompt_template = format!("{STEP_MARKER}: describe this project");
        wf.steps = vec![agent];
        let mut run = pending_run("run-kt926", &wf.id);
        insert_wf_and_run(&state, &wf, &run).await;

        execute_run(
            state.clone(),
            &wf,
            &mut run,
            &tokens,
            &agents,
            None,
            None,
            None,
        )
        .await
        .expect("the run completes");

        let prompts: Vec<String> = fixture::recorded_turns(&log)
            .into_iter()
            .filter(|turn| turn.contains(STEP_MARKER))
            .collect();
        assert_eq!(
            prompts.len(),
            1,
            "linked={linked}: the Agent step ran once, got {prompts:?}"
        );
        fixture::assert_no_unlinked_project(&prompts[0], "workflow Agent step");
        fixture::assert_no_candidate_pool(&prompts[0], "workflow Agent step");
    }
}

/// KT-1252 — the publisher hands the room's live progress each call the
/// step reports (a burst counts every call, capped), with nothing stored
/// for a step that has no in-flight result.
#[tokio::test]
async fn the_step_activity_publisher_feeds_the_rooms_progress() {
    use crate::models::{ActivityCategory, AgentActivity};
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
    let progress = crate::agents::run_progress::RunProgress::new();
    let (tx, rx) = tokio::sync::watch::channel(None);
    let stop = tokio_util::sync::CancellationToken::new();
    let publisher = spawn_step_activity_publisher(
        db,
        "run-publisher".into(),
        None,
        "analyse".into(),
        rx,
        stop.clone(),
        Some(progress.clone()),
    );
    let reached = |calls: u32| {
        let progress = progress.clone();
        async move {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while progress.snapshot().tool_calls != calls {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "the room's progress counted {} calls, not {calls}",
                    progress.snapshot().tool_calls
                )
            });
        }
    };
    let activity = |calls| {
        Some(AgentActivity {
            category: ActivityCategory::Execute,
            at: Utc::now(),
            calls,
        })
    };
    tx.send(activity(3)).unwrap();
    reached(3).await;
    let snapshot = progress.snapshot();
    assert_eq!(snapshot.phase, crate::models::AgentRunPhase::Tool);
    assert_eq!(snapshot.activity.len(), 3);
    assert!(snapshot
        .activity
        .iter()
        .all(|entry| entry.category == ActivityCategory::Execute));

    // A burst between two reads counts every call, up to the cap.
    tx.send(activity(300)).unwrap();
    reached(3 + 64).await;

    stop.cancel();
    publisher.await.unwrap();
}

/// KT-1252 — a rollback Agent step with a room publishes its tool
/// activity in that room's live progress, as a step of the main path does.
/// Run by the real `execute_run` on a scripted `claude` that makes one
/// tool call; the test reads the frames the room's socket receives.
#[cfg(unix)] // uses a POSIX FIFO; relies on POSIX process groups and signals
#[tokio::test]
async fn a_rollback_agent_step_feeds_its_rooms_live_progress() {
    use crate::api::other_projects_fixture as fixture;
    let tools = tempfile::tempdir().unwrap();
    let program = crate::acp::test_support::write_fixture_script(
        tools.path(),
        crate::acp::test_support::CLAUDE_TURN_WITH_CACHE,
    );
    let (state, tokens, agents) = test_state_and_configs();
    let repo = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap()
        .success());
    fixture::project_among_others(&state, repo.path(), false).await;
    let _route = fixture::route_claude(repo.path(), &program);
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                     VALUES ('room-rollback', 'R', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-rollback-room".into();
    wf.project_id = Some(fixture::PROJECT_ID.into());
    // No payload: the step fails, and the rollback runs.
    let mut broken = fake_step("broken_emit");
    broken.step_type = crate::models::StepType::JsonData;
    broken.json_data_payload = None;
    wf.steps = vec![broken];
    let mut recover = fake_step("recover");
    recover.prompt_template = "recover".into();
    recover.room_id = Some("room-rollback".into());
    wf.on_failure = vec![recover];
    let mut run = pending_run("run-rollback-room", &wf.id);
    insert_wf_and_run(&state, &wf, &run).await;
    let mut events = state.ws_broadcast.subscribe();

    execute_run(
        state.clone(),
        &wf,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("the run completes");
    let steps: Vec<_> = run
        .step_results
        .iter()
        .map(|step| {
            (
                step.step_name.clone(),
                step.status.clone(),
                step.output.clone(),
            )
        })
        .collect();

    // The room's last frame is the one carrying the stop.
    let calls = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut calls = 0;
        loop {
            match events.recv().await {
                Ok(crate::models::WsMessage::AgentRunProgress {
                    discussion_id,
                    progress,
                    ..
                }) if discussion_id == "room-rollback" => {
                    calls = calls.max(progress.tool_calls);
                    if progress.stopped.is_some() {
                        return calls;
                    }
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(error) => panic!("the socket closed: {error}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the rollback step never stopped its progress: {steps:?}"));
    assert_eq!(
        calls, 1,
        "the rollback step's tool call reached the room: {steps:?}"
    );
}

#[tokio::test]
async fn execute_run_records_failure_when_a_step_fails() {
    // A JsonData step with no payload fails (its executor returns a
    // failing StepResult). The orchestrator must surface that as a failed
    // run and persist it — not silently report Success.
    let (state, tokens, agents) = test_state_and_configs();

    let mut wf = make_workflow_with_artifacts(::std::collections::HashMap::new());
    wf.id = "wf-fail".into();
    // step_type JsonData but payload left None → executor fails it.
    let mut broken = fake_step("broken_emit");
    broken.step_type = crate::models::StepType::JsonData;
    broken.json_data_payload = None;
    wf.steps = vec![broken];

    let mut run = pending_run("run-fail", "wf-fail");
    let wf_db = wf.clone();
    let run_db = run.clone();
    state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::insert_workflow(conn, &wf_db)?;
            crate::db::workflows::insert_run(conn, &run_db)?;
            Ok(())
        })
        .await
        .unwrap();

    execute_run(
        state.clone(),
        &wf,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("a failing STEP is a run outcome, not an execute_run error");

    assert_eq!(run.status, RunStatus::Failed, "a failed step fails the run");
    let persisted = state
        .db
        .with_conn(move |conn| crate::db::workflows::get_run(conn, "run-fail"))
        .await
        .unwrap()
        .expect("run row exists");
    assert_eq!(
        persisted.status,
        RunStatus::Failed,
        "failure persisted to DB"
    );
}

// ─── A2 — manual resume of Interrupted runs (acceptance tests) ──────

async fn insert_wf_and_run(state: &crate::AppState, wf: &Workflow, run: &WorkflowRun) {
    let wf_db = wf.clone();
    let run_db = run.clone();
    let run_id = run.id.clone();
    let project_id = wf.project_id.clone();
    let secret = state
        .config
        .read()
        .await
        .encryption_secret
        .clone()
        .expect("test encryption secret");
    state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::insert_workflow(conn, &wf_db)?;
            crate::db::workflows::insert_run(conn, &run_db)?;
            let key = crate::core::crypto::parse_secret(&secret).map_err(anyhow::Error::msg)?;
            let values = std::collections::HashMap::new();
            let provenance = Vec::new();
            crate::db::execution_variable_snapshots::insert(
                conn,
                crate::db::execution_variable_snapshots::NewSnapshot {
                    run_kind: "workflow",
                    run_id: &run_id,
                    project_id: project_id.as_deref(),
                    environment_ref: "project_mcp_configs",
                    resolved_at: chrono::Utc::now(),
                    retention_days: 30,
                    expires_at: Some(chrono::Utc::now() + chrono::Duration::days(30)),
                    values: &values,
                    provenance: &provenance,
                },
                &key,
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

#[test]
fn side_effect_journal_classification_is_explicit() {
    for step_type in [
        StepType::ApiCall,
        StepType::BatchApiCall,
        StepType::Notify,
        StepType::Exec,
        // Replaying it after a crash would launch a second run.
        StepType::TriggerWorkflow,
    ] {
        assert!(
            uncertain_side_effect_type(&step_type).is_some(),
            "{step_type:?} must fail closed after an uncertain crash"
        );
    }
    for step_type in [
        StepType::Agent,
        StepType::BatchQuickPrompt,
        StepType::Gate,
        StepType::JsonData,
        StepType::SubWorkflow,
    ] {
        assert!(
            uncertain_side_effect_type(&step_type).is_none(),
            "{step_type:?} must not acquire the mechanical-effect marker"
        );
    }
}

#[tokio::test]
async fn uncertain_api_notify_and_exec_steps_refuse_automatic_resume() {
    let (state, _tokens, _agents) = test_state_and_configs();

    for (index, step_type) in [StepType::ApiCall, StepType::Notify, StepType::Exec]
        .into_iter()
        .enumerate()
    {
        let workflow_id = format!("wf-uncertain-{index}");
        let run_id = format!("run-uncertain-{index}");
        let mut workflow = make_workflow_with_artifacts(Default::default());
        workflow.id = workflow_id.clone();
        let mut step = fake_step(&format!("effect-{index}"));
        step.step_type = step_type;
        workflow.steps = vec![step.clone()];
        let mut run = pending_run(&run_id, &workflow_id);
        run.status = RunStatus::Running;
        insert_wf_and_run(&state, &workflow, &run).await;

        assert!(record_side_effect_intent(&mut run, &step, 0).unwrap());
        let snapshot = crate::db::workflows::RunProgressSnapshot::from_run(&run);
        let interrupted_id = run_id.clone();
        state
            .db
            .with_conn(move |conn| {
                assert!(crate::db::workflows::update_run_progress(conn, snapshot)?);
                assert!(crate::db::workflows::claim_run_status(
                    conn,
                    &interrupted_id,
                    &RunStatus::Running,
                    &RunStatus::Interrupted,
                )?);
                Ok(())
            })
            .await
            .unwrap();

        let read_id = run_id.clone();
        let mut interrupted = state
            .db
            .with_conn(move |conn| crate::db::workflows::get_run(conn, &read_id))
            .await
            .unwrap()
            .expect("interrupted run exists");
        let error = claim_interrupted_run(&state, &mut interrupted, false)
            .await
            .expect_err("uncertain external effect must not replay automatically");
        assert!(error.to_string().contains("Refusing an automatic replay"));

        let verify_id = run_id.clone();
        let persisted = state
            .db
            .with_conn(move |conn| crate::db::workflows::get_run(conn, &verify_id))
            .await
            .unwrap()
            .expect("refused run remains durable");
        assert_eq!(persisted.status, RunStatus::Interrupted);
        assert!(
            persisted
                .state
                .contains_key(UNCERTAIN_SIDE_EFFECT_STATE_KEY),
            "the unresolved marker remains available for the operator"
        );
    }
}

#[tokio::test]
async fn explicit_uncertain_retry_is_still_claimed_exactly_once() {
    let (state, _tokens, _agents) = test_state_and_configs();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-uncertain-retry".into();
    let mut step = fake_step("notify-once");
    step.step_type = StepType::Notify;
    workflow.steps = vec![step.clone()];
    let mut run = pending_run("run-uncertain-retry", &workflow.id);
    run.status = RunStatus::Interrupted;
    assert!(record_side_effect_intent(&mut run, &step, 0).unwrap());
    insert_wf_and_run(&state, &workflow, &run).await;

    let mut first = run.clone();
    let mut second = run;
    let first_result = claim_interrupted_run(&state, &mut first, true).await;
    let second_result = claim_interrupted_run(&state, &mut second, true).await;

    assert!(first_result.is_ok());
    assert!(second_result.is_err());
    assert!(
        !first.state.contains_key(UNCERTAIN_SIDE_EFFECT_STATE_KEY),
        "the winning explicit retry clears the marker atomically with its claim"
    );
}

#[test]
fn record_step_completion_seeds_ctx_from_the_snapshot() {
    // Codex review (stab-1 blocker): the ctx was seeded BEFORE the
    // snapshot filled step_agent/step_model → `{{steps.X.agent}}` was
    // None at every real run. This exercises the production helper on an
    // Agent step with a RAW (unsnapshotted) result.
    let mut step = fake_step("reason");
    step.agent = AgentType::Codex;
    step.agent_settings = Some(crate::models::AgentSettings {
        fallback: None,
        tools: None,
        model: Some("gpt-5.6-sol".into()),
        tier: None,
        reasoning_effort: None,
        max_tokens: None,
        connection_id: None,
        chrome: None,
    });
    let mut result = fake_result("reason");
    assert!(
        result.step_agent.is_none(),
        "raw executor output has no snapshot yet"
    );

    let mut ctx = TemplateContext::new();
    record_step_completion(&step, &mut result, &mut ctx, None);

    assert_eq!(
        ctx.render("({{steps.reason.agent}} - {{steps.reason.model}})")
            .unwrap(),
        "(Codex - gpt-5.6-sol)",
        "the signature composes from the REAL run metadata"
    );
    assert_eq!(
        result.step_agent,
        Some(AgentType::Codex),
        "snapshot persisted on the row too"
    );
}

#[tokio::test]
async fn double_resume_wins_exactly_one_claim() {
    let (state, _tokens, _agents) = test_state_and_configs();
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-claim".into();
    let mut run = pending_run("run-claim", "wf-claim");
    run.status = RunStatus::Interrupted;
    insert_wf_and_run(&state, &wf, &run).await;

    // Both callers hold the same (stale) Interrupted snapshot — the
    // double-click case. Validation passes for both; the atomic claim
    // decides.
    let mut first_snapshot = run.clone();
    let mut second_snapshot = run.clone();
    let first = claim_interrupted_run(&state, &mut first_snapshot, false).await;
    let second = claim_interrupted_run(&state, &mut second_snapshot, false).await;
    assert!(first.is_ok(), "first caller wins the claim: {first:?}");
    let err = second.expect_err("second caller must lose").to_string();
    assert!(
        err.contains("another caller"),
        "loser gets the no-double-resume error, got: {err}"
    );
}

#[tokio::test]
async fn cancelled_and_failed_stay_sticky_on_resume() {
    let (state, _tokens, _agents) = test_state_and_configs();
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-sticky".into();
    for (id, status) in [
        ("run-cancelled", RunStatus::Cancelled),
        ("run-failed", RunStatus::Failed),
    ] {
        let mut run = pending_run(id, "wf-sticky");
        run.status = status;
        let run_db = run.clone();
        let wf_db = wf.clone();
        state
            .db
            .with_conn(move |conn| {
                let _ = crate::db::workflows::insert_workflow(conn, &wf_db);
                crate::db::workflows::insert_run(conn, &run_db)
            })
            .await
            .unwrap();

        let err = claim_interrupted_run(&state, &mut run, false)
            .await
            .expect_err("non-Interrupted runs are not resumable")
            .to_string();
        assert!(err.contains("only Interrupted"), "{err}");

        // Belt-and-braces at the DB layer too: even a caller that skips
        // validation can't flip a terminal row.
        let rid = id.to_string();
        let claimed = state
            .db
            .with_conn(move |conn| {
                crate::db::workflows::claim_run_status(
                    conn,
                    &rid,
                    &RunStatus::Interrupted,
                    &RunStatus::Running,
                )
            })
            .await
            .unwrap();
        assert!(!claimed, "{id} must not be claimable Interrupted→Running");
    }
}

#[tokio::test]
async fn resume_refuses_children_batches_and_gone_worktrees() {
    let (state, _tokens, _agents) = test_state_and_configs();

    let mut child = pending_run("run-child", "wf-x");
    child.status = RunStatus::Interrupted;
    child.run_type = "subworkflow".into();
    child.parent_run_id = Some("run-parent".into());
    let err = claim_interrupted_run(&state, &mut child, false)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("parent"),
        "children resume through the parent: {err}"
    );

    let mut batch = pending_run("run-batch", "wf-x");
    batch.status = RunStatus::Interrupted;
    batch.run_type = "batch".into();
    let err = claim_interrupted_run(&state, &mut batch, false)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("batch"), "{err}");

    let mut gone = pending_run("run-gone-ws", "wf-x");
    gone.status = RunStatus::Interrupted;
    gone.workspace_path = Some("/nonexistent/kronn-worktree-a2".into());
    let err = claim_interrupted_run(&state, &mut gone, false)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("main checkout"),
        "a gone worktree must refuse, not fall back: {err}"
    );
    assert!(!err.contains("kept on"), "no branch to name: {err}");

    // A reclaimed worktree names the branch its commits were kept on.
    gone.produced_branches.push(crate::models::ProducedBranch {
        branch_name: "kronn/wf-x/run-gone".into(),
        head_sha: "abc123".into(),
        ahead: 1,
        pushed_upstream: false,
    });
    let err = claim_interrupted_run(&state, &mut gone, false)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("kept on `kronn/wf-x/run-gone`"), "{err}");
}

#[tokio::test]
async fn resume_during_first_step_reattaches_the_preserved_worktree() {
    // Codex A2 finding: a run interrupted DURING its first step has
    // step_results=[] — is_resume keyed on results alone took the
    // Workspace::create path and abandoned the preserved worktree.
    let (state, tokens, agents) = test_state_and_configs();

    let repo = tempfile::TempDir::new().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    std::fs::write(repo.path().join("README.md"), "x").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);

    let project: crate::models::Project = serde_json::from_value(serde_json::json!({
        "id": "proj-resume", "name": "p",
        "path": repo.path().to_string_lossy(),
        "repo_url": null, "token_override": null, "ai_config": {"detected": false, "configs": []},
        "created_at": chrono::Utc::now().to_rfc3339(),
        "updated_at": chrono::Utc::now().to_rfc3339(),
    }))
    .unwrap();
    state
        .db
        .with_conn(move |c| crate::db::projects::insert_project(c, &project))
        .await
        .unwrap();

    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-first-step".into();
    wf.project_id = Some("proj-resume".into());
    wf.steps = vec![json_data_step("only", serde_json::json!({ "n": 1 }))];

    // The preserved worktree: exists on disk, holds pre-crash work.
    let preserved = tempfile::TempDir::new().unwrap();
    let preserved_path = preserved.path().to_string_lossy().to_string();

    let mut run = pending_run("run-first-step", "wf-first-step");
    run.status = RunStatus::Interrupted;
    run.workspace_path = Some(preserved_path.clone());
    insert_wf_and_run(&state, &wf, &run).await;

    claim_interrupted_run(&state, &mut run, false)
        .await
        .expect("claim");
    resume_interrupted_run(state.clone(), &wf, &mut run, &tokens, &agents, None)
        .await
        .expect("resume must not error");

    assert_eq!(run.status, RunStatus::Success);
    assert_eq!(run.step_results.len(), 1, "the first step re-ran");
    assert_eq!(
        run.workspace_path.as_deref(),
        Some(preserved_path.as_str()),
        "resume must ATTACH the preserved worktree, not create a second one"
    );
}

#[tokio::test]
async fn resume_continues_from_the_step_after_the_last_completed_one() {
    let (state, tokens, agents) = test_state_and_configs();

    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-resume".into();
    wf.steps = vec![
        json_data_step("first", serde_json::json!({ "phase": "one" })),
        json_data_step("second", serde_json::json!({ "phase": "two" })),
    ];

    // The crash snapshot: step `first` completed + persisted, the process
    // died during `second`, the boot reconcile flipped the row.
    let mut run = pending_run("run-resume", "wf-resume");
    run.status = RunStatus::Interrupted;
    run.step_results.push(fake_result("first"));
    run.state.insert("survivor".into(), "kept".into());
    insert_wf_and_run(&state, &wf, &run).await;

    claim_interrupted_run(&state, &mut run, false)
        .await
        .expect("claim");
    resume_interrupted_run(state.clone(), &wf, &mut run, &tokens, &agents, None)
        .await
        .expect("resume must not error");

    assert_eq!(run.status, RunStatus::Success);
    assert_eq!(
        run.step_results.len(),
        2,
        "only `second` re-ran — `first` was NOT re-executed"
    );
    assert_eq!(run.step_results[0].step_name, "first");
    assert_eq!(run.step_results[1].step_name, "second");
    assert_eq!(
        run.state.get("survivor").map(String::as_str),
        Some("kept"),
        "durable state survives the resume"
    );
    let resume_history: serde_json::Value = serde_json::from_str(
        run.state
            .get(RUN_RESUME_HISTORY_KEY)
            .expect("resume history persisted"),
    )
    .unwrap();
    assert_eq!(
        resume_history["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|event| event["status"].as_str())
            .collect::<Vec<_>>(),
        vec!["Interrupted", "Running"],
        "the UI can retain the interruption after the terminal status replaces Running"
    );

    let persisted = state
        .db
        .with_conn(|conn| crate::db::workflows::get_run(conn, "run-resume"))
        .await
        .unwrap()
        .expect("row");
    assert_eq!(persisted.status, RunStatus::Success);
    assert!(
        persisted.finished_at.is_some(),
        "terminal write re-stamps finished_at"
    );
}

// ─── KT-917 — symbolic references resolved per run ───────────────────

fn exec_step(name: &str, command: &str, args: &[&str]) -> WorkflowStep {
    let mut step = fake_step(name);
    step.step_type = StepType::Exec;
    step.exec_command = Some(command.into());
    step.exec_args = args.iter().map(|arg| arg.to_string()).collect();
    step
}

#[tokio::test]
async fn an_exec_step_receives_the_local_id_of_a_workflow_reference() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = tempfile::TempDir::new().unwrap();
    insert_project_at(&state, "proj-refs", repo.path()).await;
    let mut target = make_workflow_with_artifacts(Default::default());
    target.id = "wf-local-target-7".into();
    target.name = "Nightly Triage".into();
    target.project_id = Some("proj-refs".into());
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-refs".into();
    workflow.project_id = Some("proj-refs".into());
    workflow.exec_allowlist = vec!["echo".into()];
    workflow.steps = vec![exec_step(
        "launch",
        "echo",
        &["--target={{ref:workflow:nightly-triage}}"],
    )];
    let mut run = pending_run("run-refs", &workflow.id);
    run.project_id = Some("proj-refs".into());
    let target_db = target.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &target_db))
        .await
        .unwrap();
    insert_wf_and_run(&state, &workflow, &run).await;

    execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("run");

    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let output = &run.step_results.last().unwrap().output;
    assert!(output.contains("--target=wf-local-target-7"), "{output}");
}

#[tokio::test]
async fn an_unknown_reference_fails_its_step_before_the_command_starts() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = tempfile::TempDir::new().unwrap();
    insert_project_at(&state, "proj-refs-unknown", repo.path()).await;
    let marker = repo.path().join("launched");
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-refs-unknown".into();
    workflow.project_id = Some("proj-refs-unknown".into());
    workflow.exec_allowlist = vec!["touch".into()];
    let marker_arg = marker.to_string_lossy().to_string();
    workflow.steps = vec![exec_step(
        "launch",
        "touch",
        &[marker_arg.as_str(), "{{ref:workflow:no-such-workflow}}"],
    )];
    let mut run = pending_run("run-refs-unknown", &workflow.id);
    run.project_id = Some("proj-refs-unknown".into());
    insert_wf_and_run(&state, &workflow, &run).await;

    execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("run");

    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    let output = &run.step_results.last().unwrap().output;
    assert!(output.contains("ref:workflow:no-such-workflow"), "{output}");
    assert!(!marker.exists(), "the command must not have been launched");
}

// ─── KT-920 — repository profile ─────────────────────────────────────

const PROFILE_MAIN: &str =
    "schema_version = 1\n[validation.targets.lint]\ncommand = \"make lint-from-main\"\n";

#[tokio::test]
async fn an_exec_step_reads_the_profile_of_the_default_branch_not_of_its_worktree() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, "proj-profile").await;
    let root = repo.path();
    std::fs::create_dir_all(root.join("kronn")).unwrap();
    std::fs::write(root.join("kronn/project.toml"), PROFILE_MAIN).unwrap();
    git_in(root, &["add", "."]).await;
    git_in(root, &["commit", "-q", "-m", "profile"]).await;
    // The checkout sits on a branch that rewrites the profile: the run's
    // worktree starts from it, the profile must not.
    git_in(root, &["checkout", "-q", "-b", "pr-branch"]).await;
    std::fs::write(
        root.join("kronn/project.toml"),
        PROFILE_MAIN.replace("lint-from-main", "lint-from-pr"),
    )
    .unwrap();
    git_in(root, &["commit", "-q", "-am", "pr rewrites its rules"]).await;
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-profile".into();
    workflow.name = "profile".into();
    workflow.project_id = Some("proj-profile".into());
    workflow.workspace_config = Some(WorkspaceConfig {
        hooks: WorkspaceHooks::default(),
        require_isolation: true,
        main_tree_read_only: false,
        base_ref: None,
    });
    workflow.exec_allowlist = vec!["cat".into(), "echo".into()];
    workflow.steps = vec![
        exec_step("worktree_copy", "cat", &["kronn/project.toml"]),
        exec_step(
            "profile_value",
            "echo",
            &["lint={{project.validation.targets.lint.command}}"],
        ),
    ];
    let mut run = pending_run("run-profile", &workflow.id);
    run.project_id = Some("proj-profile".into());
    insert_wf_and_run(&state, &workflow, &run).await;

    execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("run");

    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert!(run.workspace_path.is_some(), "the run is isolated");
    assert!(
        run.step_results[0].output.contains("lint-from-pr"),
        "{}",
        run.step_results[0].output
    );
    let read = &run.step_results[1].output;
    assert!(read.contains("lint=make lint-from-main"), "{read}");
    assert!(!read.contains("lint-from-pr"), "{read}");
}

#[tokio::test]
async fn a_run_reading_a_profile_its_project_lacks_fails_before_its_first_step() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, "proj-no-profile").await;
    let marker = repo.path().join("launched");
    let marker_arg = marker.to_string_lossy().to_string();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-no-profile".into();
    workflow.project_id = Some("proj-no-profile".into());
    workflow.exec_allowlist = vec!["touch".into()];
    workflow.steps = vec![exec_step(
        "launch",
        "touch",
        &[marker_arg.as_str(), "{{project.forge.base_branch}}"],
    )];
    let mut run = pending_run("run-no-profile", &workflow.id);
    run.project_id = Some("proj-no-profile".into());
    insert_wf_and_run(&state, &workflow, &run).await;

    execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("run");

    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    let result = run.step_results.last().unwrap();
    assert_eq!(result.step_name, "__project_profile__");
    assert!(
        result.output.contains("kronn/project.toml")
            && result.output.contains("project.forge.base_branch"),
        "{}",
        result.output
    );
    assert!(!marker.exists(), "the command must not have been launched");
}

#[tokio::test]
async fn a_project_without_a_profile_runs_a_workflow_that_does_not_read_one() {
    let (state, tokens, agents) = test_state_and_configs();
    let _repo = git_project(&state, "proj-plain").await;
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-plain".into();
    workflow.project_id = Some("proj-plain".into());
    workflow.exec_allowlist = vec!["echo".into()];
    workflow.steps = vec![exec_step(
        "hello",
        "echo",
        &["branch={{project.forge.base_branch ?? \"none\"}}"],
    )];
    let mut run = pending_run("run-plain", &workflow.id);
    run.project_id = Some("proj-plain".into());
    insert_wf_and_run(&state, &workflow, &run).await;

    execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("run");

    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert!(run.step_results[0].output.contains("branch=none"));
}

/// Commits `content` as the profile on main (or removes it when `None`).
async fn commit_profile(root: &std::path::Path, content: Option<&str>) {
    let file = root.join("kronn/project.toml");
    match content {
        Some(text) => {
            std::fs::create_dir_all(root.join("kronn")).unwrap();
            std::fs::write(&file, text).unwrap();
            git_in(root, &["add", "."]).await;
        }
        None => {
            git_in(root, &["rm", "-q", "kronn/project.toml"]).await;
        }
    }
    git_in(root, &["commit", "-q", "-m", "profile"]).await;
}

fn restarted(state: &crate::AppState) -> crate::AppState {
    let cfg = crate::core::config::default_config();
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(cfg));
    crate::AppState::new_defaults(
        config,
        state.db.clone(),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

/// Runs `echo before`, a Gate, `echo after`; `pause` edits the repository
/// while the run waits, then a restarted Kronn approves the Gate.
async fn profile_across_a_gate(
    id: &str,
    initial: Option<&str>,
    pause: Option<Option<&str>>,
    read: &str,
) -> WorkflowRun {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, id).await;
    if let Some(text) = initial {
        commit_profile(repo.path(), Some(text)).await;
    }
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = format!("wf-{id}");
    workflow.project_id = Some(id.into());
    workflow.exec_allowlist = vec!["echo".into()];
    let mut gate = fake_step("review");
    gate.step_type = StepType::Gate;
    workflow.steps = vec![
        exec_step("before", "echo", &[read]),
        gate,
        exec_step("after", "echo", &[read]),
    ];
    let mut run = pending_run(&format!("run-{id}"), &workflow.id);
    run.project_id = Some(id.into());
    insert_wf_and_run(&state, &workflow, &run).await;
    execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("run");
    assert_eq!(
        run.status,
        RunStatus::WaitingApproval,
        "{:?}",
        run.step_results
    );
    if let Some(change) = pause {
        commit_profile(repo.path(), change).await;
    }
    // As the approval endpoint does: claim the waiting run, then resume.
    let claimed_id = run.id.clone();
    let claimed = state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::claim_waiting_run(conn, &claimed_id, &RunStatus::Running)
        })
        .await
        .unwrap();
    assert!(claimed);
    run.status = RunStatus::Running;
    resume_run(
        restarted(&state),
        &workflow,
        &mut run,
        GateDecision::Approve { comment: None },
        &tokens,
        &agents,
        None,
    )
    .await
    .expect("approval resumes");
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    run
}

fn step_output<'a>(run: &'a WorkflowRun, name: &str) -> &'a str {
    &run.step_results
        .iter()
        .rfind(|step| step.step_name == name)
        .unwrap_or_else(|| panic!("{name} ran: {:?}", run.step_results))
        .output
}

#[tokio::test]
async fn a_resumed_run_keeps_the_profile_it_started_with() {
    let read = "lint={{project.validation.targets.lint.command}}";
    let modified = PROFILE_MAIN.replace("lint-from-main", "lint-changed-in-pause");
    for (id, pause) in [
        ("proj-pin-modified", Some(modified.as_str())),
        ("proj-pin-deleted", None),
    ] {
        let run = profile_across_a_gate(id, Some(PROFILE_MAIN), Some(pause), read).await;
        for step in ["before", "after"] {
            let output = step_output(&run, step);
            assert!(
                output.contains("lint=make lint-from-main"),
                "{id} {step}: {output}"
            );
        }
    }
}

#[tokio::test]
async fn a_profile_added_during_a_pause_does_not_reach_the_resumed_run() {
    let read = "lint={{project.validation.targets.lint.command ?? \"none\"}}";
    let run = profile_across_a_gate("proj-pin-added", None, Some(Some(PROFILE_MAIN)), read).await;
    assert!(step_output(&run, "before").contains("lint=none"));
    let after = step_output(&run, "after");
    assert!(after.contains("lint=none"), "{after}");
}

#[tokio::test]
async fn a_child_run_reads_the_profile_its_parent_pinned() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, "proj-pin-child").await;
    commit_profile(repo.path(), Some(PROFILE_MAIN)).await;
    let mut parent = make_workflow_with_artifacts(Default::default());
    parent.id = "wf-pin-parent".into();
    parent.project_id = Some("proj-pin-child".into());
    let mut gate = fake_step("review");
    gate.step_type = StepType::Gate;
    parent.steps = vec![gate];
    let mut parent_run = pending_run("run-pin-parent", &parent.id);
    parent_run.project_id = parent.project_id.clone();
    insert_wf_and_run(&state, &parent, &parent_run).await;
    execute_run(
        state.clone(),
        &parent,
        &mut parent_run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("parent");
    assert_eq!(parent_run.status, RunStatus::WaitingApproval);
    commit_profile(
        repo.path(),
        Some(&PROFILE_MAIN.replace("lint-from-main", "lint-after-parent")),
    )
    .await;

    let mut child = make_workflow_with_artifacts(Default::default());
    child.id = "wf-pin-child".into();
    child.project_id = parent.project_id.clone();
    child.exec_allowlist = vec!["echo".into()];
    child.steps = vec![exec_step(
        "item",
        "echo",
        &["lint={{project.validation.targets.lint.command}}"],
    )];
    let mut child_run = pending_run("run-pin-child", &child.id);
    child_run.project_id = child.project_id.clone();
    child_run.parent_run_id = Some(parent_run.id.clone());
    insert_wf_and_run(&state, &child, &child_run).await;
    execute_run(
        state.clone(),
        &child,
        &mut child_run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("child");
    assert_eq!(
        child_run.status,
        RunStatus::Success,
        "{:?}",
        child_run.step_results
    );
    let output = step_output(&child_run, "item");
    assert!(output.contains("lint=make lint-from-main"), "{output}");
}

/// Parent in project A paused at a Gate; child B1 (project B) runs, B's
/// profile changes, then child B2 runs, pauses at a Gate and resumes on a
/// restarted Kronn. Returns what B1 and B2 (before and after) read.
async fn two_children_of_another_project(
    tag: &str,
    initial_b: Option<&str>,
    changed_b: Option<&str>,
    read: &str,
) -> (String, String, String) {
    let (state, tokens, agents) = test_state_and_configs();
    let _repo_a = git_project(&state, &format!("proj-{tag}-a")).await;
    let repo_b = git_project(&state, &format!("proj-{tag}-b")).await;
    if let Some(text) = initial_b {
        commit_profile(repo_b.path(), Some(text)).await;
    }
    let mut parent = make_workflow_with_artifacts(Default::default());
    parent.id = format!("wf-{tag}-parent");
    parent.project_id = Some(format!("proj-{tag}-a"));
    let mut gate = fake_step("hold");
    gate.step_type = StepType::Gate;
    parent.steps = vec![gate];
    let mut parent_run = pending_run(&format!("run-{tag}-parent"), &parent.id);
    parent_run.project_id = parent.project_id.clone();
    insert_wf_and_run(&state, &parent, &parent_run).await;
    execute_run(
        state.clone(),
        &parent,
        &mut parent_run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("parent");
    assert_eq!(parent_run.status, RunStatus::WaitingApproval);

    let child_run_of = |id: &str, workflow: &Workflow| {
        let mut run = pending_run(id, &workflow.id);
        run.project_id = workflow.project_id.clone();
        run.parent_run_id = Some(parent_run.id.clone());
        run
    };
    let mut b1 = make_workflow_with_artifacts(Default::default());
    b1.id = format!("wf-{tag}-b1");
    b1.project_id = Some(format!("proj-{tag}-b"));
    b1.exec_allowlist = vec!["echo".into()];
    b1.steps = vec![exec_step("item", "echo", &[read])];
    let mut b1_run = child_run_of(&format!("run-{tag}-b1"), &b1);
    insert_wf_and_run(&state, &b1, &b1_run).await;
    execute_run(
        state.clone(),
        &b1,
        &mut b1_run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("b1");
    assert_eq!(
        b1_run.status,
        RunStatus::Success,
        "{:?}",
        b1_run.step_results
    );

    commit_profile(repo_b.path(), changed_b).await;

    let mut b2 = make_workflow_with_artifacts(Default::default());
    b2.id = format!("wf-{tag}-b2");
    b2.project_id = Some(format!("proj-{tag}-b"));
    b2.exec_allowlist = vec!["echo".into()];
    let mut gate = fake_step("review");
    gate.step_type = StepType::Gate;
    b2.steps = vec![
        exec_step("before", "echo", &[read]),
        gate,
        exec_step("after", "echo", &[read]),
    ];
    let mut b2_run = child_run_of(&format!("run-{tag}-b2"), &b2);
    insert_wf_and_run(&state, &b2, &b2_run).await;
    execute_run(
        state.clone(),
        &b2,
        &mut b2_run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("b2");
    assert_eq!(
        b2_run.status,
        RunStatus::WaitingApproval,
        "{:?}",
        b2_run.step_results
    );
    let claimed_id = b2_run.id.clone();
    assert!(state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::claim_waiting_run(conn, &claimed_id, &RunStatus::Running)
        })
        .await
        .unwrap());
    b2_run.status = RunStatus::Running;
    resume_run(
        restarted(&state),
        &b2,
        &mut b2_run,
        GateDecision::Approve { comment: None },
        &tokens,
        &agents,
        None,
    )
    .await
    .expect("b2 resumes");
    assert_eq!(
        b2_run.status,
        RunStatus::Success,
        "{:?}",
        b2_run.step_results
    );
    (
        step_output(&b1_run, "item").to_string(),
        step_output(&b2_run, "before").to_string(),
        step_output(&b2_run, "after").to_string(),
    )
}

#[tokio::test]
async fn children_of_another_project_share_one_profile_snapshot() {
    let read = "lint={{project.validation.targets.lint.command}}";
    let changed = PROFILE_MAIN.replace("lint-from-main", "lint-changed-between-children");
    let (b1, before, after) =
        two_children_of_another_project("share", Some(PROFILE_MAIN), Some(&changed), read).await;
    for output in [&b1, &before, &after] {
        assert!(output.contains("lint=make lint-from-main"), "{output}");
    }
}

#[tokio::test]
async fn children_of_another_project_share_its_absence_of_profile() {
    let read = "lint={{project.validation.targets.lint.command ?? \"none\"}}";
    let (b1, before, after) =
        two_children_of_another_project("absent", None, Some(PROFILE_MAIN), read).await;
    for output in [&b1, &before, &after] {
        assert!(output.contains("lint=none"), "{output}");
    }
}

/// A workflow whose single Gate pauses its run, keeping its pin alive.
fn paused_workflow(id: &str, project: &str) -> Workflow {
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = id.into();
    workflow.project_id = Some(project.into());
    let mut gate = fake_step("hold");
    gate.step_type = StepType::Gate;
    workflow.steps = vec![gate];
    workflow
}

#[tokio::test]
async fn a_grandchild_shares_its_profile_read_with_the_whole_tree() {
    let (state, tokens, agents) = test_state_and_configs();
    let _repo_a = git_project(&state, "proj-tree-a").await;
    let repo_b = git_project(&state, "proj-tree-b").await;
    commit_profile(repo_b.path(), Some(PROFILE_MAIN)).await;
    let read = "lint={{project.validation.targets.lint.command}}";
    let echo = |id: &str| {
        let mut workflow = make_workflow_with_artifacts(Default::default());
        workflow.id = id.into();
        workflow.project_id = Some("proj-tree-b".into());
        workflow.exec_allowlist = vec!["echo".into()];
        workflow.steps = vec![exec_step("item", "echo", &[read])];
        workflow
    };
    let start = |id: &str, workflow: &Workflow, parent: Option<&str>| {
        let mut run = pending_run(id, &workflow.id);
        run.project_id = workflow.project_id.clone();
        run.parent_run_id = parent.map(str::to_string);
        run
    };
    let mut finished = Vec::new();
    for (run_id, workflow, parent) in [
        (
            "run-tree-root",
            paused_workflow("wf-tree-root", "proj-tree-a"),
            None,
        ),
        (
            "run-tree-c",
            paused_workflow("wf-tree-c", "proj-tree-a"),
            Some("run-tree-root"),
        ),
        ("run-tree-b1", echo("wf-tree-b1"), Some("run-tree-c")),
        ("run-tree-b2", echo("wf-tree-b2"), Some("run-tree-root")),
    ] {
        if run_id == "run-tree-b2" {
            commit_profile(
                repo_b.path(),
                Some(&PROFILE_MAIN.replace("lint-from-main", "lint-changed-in-tree")),
            )
            .await;
        }
        let mut run = start(run_id, &workflow, parent);
        insert_wf_and_run(&state, &workflow, &run).await;
        execute_run(
            state.clone(),
            &workflow,
            &mut run,
            &tokens,
            &agents,
            None,
            None,
            None,
        )
        .await
        .expect(run_id);
        finished.push(run);
    }
    assert_eq!(finished[0].status, RunStatus::WaitingApproval);
    assert_eq!(finished[1].status, RunStatus::WaitingApproval);
    for run in &finished[2..] {
        assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
        let output = step_output(run, "item");
        assert!(
            output.contains("lint=make lint-from-main"),
            "{}: {output}",
            run.id
        );
    }
}

const SECRET: &str = "ghp_0123456789abcdefghijABCDEFGHIJ012345";

#[tokio::test]
async fn a_profile_refusal_stores_no_value_from_the_file() {
    refusal_stores_no_secret(
            "proj-profile-secret",
            &format!(
                "schema_version = 1\n[forge]\nrequired_approvals = \"{SECRET}\"\napi_token = \"{SECRET}\"\n"
            ),
        )
        .await;
}

#[tokio::test]
async fn a_refused_dynamic_key_reaches_neither_the_result_nor_the_pin() {
    refusal_stores_no_secret(
            "proj-profile-key",
            &format!(
                "schema_version = 1\n[tracker]\nkind = \"jira\"\n[tracker.statuses]\n\"{SECRET}\" = \"To Review\"\n"
            ),
        )
        .await;
}

async fn refusal_stores_no_secret(project: &str, profile: &str) {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, project).await;
    let token = SECRET;
    commit_profile(repo.path(), Some(profile)).await;
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = format!("wf-{project}");
    workflow.project_id = Some(project.into());
    workflow.exec_allowlist = vec!["echo".into()];
    workflow.steps = vec![exec_step("x", "echo", &["{{project.forge.base_branch}}"])];
    let run_id = format!("run-{project}");
    let mut run = pending_run(&run_id, &workflow.id);
    run.project_id = workflow.project_id.clone();
    insert_wf_and_run(&state, &workflow, &run).await;
    execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("run");
    assert_eq!(run.status, RunStatus::Failed);
    let result = run.step_results.last().unwrap();
    assert_eq!(result.step_name, "__project_profile__");
    assert!(
        result.output.contains("kronn/project.toml"),
        "{}",
        result.output
    );
    assert!(!result.output.contains(token), "{}", result.output);
    let persisted = state
        .db
        .with_conn(move |conn| {
            let run = crate::db::workflows::get_run(conn, &run_id)?;
            let pins: Vec<String> = conn
                .prepare("SELECT content_json FROM workflow_run_pins")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok((serde_json::to_string(&run)?, pins.join("\n")))
        })
        .await
        .unwrap();
    assert!(
        !persisted.0.contains(token),
        "the durable run holds the value"
    );
    assert!(!persisted.1.contains(token), "the pin holds the value");
}

// ─── KT-910 — the run's artifacts directory ──────────────────────────

#[tokio::test]
async fn an_exec_step_writes_an_image_the_next_steps_find_and_the_run_end_removes() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = tempfile::TempDir::new().unwrap();
    insert_project_at(&state, "proj-artifacts", repo.path()).await;
    let image = repo.path().join("capture.png");
    std::fs::write(&image, [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]).unwrap();
    let image_arg = image.to_string_lossy().to_string();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-artifacts".into();
    workflow.project_id = Some("proj-artifacts".into());
    workflow.exec_allowlist = vec!["cp".into(), "ls".into()];
    workflow.steps = vec![
        exec_step(
            "capture",
            "cp",
            &[image_arg.as_str(), "{{run.artifacts_dir}}/S1.png"],
        ),
        exec_step("list", "ls", &["{{run.artifacts_dir}}"]),
    ];
    let mut run = pending_run("run-artifacts-1", &workflow.id);
    run.project_id = Some("proj-artifacts".into());
    insert_wf_and_run(&state, &workflow, &run).await;
    let dir = crate::workflows::run_artifacts::dir_for(&run.id).unwrap();

    execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("run");

    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert!(
        run.step_results[1].output.contains("S1.png"),
        "{:?}",
        run.step_results
    );
    assert!(!dir.exists(), "the directory goes with the ended run");
    assert!(!image.starts_with(&dir));
}

#[test]
fn only_a_claude_or_codex_step_naming_the_directory_reads_it() {
    let mut ctx = TemplateContext::new();
    ctx.set_builtin("run.artifacts_dir", "/data/run-artifacts/run-1");
    let mut step = fake_step("constat");
    step.agent = AgentType::ClaudeCode;
    let prompt = "Look at /data/run-artifacts/run-1/S1.png";
    assert_eq!(
        crate::workflows::steps::artifacts_read_only_dirs(&step, &ctx, prompt),
        vec!["/data/run-artifacts/run-1".to_string()]
    );
    assert!(crate::workflows::steps::artifacts_read_only_dirs(&step, &ctx, "No files.").is_empty());
    step.agent = AgentType::GeminiCli;
    assert!(crate::workflows::steps::artifacts_read_only_dirs(&step, &ctx, prompt).is_empty());
    step.agent = AgentType::Codex;
    assert_eq!(
        crate::workflows::steps::artifacts_read_only_dirs(&step, &ctx, prompt).len(),
        1
    );
    assert!(crate::workflows::steps::artifacts_read_only_dirs(
        &step,
        &TemplateContext::new(),
        prompt
    )
    .is_empty());
}

// ─── KT-851 — one workflow, several projects ─────────────────────────

async fn git_project(state: &crate::AppState, id: &str) -> tempfile::TempDir {
    let repo = tempfile::TempDir::new().unwrap();
    git_in(repo.path(), &["init", "-q", "-b", "main"]).await;
    git_in(repo.path(), &["config", "user.email", "test@kronn.local"]).await;
    git_in(repo.path(), &["config", "user.name", "test"]).await;
    std::fs::write(repo.path().join("README.md"), id).unwrap();
    git_in(repo.path(), &["add", "."]).await;
    git_in(repo.path(), &["commit", "-q", "-m", "init"]).await;
    insert_project_at(state, id, repo.path()).await;
    repo
}

#[tokio::test]
async fn one_workflow_triggered_for_two_projects_runs_isolated_in_each_project() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo_a = git_project(&state, "proj-multi-a").await;
    let repo_b = git_project(&state, "proj-multi-b").await;
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-multi".into();
    workflow.name = "multi".into();
    workflow.project_id = None;
    workflow.project_scope = Some(crate::models::WorkflowProjectScope::Projects {
        project_ids: vec!["proj-multi-a".into(), "proj-multi-b".into()],
    });
    workflow.workspace_config = Some(WorkspaceConfig {
        hooks: WorkspaceHooks::default(),
        require_isolation: true,
        main_tree_read_only: false,
        base_ref: None,
    });
    workflow.exec_allowlist = vec!["cat".into()];
    workflow.steps = vec![exec_step("which", "cat", &["README.md"])];
    let wf_db = workflow.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &wf_db))
        .await
        .unwrap();

    let mut finished = Vec::new();
    for project in ["proj-multi-a", "proj-multi-b"] {
        let (wf, mut run) = crate::api::workflows::create_manual_run(
            &state,
            &workflow.id,
            Default::default(),
            Default::default(),
            crate::core::launch_context::LaunchContext {
                requested_project_id: Some(project.into()),
                ..Default::default()
            },
        )
        .await
        .expect("admitted");
        assert_eq!(run.project_id.as_deref(), Some(project));
        execute_run(
            state.clone(),
            &wf,
            &mut run,
            &tokens,
            &agents,
            None,
            None,
            None,
        )
        .await
        .expect("run");
        finished.push(run);
    }
    let refused = crate::api::workflows::create_manual_run(
        &state,
        &workflow.id,
        Default::default(),
        Default::default(),
        crate::core::launch_context::LaunchContext {
            requested_project_id: Some("proj-elsewhere".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(refused.contains("does not serve"), "{refused}");

    for (run, repo, readme) in [
        (&finished[0], &repo_a, "proj-multi-a"),
        (&finished[1], &repo_b, "proj-multi-b"),
    ] {
        assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
        assert!(
            run.step_results[0].output.contains(readme),
            "{:?}",
            run.step_results
        );
        let workspace = run.workspace_path.as_deref().expect("an isolated worktree");
        let canonical_repo = repo.path().canonicalize().unwrap();
        assert!(
            std::path::Path::new(workspace).starts_with(repo.path())
                || std::path::Path::new(workspace).starts_with(&canonical_repo),
            "{workspace} is not under its project's repository"
        );
    }
    assert_ne!(finished[0].workspace_path, finished[1].workspace_path);
}

// ─── KT-1015 — the run's launch project ──────────────────────────────

async fn insert_project_at(state: &crate::AppState, id: &str, path: &std::path::Path) {
    let now = chrono::Utc::now();
    let project: Project = serde_json::from_value(serde_json::json!({
        "id": id, "name": id, "path": path.to_string_lossy(),
        "repo_url": null, "token_override": null,
        "ai_config": {"detected": false, "configs": []},
        "created_at": now.to_rfc3339(), "updated_at": now.to_rfc3339(),
    }))
    .unwrap();
    state
        .db
        .with_conn(move |conn| crate::db::projects::insert_project(conn, &project))
        .await
        .unwrap();
}

fn pwd_step(name: &str) -> WorkflowStep {
    let mut step = fake_step(name);
    step.step_type = StepType::Exec;
    step.exec_command = Some("pwd".into());
    step
}

fn dir_name(path: &std::path::Path) -> String {
    path.file_name().unwrap().to_string_lossy().to_string()
}

#[tokio::test]
async fn a_manual_launch_records_the_launch_project_of_a_global_workflow() {
    let (state, _, _) = test_state_and_configs();
    let repo = tempfile::TempDir::new().unwrap();
    insert_project_at(&state, "proj-manual-launch", repo.path()).await;
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-manual-global".into();
    workflow.project_id = None;
    let wf_db = workflow.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &wf_db))
        .await
        .unwrap();

    let (_, run) = crate::api::workflows::create_manual_run(
        &state,
        &workflow.id,
        Default::default(),
        Default::default(),
        crate::core::launch_context::LaunchContext {
            project_id: Some("proj-manual-launch".into()),
            ..Default::default()
        },
    )
    .await
    .expect("admitted");
    let run_id = run.id.clone();
    let persisted = state
        .db
        .with_conn(move |conn| crate::db::workflows::get_run(conn, &run_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.project_id.as_deref(), Some("proj-manual-launch"));
    let stored = state
        .db
        .with_conn(|conn| crate::db::workflows::get_workflow(conn, "wf-manual-global"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.project_id, None, "the workflow itself stays global");
}

#[tokio::test]
async fn a_gate_approval_continues_a_global_workflow_in_its_launch_project() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = tempfile::TempDir::new().unwrap();
    insert_project_at(&state, "proj-gate-launch", repo.path()).await;

    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-gate-global".into();
    workflow.project_id = None;
    workflow.exec_allowlist = vec!["pwd".into()];
    let mut gate = fake_step("review");
    gate.step_type = StepType::Gate;
    workflow.steps = vec![gate, pwd_step("where")];

    let mut run = pending_run("run-gate-global", &workflow.id);
    run.project_id = Some("proj-gate-launch".into());
    run.status = RunStatus::Running;
    let mut paused = fake_result("review");
    paused.status = RunStatus::WaitingApproval;
    run.step_results = vec![paused];
    insert_wf_and_run(&state, &workflow, &run).await;

    resume_run(
        state.clone(),
        &workflow,
        &mut run,
        GateDecision::Approve { comment: None },
        &tokens,
        &agents,
        None,
    )
    .await
    .expect("approval resumes");

    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let output = &run.step_results.last().unwrap().output;
    assert!(
        output.contains(&dir_name(repo.path())),
        "the step ran in the launch project, not in the server's cwd: {output}"
    );
}

// ─── KT-1025 — agents chosen at launch ───────────────────────────────

/// An ACP turn that answers at once, whatever agent it stands in for.
struct AnsweringTurn;

#[async_trait::async_trait]
impl crate::acp::AcpTransport for AnsweringTurn {
    async fn initialize(
        &self,
        _: crate::acp::AcpInitialize,
    ) -> Result<crate::acp::AcpNegotiatedCapabilities, crate::acp::AcpError> {
        Ok(crate::acp::AcpNegotiatedCapabilities {
            protocol_version: 1,
            capabilities: std::collections::BTreeSet::from([
                crate::acp::AcpCapability::Sessions,
                crate::acp::AcpCapability::Streaming,
                crate::acp::AcpCapability::Cancellation,
                crate::acp::AcpCapability::McpInjection,
            ]),
        })
    }
    async fn create_session(&self) -> Result<crate::acp::AcpSessionTarget, crate::acp::AcpError> {
        crate::acp::AcpSessionTarget::new(crate::acp::AcpAgent::ClaudeCode, "answering-turn")
    }
    async fn config_options(&self) -> Vec<crate::acp::AcpConfigOption> {
        Vec::new()
    }
    async fn set_config_option(
        &self,
        _: &crate::acp::AcpSessionTarget,
        _: &str,
        _: &str,
    ) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn resume_session(
        &self,
        _: &crate::acp::AcpSessionTarget,
    ) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn prompt(
        &self,
        _: &crate::acp::AcpSessionTarget,
        _: &str,
        events: tokio::sync::mpsc::Sender<crate::acp::AcpSessionEvent>,
    ) -> Result<(), crate::acp::AcpError> {
        use crate::acp::AcpSessionEvent;
        let _ = events.send(AcpSessionEvent::TextDelta("done".into())).await;
        let _ = events.send(AcpSessionEvent::Completed).await;
        Ok(())
    }
    async fn cancel(&self, _: &crate::acp::AcpSessionTarget) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
}

/// A project with an answering agent route, and a workflow whose Agent
/// steps all run on Claude Code: `draft`, a Gate, then `final`.
async fn launch_choice_fixture(
    state: &crate::AppState,
    id: &str,
) -> (
    tempfile::TempDir,
    crate::agents::runner::test_acp_routes::RouteGuard,
    Workflow,
) {
    let repo = tempfile::TempDir::new().unwrap();
    insert_project_at(state, &format!("proj-{id}"), repo.path()).await;
    let repo_path = repo.path().to_string_lossy().into_owned();
    let work_dir =
        crate::agents::runner::resolve_agent_work_dir(Some(&repo_path), &repo_path).unwrap();
    let route = crate::agents::runner::test_acp_routes::route(
        &work_dir,
        std::sync::Arc::new(AnsweringTurn),
    );
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = format!("wf-{id}");
    workflow.project_id = Some(format!("proj-{id}"));
    let mut draft = fake_step("draft");
    draft.id = Some(STEP_DRAFT.into());
    draft.prompt_template = "Draft".into();
    let mut gate = fake_step("review");
    gate.step_type = StepType::Gate;
    let mut last = fake_step("final");
    last.id = Some(STEP_FINAL.into());
    last.prompt_template = "Finish".into();
    workflow.steps = vec![draft, gate, last];
    (repo, route, workflow)
}

// Stored workflows keep only UUID step ids.
const STEP_DRAFT: &str = "6f1c2a1e-0d6b-4f0e-9a51-1025000000d1";
const STEP_FINAL: &str = "6f1c2a1e-0d6b-4f0e-9a51-1025000000f1";

fn launch_choice_trigger(model: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "manual",
        crate::workflows::step_agents::TRIGGER_CONTEXT_KEY: {
            STEP_DRAFT: { "agent": "Codex", "model": model },
            STEP_FINAL: { "agent": "Codex", "model": model },
        }
    })
}

async fn stored_workflow(state: &crate::AppState, id: &str) -> Workflow {
    let id = id.to_string();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &id))
        .await
        .unwrap()
        .unwrap()
}

async fn stored_run(state: &crate::AppState, id: &str) -> WorkflowRun {
    let id = id.to_string();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::get_run(conn, &id))
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn a_launch_agent_choice_survives_a_gate_pause_reloaded_from_the_database() {
    let (state, tokens, agents) = test_state_and_configs();
    let (_repo, _route, workflow) = launch_choice_fixture(&state, "launch-gate").await;
    let mut run = pending_run("run-launch-gate", &workflow.id);
    run.project_id = workflow.project_id.clone();
    run.trigger_context = Some(launch_choice_trigger("gpt-test-b"));
    insert_wf_and_run(&state, &workflow, &run).await;

    execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("runs to the gate");
    assert_eq!(
        run.status,
        RunStatus::WaitingApproval,
        "{:?}",
        run.step_results
    );
    assert_eq!(run.step_results[0].step_agent, Some(AgentType::Codex));

    // What the gate decide route does: claim, then both reloaded from the database.
    let claimed_id = run.id.clone();
    state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::claim_waiting_run(conn, &claimed_id, &RunStatus::Running)
        })
        .await
        .unwrap();
    let reloaded_workflow = stored_workflow(&state, &workflow.id).await;
    let mut reloaded_run = stored_run(&state, &run.id).await;
    reloaded_run.status = RunStatus::Running;
    resume_run(
        state.clone(),
        &reloaded_workflow,
        &mut reloaded_run,
        GateDecision::Approve { comment: None },
        &tokens,
        &agents,
        None,
    )
    .await
    .expect("approval resumes");

    assert_eq!(
        reloaded_run.status,
        RunStatus::Success,
        "{:?}",
        reloaded_run.step_results
    );
    let last = reloaded_run.step_results.last().unwrap();
    assert_eq!(last.step_name, "final");
    assert_eq!(last.step_agent, Some(AgentType::Codex));
    let attempt = last
        .agent_provenance
        .as_ref()
        .and_then(|provenance| provenance.attempts.last())
        .expect("the step records its attempt");
    assert_eq!(attempt.agent, AgentType::Codex);
    assert_eq!(attempt.requested_model.as_deref(), Some("gpt-test-b"));

    let unchanged = stored_workflow(&state, &workflow.id).await;
    assert!(unchanged
        .steps
        .iter()
        .filter(|step| matches!(step.step_type, StepType::Agent))
        .all(|step| step.agent == AgentType::ClaudeCode));
    assert_eq!(
        unchanged.steps[0]
            .agent_settings
            .as_ref()
            .and_then(|s| s.model.clone()),
        None,
        "the workflow keeps its own model"
    );
}

#[tokio::test]
async fn a_launch_agent_choice_survives_an_interrupted_resume() {
    let (state, tokens, agents) = test_state_and_configs();
    let (_repo, _route, mut workflow) = launch_choice_fixture(&state, "launch-resume").await;
    workflow.steps.remove(1);
    let mut run = pending_run("run-launch-resume", &workflow.id);
    run.project_id = workflow.project_id.clone();
    run.status = RunStatus::Interrupted;
    run.trigger_context = Some(launch_choice_trigger("gpt-test-r"));
    run.step_results.push(fake_result("draft"));
    insert_wf_and_run(&state, &workflow, &run).await;

    let reloaded_workflow = stored_workflow(&state, &workflow.id).await;
    let mut reloaded_run = stored_run(&state, &run.id).await;
    claim_interrupted_run(&state, &mut reloaded_run, false)
        .await
        .expect("claim");
    resume_interrupted_run(
        state.clone(),
        &reloaded_workflow,
        &mut reloaded_run,
        &tokens,
        &agents,
        None,
    )
    .await
    .expect("resume");

    assert_eq!(
        reloaded_run.status,
        RunStatus::Success,
        "{:?}",
        reloaded_run.step_results
    );
    let last = reloaded_run.step_results.last().unwrap();
    assert_eq!(last.step_name, "final");
    assert_eq!(last.step_agent, Some(AgentType::Codex));
}

#[tokio::test]
async fn a_manual_launch_stores_its_agent_choice_on_the_run_and_refuses_a_wrong_one() {
    let (state, _, _) = test_state_and_configs();
    // A declared LiteLLM proxy needs no local binary, so the check passes anywhere.
    state.config.write().await.agents.lite_llm.base_url = Some("http://127.0.0.1:9".into());
    let (_repo, _route, workflow) = launch_choice_fixture(&state, "launch-store").await;
    let wf_db = workflow.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &wf_db))
        .await
        .unwrap();
    let choice = |agent: AgentType| crate::models::StepAgentOverride {
        agent,
        model: Some("m-1".into()),
        reasoning_effort: None,
    };

    let (_, run) = crate::api::workflows::create_manual_run(
        &state,
        &workflow.id,
        Default::default(),
        Default::default(),
        crate::core::launch_context::LaunchContext {
            step_agents: [("final".to_string(), choice(AgentType::LiteLlm))].into(),
            ..Default::default()
        },
    )
    .await
    .expect("admitted");
    let stored = stored_run(&state, &run.id).await;
    let chosen = crate::workflows::step_agents::from_run(&stored).expect("stored on the run");
    assert_eq!(chosen[STEP_FINAL].agent, AgentType::LiteLlm);
    assert_eq!(chosen[STEP_FINAL].model.as_deref(), Some("m-1"));
    assert_eq!(
        stored_workflow(&state, &workflow.id).await.steps[2].agent,
        AgentType::ClaudeCode
    );

    for (key, expected) in [("review", "only Agent steps"), ("ghost", "no step `ghost`")] {
        let refused = crate::api::workflows::create_manual_run(
            &state,
            &workflow.id,
            Default::default(),
            Default::default(),
            crate::core::launch_context::LaunchContext {
                step_agents: [(key.to_string(), choice(AgentType::LiteLlm))].into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert!(refused.contains(expected), "{refused}");
    }
    let workflow_id = workflow.id.clone();
    let runs = state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::list_runs_paginated(conn, &workflow_id, None, None)
        })
        .await
        .unwrap();
    assert_eq!(runs.len(), 1, "a refused choice creates no run");
}

#[tokio::test]
async fn an_interrupted_resume_continues_a_global_workflow_in_its_launch_project() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = tempfile::TempDir::new().unwrap();
    insert_project_at(&state, "proj-interrupted-launch", repo.path()).await;

    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-interrupted-global".into();
    workflow.project_id = None;
    workflow.exec_allowlist = vec!["pwd".into()];
    workflow.steps = vec![
        json_data_step("first", serde_json::json!({ "n": 1 })),
        pwd_step("where"),
    ];
    let mut run = pending_run("run-interrupted-global", &workflow.id);
    run.project_id = Some("proj-interrupted-launch".into());
    run.status = RunStatus::Interrupted;
    run.step_results.push(fake_result("first"));
    insert_wf_and_run(&state, &workflow, &run).await;

    claim_interrupted_run(&state, &mut run, false)
        .await
        .expect("claim");
    resume_interrupted_run(state.clone(), &workflow, &mut run, &tokens, &agents, None)
        .await
        .expect("resume");

    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let output = &run.step_results.last().unwrap().output;
    assert!(output.contains(&dir_name(repo.path())), "{output}");
}

#[tokio::test]
async fn a_sub_workflow_pinned_to_another_project_runs_in_that_project() {
    let (state, tokens, agents) = test_state_and_configs();
    let parent_repo = tempfile::TempDir::new().unwrap();
    git_in(parent_repo.path(), &["init", "-q", "-b", "main"]).await;
    git_in(
        parent_repo.path(),
        &["config", "user.email", "t@kronn.local"],
    )
    .await;
    git_in(parent_repo.path(), &["config", "user.name", "t"]).await;
    std::fs::write(parent_repo.path().join("README.md"), "a\n").unwrap();
    git_in(parent_repo.path(), &["add", "."]).await;
    git_in(parent_repo.path(), &["commit", "-q", "-m", "init"]).await;
    let child_repo = tempfile::TempDir::new().unwrap();
    insert_project_at(&state, "proj-parent-a", parent_repo.path()).await;
    insert_project_at(&state, "proj-child-b", child_repo.path()).await;

    let mut child = make_workflow_with_artifacts(Default::default());
    child.id = "wf-child-b".into();
    child.name = "child-b".into();
    child.project_id = Some("proj-child-b".into());
    child.exec_allowlist = vec!["pwd".into()];
    child.steps = vec![pwd_step("where")];
    let child_db = child.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &child_db))
        .await
        .unwrap();

    let mut parent = make_workflow_with_artifacts(Default::default());
    parent.id = "wf-parent-a".into();
    parent.name = "parent-a".into();
    parent.project_id = Some("proj-parent-a".into());
    parent.workspace_config = Some(WorkspaceConfig {
        hooks: WorkspaceHooks::default(),
        require_isolation: true,
        main_tree_read_only: false,
        base_ref: None,
    });
    let mut call = fake_step("call_child");
    call.step_type = StepType::SubWorkflow;
    call.sub_workflow_id = Some(child.id.clone());
    parent.steps = vec![call];
    let mut run = pending_run("run-parent-a", &parent.id);
    run.project_id = parent.project_id.clone();
    insert_wf_and_run(&state, &parent, &run).await;

    execute_run(
        state.clone(),
        &parent,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("parent run");

    let child_run_id = run.step_results[0]
        .child_run_id
        .clone()
        .unwrap_or_else(|| panic!("child launched: {:?}", run.step_results));
    let child_run = state
        .db
        .with_conn(move |conn| crate::db::workflows::get_run(conn, &child_run_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(child_run.project_id.as_deref(), Some("proj-child-b"));
    assert_eq!(
        child_run.status,
        RunStatus::Success,
        "{:?}",
        child_run.step_results
    );
    let output = &child_run.step_results[0].output;
    assert!(
        output.contains(&dir_name(child_repo.path())),
        "the child ran in its own project's repository: {output}"
    );
    assert!(
        !output.contains(&dir_name(parent_repo.path())),
        "never in the parent's worktree: {output}"
    );
}

/// KT-1755 — every nested run keeps its poll frames on the worker's stack
/// until it ends, in production as in tests. The chain runs on the
/// production runtime (`runtime_builder`, its worker stack size), never on
/// a thread this test sizes: a run that outgrows that stack aborts the
/// whole process. The root and three nested sub-workflows run to the end.
#[test]
fn a_three_level_sub_workflow_chain_runs_on_a_production_worker_stack() {
    const LEVELS: usize = 3;
    let runtime = crate::runtime_builder()
        .worker_threads(1)
        .build()
        .expect("production-shaped runtime");
    let chain = runtime.block_on(async {
        tokio::spawn(async {
            let (state, tokens, agents) = test_state_and_configs();
            // The leaf first: each level calls the one inserted before it.
            let mut called: Option<String> = None;
            let mut root: Option<Workflow> = None;
            for level in (0..=LEVELS).rev() {
                let mut workflow = make_workflow_with_artifacts(Default::default());
                workflow.id = format!("wf-chain-{level}");
                workflow.name = format!("chain-{level}");
                workflow.project_id = None;
                workflow.steps = vec![match called.take() {
                    Some(target) => {
                        let mut call = fake_step("call_next");
                        call.step_type = StepType::SubWorkflow;
                        call.sub_workflow_id = Some(target);
                        call
                    }
                    None => json_data_step("leaf", serde_json::json!({ "level": level })),
                }];
                called = Some(workflow.id.clone());
                if level == 0 {
                    root = Some(workflow);
                } else {
                    let nested = workflow.clone();
                    state
                        .db
                        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &nested))
                        .await
                        .unwrap();
                }
            }
            let root = root.expect("root workflow");
            let mut run = pending_run("run-chain-0", &root.id);
            insert_wf_and_run(&state, &root, &run).await;
            execute_run(
                state.clone(),
                &root,
                &mut run,
                &tokens,
                &agents,
                None,
                None,
                None,
            )
            .await
            .expect("root run");

            // Follow the child run ids down the chain: every level ran.
            let mut statuses = vec![run.status.clone()];
            let mut next = run.step_results[0].child_run_id.clone();
            while let Some(child_run_id) = next {
                let child = state
                    .db
                    .with_conn(move |conn| crate::db::workflows::get_run(conn, &child_run_id))
                    .await
                    .unwrap()
                    .expect("child run row");
                statuses.push(child.status.clone());
                next = child
                    .step_results
                    .first()
                    .and_then(|result| result.child_run_id.clone());
            }
            statuses
        })
        .await
        .expect("the chain finished on the worker")
    });
    assert_eq!(
        chain,
        vec![RunStatus::Success; LEVELS + 1],
        "root + {LEVELS} nested runs all succeed"
    );
}

// ─── KT-1018 — run lifecycle holes ───────────────────────────────────

async fn isolated_repo_fixture(
    state: &crate::AppState,
    project_id: &str,
) -> (tempfile::TempDir, Workflow) {
    let repo = tempfile::TempDir::new().unwrap();
    git_in(repo.path(), &["init", "-q", "-b", "main"]).await;
    git_in(repo.path(), &["config", "user.email", "t@kronn.local"]).await;
    git_in(repo.path(), &["config", "user.name", "t"]).await;
    std::fs::write(repo.path().join("README.md"), "x\n").unwrap();
    git_in(repo.path(), &["add", "."]).await;
    git_in(repo.path(), &["commit", "-q", "-m", "init"]).await;
    insert_project_at(state, project_id, repo.path()).await;
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = format!("wf-{project_id}");
    workflow.name = format!("isolated-{project_id}");
    workflow.project_id = Some(project_id.into());
    workflow.workspace_config = Some(WorkspaceConfig {
        hooks: WorkspaceHooks::default(),
        require_isolation: true,
        main_tree_read_only: false,
        base_ref: None,
    });
    workflow.exec_allowlist = vec!["pwd".into()];
    (repo, workflow)
}

#[tokio::test]
async fn a_gate_resume_whose_worktree_vanished_never_runs_in_the_main_checkout() {
    let (state, tokens, agents) = test_state_and_configs();
    let (repo, mut workflow) = isolated_repo_fixture(&state, "proj-vanished").await;
    let mut gate = fake_step("review");
    gate.step_type = StepType::Gate;
    workflow.steps = vec![gate, pwd_step("where")];

    let mut run = pending_run("run-vanished", &workflow.id);
    run.project_id = workflow.project_id.clone();
    run.status = RunStatus::Running;
    let gone = repo.path().join(".kronn/worktrees/removed-by-hand");
    run.workspace_path = Some(gone.to_string_lossy().to_string());
    let mut paused = fake_result("review");
    paused.status = RunStatus::WaitingApproval;
    run.step_results = vec![paused];
    insert_wf_and_run(&state, &workflow, &run).await;

    resume_run(
        state.clone(),
        &workflow,
        &mut run,
        GateDecision::Approve { comment: None },
        &tokens,
        &agents,
        None,
    )
    .await
    .expect("the refusal is a run outcome, not an error");

    assert_eq!(run.status, RunStatus::Failed);
    assert!(
        run.step_results
            .iter()
            .all(|step| step.step_name != "where"),
        "no step ran: {:?}",
        run.step_results
    );
    let last = run.step_results.last().unwrap();
    assert_eq!(last.step_name, "__workspace__");
    assert!(
        last.output.contains("refusing to resume"),
        "{}",
        last.output
    );
    let persisted = state
        .db
        .with_conn(|conn| crate::db::workflows::get_run(conn, "run-vanished"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.status, RunStatus::Failed);
}

#[tokio::test]
async fn an_early_error_after_the_worktree_is_created_removes_it() {
    let (state, tokens, agents) = test_state_and_configs();
    let (repo, mut workflow) = isolated_repo_fixture(&state, "proj-early-error").await;
    // A declared variable whose snapshot was never written: execute_run
    // fails after creating the worktree.
    workflow.variables = vec![crate::models::PromptVariable {
        name: "ticket".into(),
        label: "Ticket".into(),
        placeholder: String::new(),
        description: None,
        required: true,
        pattern: None,
        source: None,
        source_ref: None,
        allow_manual_override: false,
        control: None,
    }];
    workflow.steps = vec![pwd_step("where")];
    let mut run = pending_run("run-early-error", &workflow.id);
    run.project_id = workflow.project_id.clone();
    let (wf_db, run_db) = (workflow.clone(), run.clone());
    state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::insert_workflow(conn, &wf_db)?;
            crate::db::workflows::insert_run(conn, &run_db)
        })
        .await
        .unwrap();

    let error = execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect_err("the missing snapshot is an infrastructure error");
    assert!(error.to_string().contains("snapshot"), "{error}");

    let path = std::path::PathBuf::from(run.workspace_path.clone().expect("worktree created"));
    assert!(!path.exists(), "the worktree goes with the error");
    let listed = git_in(repo.path(), &["worktree", "list", "--porcelain"]).await;
    assert!(
        !listed.contains("removed") && !listed.contains(&dir_name(&path)),
        "{listed}"
    );
}

#[tokio::test]
async fn an_interrupted_resume_rechecks_the_concurrency_limit() {
    let (state, _, _) = test_state_and_configs();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-resume-limit".into();
    workflow.concurrency_limit = Some(1);
    workflow.steps = vec![json_data_step("only", serde_json::json!({ "n": 1 }))];
    let mut interrupted = pending_run("run-resume-limit", &workflow.id);
    interrupted.status = RunStatus::Interrupted;
    insert_wf_and_run(&state, &workflow, &interrupted).await;
    let mut other = pending_run("run-other-active", &workflow.id);
    other.status = RunStatus::WaitingApproval;
    let other_db = other.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_run(conn, &other_db))
        .await
        .unwrap();

    let refused = claim_interrupted_run(&state, &mut interrupted.clone(), false)
        .await
        .expect_err("a paused run holds the only slot");
    assert!(
        refused.to_string().contains("Concurrency limit"),
        "{refused}"
    );
    let still = state
        .db
        .with_conn(|conn| crate::db::workflows::get_run(conn, "run-resume-limit"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still.status, RunStatus::Interrupted, "not claimed");

    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE workflow_runs SET status = 'Success' WHERE id = 'run-other-active'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    claim_interrupted_run(&state, &mut interrupted, false)
        .await
        .expect("the slot is free again");
    assert_eq!(interrupted.status, RunStatus::Running);
}

// ─── KT-985 — a run's worktree goes at every end, its commits stay ───

fn commit_step(name: &str) -> WorkflowStep {
    let mut step = fake_step(name);
    step.step_type = StepType::Exec;
    step.exec_command = Some("git".into());
    step.exec_args = [
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "work",
    ]
    .map(String::from)
    .to_vec();
    step
}

async fn assert_worktree_gone_and_commit_kept(repo: &std::path::Path, run: &WorkflowRun) {
    let path = std::path::PathBuf::from(run.workspace_path.clone().expect("isolated"));
    assert!(!path.exists(), "the worktree is removed");
    let listed = git_in(repo, &["worktree", "list", "--porcelain"]).await;
    assert!(!listed.contains("prunable"), "{listed}");
    assert_eq!(
        run.produced_branches.len(),
        1,
        "{:?}",
        run.produced_branches
    );
    let branch = &run.produced_branches[0];
    assert_eq!(branch.ahead, 1);
    git_in(
        repo,
        &[
            "show-ref",
            "--verify",
            &format!("refs/heads/{}", branch.branch_name),
        ],
    )
    .await;
}

async fn run_to_end(state: &crate::AppState, workflow: &Workflow, run_id: &str) -> WorkflowRun {
    let (_, tokens, agents) = test_state_and_configs();
    let mut run = pending_run(run_id, &workflow.id);
    run.project_id = workflow.project_id.clone();
    insert_wf_and_run(state, workflow, &run).await;
    execute_run(
        state.clone(),
        workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("run");
    run
}

#[tokio::test]
async fn a_successful_run_removes_its_worktree_and_keeps_its_commits() {
    let (state, _, _) = test_state_and_configs();
    let (repo, mut workflow) = isolated_repo_fixture(&state, "proj-end-success").await;
    workflow.exec_allowlist = vec!["git".into()];
    workflow.steps = vec![commit_step("commit")];
    let run = run_to_end(&state, &workflow, "run-end-success").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert_worktree_gone_and_commit_kept(repo.path(), &run).await;
}

#[tokio::test]
async fn a_successful_run_keeps_a_worktree_holding_uncommitted_work() {
    // A step's output can be files no later step commits: success is not
    // a licence to delete them.
    let (state, _, _) = test_state_and_configs();
    let (_repo, mut workflow) = isolated_repo_fixture(&state, "proj-end-dirty").await;
    workflow.exec_allowlist = vec!["sh".into()];
    let mut design = fake_step("design");
    design.step_type = StepType::Exec;
    design.exec_command = Some("sh".into());
    design.exec_args = vec!["-c".into(), "printf 'spec\\n' > design.md".into()];
    workflow.steps = vec![design];
    let run = run_to_end(&state, &workflow, "run-end-dirty").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let path = std::path::PathBuf::from(run.workspace_path.clone().expect("isolated"));
    assert_eq!(
        std::fs::read_to_string(path.join("design.md")).expect("the file survives"),
        "spec\n"
    );
    let last = &run.step_results.last().expect("one step").output;
    assert!(last.contains("kept with its branch"), "{last}");
    assert!(persisted_workspace(&state, "run-end-dirty").await.is_some());
}

async fn persisted_workspace(state: &crate::AppState, run_id: &str) -> Option<String> {
    let run_id = run_id.to_string();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::get_run(conn, &run_id))
        .await
        .unwrap()
        .unwrap()
        .workspace_path
}

async fn trimmable(state: &crate::AppState, run_id: &str) -> bool {
    let run_id = run_id.to_string();
    state
        .db
        .with_conn(move |conn| {
            // A cutoff tomorrow: any finished run is old enough.
            let future = (Utc::now() + chrono::Duration::days(1)).to_rfc3339();
            crate::db::run_retention::compact_run_payloads_chunk(conn, &future, 100)?;
            Ok(conn.query_row(
                "SELECT payload_compacted_at IS NOT NULL FROM workflow_runs WHERE id = ?1",
                [run_id],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn a_run_whose_worktree_is_removed_at_runtime_becomes_trimmable() {
    // KT-984 — the path stayed set until the boot janitor, so retention
    // skipped every run finished since the last boot.
    let (state, _, _) = test_state_and_configs();
    let (_repo, mut workflow) = isolated_repo_fixture(&state, "proj-forget-path").await;
    workflow.exec_allowlist = vec!["git".into()];
    workflow.steps = vec![commit_step("commit")];
    let run = run_to_end(&state, &workflow, "run-forget-path").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert!(!std::path::Path::new(run.workspace_path.as_deref().unwrap()).exists());
    assert_eq!(persisted_workspace(&state, "run-forget-path").await, None);
    assert!(trimmable(&state, "run-forget-path").await);
}

#[tokio::test]
async fn an_errored_run_settled_after_its_worktree_went_is_trimmable() {
    let (state, tokens, agents) = test_state_and_configs();
    let (_repo, mut workflow) = isolated_repo_fixture(&state, "proj-forget-error").await;
    workflow.variables = vec![crate::models::PromptVariable {
        name: "ticket".into(),
        label: "Ticket".into(),
        placeholder: String::new(),
        description: None,
        required: true,
        pattern: None,
        source: None,
        source_ref: None,
        allow_manual_override: false,
        control: None,
    }];
    workflow.steps = vec![pwd_step("where")];
    let mut run = pending_run("run-forget-error", &workflow.id);
    run.project_id = workflow.project_id.clone();
    // No variable snapshot: execute_run fails after creating the worktree.
    let (wf_db, run_db) = (workflow.clone(), run.clone());
    state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::insert_workflow(conn, &wf_db)?;
            crate::db::workflows::insert_run(conn, &run_db)
        })
        .await
        .unwrap();
    let error = execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect_err("missing snapshot");
    settle_errored_run(&state, &workflow, &mut run, &error).await;
    assert_eq!(persisted_workspace(&state, "run-forget-error").await, None);
    assert!(trimmable(&state, "run-forget-error").await);
}

#[tokio::test]
async fn a_kept_checkout_or_a_live_run_keeps_its_path() {
    let (state, _, _) = test_state_and_configs();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-keep-path".into();
    let kept = tempfile::TempDir::new().unwrap();
    let mut finished = pending_run("run-kept-checkout", &workflow.id);
    finished.status = RunStatus::Success;
    finished.workspace_path = Some(kept.path().to_string_lossy().to_string());
    insert_wf_and_run(&state, &workflow, &finished).await;
    let mut paused = pending_run("run-paused-gone", &workflow.id);
    paused.status = RunStatus::WaitingApproval;
    paused.workspace_path = Some("/nonexistent/kronn-paused".into());
    let paused_db = paused.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_run(conn, &paused_db))
        .await
        .unwrap();

    forget_removed_workspace(&state, &finished).await;
    forget_removed_workspace(&state, &paused).await;
    assert!(persisted_workspace(&state, "run-kept-checkout")
        .await
        .is_some());
    assert!(persisted_workspace(&state, "run-paused-gone")
        .await
        .is_some());
}

#[tokio::test]
async fn a_failed_run_removes_its_worktree_and_keeps_its_commits() {
    let (state, _, _) = test_state_and_configs();
    let (repo, mut workflow) = isolated_repo_fixture(&state, "proj-end-failed").await;
    workflow.exec_allowlist = vec!["git".into()];
    let mut broken = fake_step("broken");
    broken.step_type = StepType::Exec;
    broken.exec_command = Some("git".into());
    broken.exec_args = vec!["no-such-subcommand".into()];
    workflow.steps = vec![commit_step("commit"), broken];
    let run = run_to_end(&state, &workflow, "run-end-failed").await;
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    assert_worktree_gone_and_commit_kept(repo.path(), &run).await;
}

#[tokio::test]
async fn a_cancelled_run_removes_its_worktree_and_keeps_its_commits() {
    let (state, tokens, agents) = test_state_and_configs();
    let (repo, mut workflow) = isolated_repo_fixture(&state, "proj-end-cancel").await;
    // The step announces that it is running by creating `marker`, then
    // becomes `sleep`. The marker is the observable state the canceller
    // awaits: it exists only once the commit step has finished and the
    // long step is in flight, whatever the load (KT-1756). A bounded
    // poll of the run row plus a fixed sleep used to cancel before the
    // commit under load, leaving no branch to keep.
    let marker_dir = tempfile::TempDir::new().unwrap();
    let marker = marker_dir.path().join("sleeping");
    workflow.exec_allowlist = vec!["git".into(), "sh".into()];
    let mut wait = fake_step("wait");
    wait.step_type = StepType::Exec;
    wait.exec_command = Some("sh".into());
    wait.exec_args = vec![
        "-c".into(),
        "touch \"$0\" && exec sleep 60".into(),
        marker.to_string_lossy().to_string(),
    ];
    workflow.steps = vec![commit_step("commit"), wait];
    let mut run = pending_run("run-end-cancel", &workflow.id);
    run.project_id = workflow.project_id.clone();
    insert_wf_and_run(&state, &workflow, &run).await;

    let canceller = {
        let state = state.clone();
        let marker = marker.clone();
        async move {
            // No wall-clock assumption: the loop ends on the marker. The
            // timeout only turns a run that never reaches the step into
            // a failure instead of a hung suite.
            tokio::time::timeout(std::time::Duration::from_secs(300), async {
                while !marker.exists() {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("the long step never started");
            let cancelled_at = std::time::Instant::now();
            crate::workflows::cancellation::cancel_run_tree(
                &state,
                "run-end-cancel",
                crate::workflows::cancellation::CancellationScope::RunTree,
                "cancelled_by_operator",
            )
            .await
            .unwrap();
            cancelled_at
        }
    };
    let (result, cancelled_at) = tokio::join!(
        execute_run(
            state.clone(),
            &workflow,
            &mut run,
            &tokens,
            &agents,
            None,
            None,
            None
        ),
        canceller,
    );
    result.expect("run");
    // Measured from the cancel, not from the start, so a slow start under
    // load does not count: the 60 s sleep must have been cut short.
    assert!(
        cancelled_at.elapsed() < std::time::Duration::from_secs(45),
        "the sleep ran to its end instead of being cut short"
    );
    assert_eq!(run.status, RunStatus::Cancelled, "{:?}", run.step_results);
    assert_worktree_gone_and_commit_kept(repo.path(), &run).await;
}

// ─── KT-918 — approved repository scripts ────────────────────────────

fn write_file(root: &std::path::Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// An entry that loads its helper through a path built from its cwd, the
/// dynamic import a copy alone would not cover.
const SCRIPT_ENTRY: &str = "const fs = require('fs');\n\
        const h = require(process.cwd() + '/lib/helper.cjs');\n\
        fs.writeFileSync(require('path').join(process.env.KRONN_WORKTREE, 'ran'), h);\n\
        console.log('SENTINEL=' + h + ' WT=' + process.env.KRONN_WORKTREE);\n";

async fn create_script_workflow(
    state: &crate::AppState,
    home: &str,
    scope: Option<serde_json::Value>,
    files: &[&str],
) -> Result<Workflow, String> {
    let mut request = serde_json::json!({
        "name": format!("scripts-{home}"),
        "project_id": home,
        "trigger": {"type": "Manual"},
        "exec_allowlist": ["node"],
        "steps": [{
            "name": "framing",
            "step_type": {"type": "Exec"},
            "agent": "ClaudeCode",
            "prompt_template": "",
            "exec_command": "node",
            "exec_args": ["scripts/entry.cjs"],
            "exec_script_files": files
                .iter()
                .map(|path| serde_json::json!({"path": path}))
                .collect::<Vec<_>>(),
        }],
    });
    if let Some(scope) = scope {
        request["project_scope"] = scope;
    }
    let request: crate::models::CreateWorkflowRequest = serde_json::from_value(request).unwrap();
    let response = crate::api::workflows::create_as(
        state.clone(),
        request,
        crate::api::workflows::WorkflowWriter::Human,
    )
    .await
    .0;
    match response.data {
        Some(workflow) if response.success => Ok(workflow),
        _ => Err(response.error.unwrap_or_default()),
    }
}

async fn run_scripts(state: &crate::AppState, workflow: &Workflow, project: &str) -> WorkflowRun {
    let (_, tokens, agents) = test_state_and_configs();
    let (wf, mut run) = crate::api::workflows::create_manual_run(
        state,
        &workflow.id,
        Default::default(),
        Default::default(),
        crate::core::launch_context::LaunchContext {
            requested_project_id: Some(project.into()),
            ..Default::default()
        },
    )
    .await
    .expect("admitted");
    execute_run(
        state.clone(),
        &wf,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("run");
    run
}

#[tokio::test]
async fn an_approved_script_loads_its_approved_helper_not_the_worktree_one() {
    let (state, _, _) = test_state_and_configs();
    let home = tempfile::TempDir::new().unwrap();
    let target = tempfile::TempDir::new().unwrap();
    write_file(home.path(), "scripts/entry.cjs", SCRIPT_ENTRY);
    write_file(
        home.path(),
        "lib/helper.cjs",
        "module.exports = 'APPROVED';\n",
    );
    // The run's checkout holds modified copies at the same paths.
    write_file(
        target.path(),
        "scripts/entry.cjs",
        "console.log('MUTATED ENTRY');\n",
    );
    write_file(
        target.path(),
        "lib/helper.cjs",
        "module.exports = 'MUTATED';\n",
    );
    insert_project_at(&state, "proj-kt918-home", home.path()).await;
    insert_project_at(&state, "proj-kt918-target", target.path()).await;
    let workflow = create_script_workflow(
        &state,
        "proj-kt918-home",
        Some(serde_json::json!({"type": "Projects", "project_ids": ["proj-kt918-target"]})),
        &["scripts/entry.cjs", "lib/helper.cjs"],
    )
    .await
    .expect("saved");
    let pinned = &workflow.steps[0].exec_script_files;
    assert!(
        pinned.iter().all(|file| file.sha256.len() == 64),
        "{pinned:?}"
    );

    let run = run_scripts(&state, &workflow, "proj-kt918-target").await;

    let output = &run.step_results.last().unwrap().output;
    assert_eq!(run.status, RunStatus::Success, "{output}");
    assert!(output.contains("SENTINEL=APPROVED"), "{output}");
    assert!(!output.contains("MUTATED"), "{output}");
    assert!(
        output.contains(&format!("WT={}", target.path().display())),
        "{output}"
    );
    assert_eq!(
        std::fs::read_to_string(target.path().join("ran")).unwrap(),
        "APPROVED"
    );
}

#[tokio::test]
async fn a_one_line_change_to_the_script_or_a_module_blocks_the_run_and_names_the_file() {
    let (state, _, _) = test_state_and_configs();
    let home = tempfile::TempDir::new().unwrap();
    write_file(home.path(), "scripts/entry.cjs", SCRIPT_ENTRY);
    write_file(
        home.path(),
        "lib/helper.cjs",
        "module.exports = 'APPROVED';\n",
    );
    insert_project_at(&state, "proj-kt918-drift", home.path()).await;
    let workflow = create_script_workflow(
        &state,
        "proj-kt918-drift",
        None,
        &["scripts/entry.cjs", "lib/helper.cjs"],
    )
    .await
    .expect("saved");

    write_file(
        home.path(),
        "lib/helper.cjs",
        "module.exports = 'APPROVED';\n// x\n",
    );
    let run = run_scripts(&state, &workflow, "proj-kt918-drift").await;
    let output = &run.step_results.last().unwrap().output;
    assert_eq!(run.status, RunStatus::Failed, "{output}");
    assert!(
        output.contains("`lib/helper.cjs` changed since approval"),
        "{output}"
    );
    assert!(
        !home.path().join("ran").exists(),
        "the step must not have run"
    );

    write_file(
        home.path(),
        "lib/helper.cjs",
        "module.exports = 'APPROVED';\n",
    );
    write_file(
        home.path(),
        "scripts/entry.cjs",
        &format!("{SCRIPT_ENTRY}// x\n"),
    );
    let run = run_scripts(&state, &workflow, "proj-kt918-drift").await;
    let output = &run.step_results.last().unwrap().output;
    assert_eq!(run.status, RunStatus::Failed, "{output}");
    assert!(
        output.contains("`scripts/entry.cjs` changed since approval"),
        "{output}"
    );
    assert!(
        !home.path().join("ran").exists(),
        "the step must not have run"
    );

    // Restored content matches the approval again.
    write_file(home.path(), "scripts/entry.cjs", SCRIPT_ENTRY);
    let run = run_scripts(&state, &workflow, "proj-kt918-drift").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
}

#[tokio::test]
async fn a_script_saved_by_an_agent_does_not_run_until_a_human_approves_it() {
    let (state, _, _) = test_state_and_configs();
    let home = tempfile::TempDir::new().unwrap();
    write_file(home.path(), "scripts/entry.cjs", SCRIPT_ENTRY);
    write_file(
        home.path(),
        "lib/helper.cjs",
        "module.exports = 'APPROVED';\n",
    );
    insert_project_at(&state, "proj-kt918-agent", home.path()).await;
    let request: crate::models::CreateWorkflowRequest = serde_json::from_value(serde_json::json!({
        "name": "agent-draft", "project_id": "proj-kt918-agent",
        "trigger": {"type": "Manual"}, "exec_allowlist": ["node"],
        "steps": [{"name": "framing", "step_type": {"type": "Exec"},
            "agent": "ClaudeCode", "prompt_template": "", "exec_command": "node",
            "exec_args": ["scripts/entry.cjs"],
            "exec_script_files": [{"path": "scripts/entry.cjs"}, {"path": "lib/helper.cjs"}]}],
    }))
    .unwrap();
    let response = crate::api::workflows::create_as(
        state.clone(),
        request,
        crate::api::workflows::WorkflowWriter::Agent,
    )
    .await
    .0;
    let workflow = response.data.expect("saved");
    assert!(workflow.steps[0]
        .exec_script_files
        .iter()
        .all(|file| file.sha256.is_empty()));
    // An agent's workflow lands disabled (KT-1037); a human enables it.
    assert!(!workflow.enabled);
    let enable: crate::models::UpdateWorkflowRequest =
        serde_json::from_value(serde_json::json!({"enabled": true})).unwrap();
    let workflow = crate::api::workflows::update_as(
        state.clone(),
        workflow.id.clone(),
        enable,
        crate::api::workflows::WorkflowWriter::Human,
    )
    .await
    .0
    .data
    .expect("enabled by a human");

    let run = run_scripts(&state, &workflow, "proj-kt918-agent").await;
    let output = &run.step_results.last().unwrap().output;
    assert_eq!(run.status, RunStatus::Failed, "{output}");
    assert!(output.contains("no approved hash"), "{output}");
    assert!(
        !home.path().join("ran").exists(),
        "the step must not have run"
    );
}

#[tokio::test]
async fn saving_refuses_a_script_outside_the_repository_or_behind_an_escaping_symlink() {
    let (state, _, _) = test_state_and_configs();
    let home = tempfile::TempDir::new().unwrap();
    let outside = tempfile::TempDir::new().unwrap();
    write_file(home.path(), "scripts/entry.cjs", SCRIPT_ENTRY);
    write_file(outside.path(), "secret.cjs", "x");
    insert_project_at(&state, "proj-kt918-save", home.path()).await;

    let error = create_script_workflow(&state, "proj-kt918-save", None, &["../secret.cjs"])
        .await
        .unwrap_err();
    assert!(error.contains("relative to the repository"), "{error}");
    let error = create_script_workflow(&state, "proj-kt918-save", None, &["scripts/missing.cjs"])
        .await
        .unwrap_err();
    assert!(error.contains("scripts/missing.cjs"), "{error}");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            outside.path().join("secret.cjs"),
            home.path().join("scripts/leak.cjs"),
        )
        .unwrap();
        let error = create_script_workflow(&state, "proj-kt918-save", None, &["scripts/leak.cjs"])
            .await
            .unwrap_err();
        assert!(error.contains("outside the repository"), "{error}");
    }
    let request: crate::models::CreateWorkflowRequest = serde_json::from_value(serde_json::json!({
        "name": "no-project", "project_id": null, "trigger": {"type": "Manual"},
        "exec_allowlist": ["node"],
        "steps": [{"name": "s", "step_type": {"type": "Exec"}, "agent": "ClaudeCode",
            "prompt_template": "", "exec_command": "node",
            "exec_script_files": [{"path": "scripts/entry.cjs"}]}],
    }))
    .unwrap();
    let response = crate::api::workflows::create_as(
        state.clone(),
        request,
        crate::api::workflows::WorkflowWriter::Human,
    )
    .await
    .0;
    assert!(!response.success);
    assert!(
        response
            .error
            .unwrap_or_default()
            .contains("need a project"),
        "a script needs a carrying repository"
    );
}

#[tokio::test]
async fn the_step_editor_reads_each_file_state() {
    let (state, _, _) = test_state_and_configs();
    let home = tempfile::TempDir::new().unwrap();
    write_file(home.path(), "scripts/entry.cjs", SCRIPT_ENTRY);
    write_file(home.path(), "lib/helper.cjs", "module.exports = 1;\n");
    insert_project_at(&state, "proj-kt918-status", home.path()).await;
    let workflow = create_script_workflow(
        &state,
        "proj-kt918-status",
        None,
        &["scripts/entry.cjs", "lib/helper.cjs"],
    )
    .await
    .expect("saved");
    write_file(home.path(), "lib/helper.cjs", "module.exports = 2;\n");
    let mut files = workflow.steps[0].exec_script_files.clone();
    files.push(crate::models::ExecScriptFile {
        path: "scripts/new.cjs".into(),
        sha256: String::new(),
    });
    let request = crate::models::ExecScriptStatusRequest {
        project_id: Some("proj-kt918-status".into()),
        files,
    };
    let response = crate::api::workflows::exec_script_status(
        axum::extract::State(state.clone()),
        axum::Json(request),
    )
    .await
    .0;
    let states: Vec<_> = response
        .data
        .unwrap()
        .into_iter()
        .map(|status| (status.path, status.state))
        .collect();
    use crate::models::ExecScriptFileState::*;
    assert_eq!(
        states,
        vec![
            ("scripts/entry.cjs".to_string(), Approved),
            ("lib/helper.cjs".to_string(), Changed),
            ("scripts/new.cjs".to_string(), Invalid),
        ]
    );
}

#[path = "runner/tests/gate_guard_runs.rs"]
mod gate_guard_runs;
#[path = "runner/tests/native_full_access_runs.rs"]
mod native_full_access_runs;
#[path = "runner/tests/quota_recheck_runs.rs"]
mod quota_recheck_runs;
#[path = "runner/tests/quota_wait_foreach_runs.rs"]
mod quota_wait_foreach_runs;
#[path = "runner/tests/quota_wait_runs.rs"]
mod quota_wait_runs;
#[path = "runner/tests/quota_wait_subworkflow_runs.rs"]
mod quota_wait_subworkflow_runs;
#[path = "runner/tests/run_effect_runs.rs"]
mod run_effect_runs;
#[path = "runner/tests/run_pin_runs.rs"]
mod run_pin_runs;
#[path = "runner/tests/step_shadow_runs.rs"]
mod step_shadow_runs;
#[path = "runner/tests/task_board_runs.rs"]
mod task_board_runs;
#[path = "runner/tests/workflow_safety_runs.rs"]
mod workflow_safety_runs;

#[tokio::test]
async fn project_linked_fire_and_forget_batch_runs_without_parent_worktree() {
    // KT-343 composition regression: the runner used to create a worktree
    // for every project-linked workflow, even when isolation was disabled.
    // BatchQuickPrompt correctly rejects fire-and-forget from a parent
    // worktree because the parent may delete it while children still run.
    // Driving the real runner against a real git repo locks both contracts
    // together; reverting to unconditional isolation makes this test fail.
    let (state, tokens, agents) = test_state_and_configs();
    let repo = tempfile::TempDir::new().unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args([
                "-c",
                "user.email=test@kronn.local",
                "-c",
                "user.name=Kronn Test",
            ])
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q"]);
    std::fs::write(repo.path().join("README.md"), "fixture\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "fixture"]);

    let now = chrono::Utc::now();
    let project: Project = serde_json::from_value(serde_json::json!({
        "id": "proj-fanout", "name": "Fanout fixture",
        "path": repo.path().to_string_lossy(),
        "repo_url": null, "token_override": null,
        "ai_config": {"detected": false, "configs": []},
        "created_at": now.to_rfc3339(), "updated_at": now.to_rfc3339(),
    }))
    .unwrap();
    let qp = QuickPrompt {
        id: "qp-fanout".into(),
        pinned: false,
        name: "Read-only analysis".into(),
        icon: "🔎".into(),
        prompt_template: "Analyse {{ticket}}".into(),
        variables: vec![PromptVariable {
            name: "ticket".into(),
            label: "Ticket".into(),
            placeholder: "KT-1".into(),
            description: None,
            required: true,
            pattern: None,
            source: Default::default(),
            source_ref: None,
            allow_manual_override: false,
            control: None,
        }],
        agent: AgentType::ClaudeCode,
        connection_id: None,
        project_id: Some(project.id.clone()),
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        tier: ModelTier::Default,
        agent_settings: None,
        description: "Composition fixture".into(),
        created_at: now,
        updated_at: now,
    };

    let project_db = project.clone();
    let qp_db = qp.clone();
    state
        .db
        .with_conn(move |conn| {
            crate::db::projects::insert_project(conn, &project_db)?;
            crate::db::quick_prompts::insert_quick_prompt(conn, &qp_db)?;
            Ok(())
        })
        .await
        .unwrap();

    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-fanout".into();
    workflow.name = "Read-only project fanout".into();
    workflow.project_id = Some(project.id);
    workflow.workspace_config = None;
    let mut fanout = fake_step("fanout");
    fanout.step_type = StepType::BatchQuickPrompt;
    fanout.batch_quick_prompt_id = Some(qp.id);
    fanout.batch_items_from = Some(r#"["KT-1"]"#.into());
    fanout.batch_wait_for_completion = Some(false);
    fanout.batch_workspace_mode = Some("Direct".into());
    workflow.steps = vec![fanout];

    let mut run = pending_run("run-fanout", &workflow.id);
    insert_wf_and_run(&state, &workflow, &run).await;
    execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("read-only project fan-out must execute");

    assert_eq!(run.status, RunStatus::Success);
    assert_eq!(run.workspace_path, None, "isolation is not implicit");
    let parent_id = run.id.clone();
    let (child_runs, discussions) = state
            .db
            .with_conn(move |conn| -> anyhow::Result<(i64, i64)> {
                let child_runs = conn.query_row(
                    "SELECT COUNT(*) FROM workflow_runs WHERE parent_run_id = ?1",
                    [&parent_id],
                    |row| row.get(0),
                )?;
                let discussions = conn.query_row(
                    "SELECT COUNT(*) FROM discussions WHERE workflow_run_id IN (SELECT id FROM workflow_runs WHERE parent_run_id = ?1)",
                    [&parent_id],
                    |row| row.get(0),
                )?;
                Ok((child_runs, discussions))
            })
            .await
            .unwrap();
    assert_eq!(child_runs, 1, "one durable child batch run is created");
    assert_eq!(discussions, 1, "one fire-and-forget discussion is queued");
}

/// A project cloned from an upstream that then gained a commit, and an
/// isolated workflow starting from `origin/main` whose only step prints the
/// worktree's HEAD. Returns (dirs, clone, upstream tip, workflow).
async fn base_ref_fixture(
    state: &crate::AppState,
    suffix: &str,
) -> (Vec<tempfile::TempDir>, std::path::PathBuf, String, Workflow) {
    let upstream_dir = tempfile::TempDir::new().unwrap();
    let upstream = upstream_dir.path().to_path_buf();
    git_in(&upstream, &["init", "-q", "-b", "main"]).await;
    git_in(&upstream, &["config", "user.email", "test@kronn.local"]).await;
    git_in(&upstream, &["config", "user.name", "test"]).await;
    std::fs::write(upstream.join("README.md"), "v1\n").unwrap();
    git_in(&upstream, &["add", "."]).await;
    git_in(&upstream, &["commit", "-q", "-m", "init"]).await;
    let clone_dir = tempfile::TempDir::new().unwrap();
    let clone = clone_dir.path().join("project");
    git_in(
        clone_dir.path(),
        &["clone", "-q", &upstream.to_string_lossy(), "project"],
    )
    .await;
    std::fs::write(upstream.join("README.md"), "v2\n").unwrap();
    git_in(&upstream, &["commit", "-q", "-am", "upstream moved on"]).await;
    let remote_tip = git_in(&upstream, &["rev-parse", "HEAD"]).await;

    let now = chrono::Utc::now();
    let project: Project = serde_json::from_value(serde_json::json!({
        "id": format!("proj-base-ref-{suffix}"), "name": "Base ref fixture",
        "path": clone.to_string_lossy(),
        "repo_url": null, "token_override": null,
        "ai_config": {"detected": false, "configs": []},
        "created_at": now.to_rfc3339(), "updated_at": now.to_rfc3339(),
    }))
    .unwrap();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = format!("wf-base-ref-{suffix}");
    workflow.name = format!("base-ref-{suffix}");
    workflow.project_id = Some(project.id.clone());
    workflow.workspace_config = Some(WorkspaceConfig {
        hooks: WorkspaceHooks::default(),
        require_isolation: true,
        main_tree_read_only: false,
        base_ref: Some("origin/main".into()),
    });
    workflow.exec_allowlist = vec!["git".into()];
    let mut head = fake_step("head");
    head.step_type = StepType::Exec;
    head.exec_command = Some("git".into());
    head.exec_args = vec!["rev-parse".into(), "HEAD".into()];
    workflow.steps = vec![head];
    state
        .db
        .with_conn(move |conn| crate::db::projects::insert_project(conn, &project))
        .await
        .unwrap();
    (vec![upstream_dir, clone_dir], clone, remote_tip, workflow)
}

#[tokio::test]
async fn an_isolated_run_with_base_ref_starts_from_the_remote_tip() {
    let (state, tokens, agents) = test_state_and_configs();
    let (_dirs, clone, remote_tip, workflow) = base_ref_fixture(&state, "tip").await;
    let local_head = git_in(&clone, &["rev-parse", "HEAD"]).await;
    assert_ne!(local_head, remote_tip, "the project is behind its remote");

    let mut run = pending_run("run-base-ref-tip", &workflow.id);
    insert_wf_and_run(&state, &workflow, &run).await;
    execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let output = &run.step_results[0].output;
    assert!(
        output.contains(&remote_tip),
        "the step ran on the remote tip: {output}"
    );
    assert!(!output.contains(&local_head), "{output}");
    assert!(
        run.workspace_path
            .as_deref()
            .is_some_and(|path| path.contains(".kronn/worktrees")),
        "it ran in its own worktree"
    );
}

#[tokio::test]
async fn a_base_ref_run_refuses_rather_than_fall_back_when_the_fetch_fails() {
    let (state, tokens, agents) = test_state_and_configs();
    let (dirs, clone, _tip, mut workflow) = base_ref_fixture(&state, "offline").await;
    let gone = dirs[0].path().join("unreachable");
    git_in(
        &clone,
        &["remote", "set-url", "origin", &gone.to_string_lossy()],
    )
    .await;
    // Even without require_isolation, the main checkout is not at the base.
    workflow
        .workspace_config
        .as_mut()
        .unwrap()
        .require_isolation = false;

    let mut run = pending_run("run-base-ref-offline", &workflow.id);
    insert_wf_and_run(&state, &workflow, &run).await;
    execute_run(
        state.clone(),
        &workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(
        run.step_results.len(),
        1,
        "no step ran: {:?}",
        run.step_results
    );
    let refusal = &run.step_results[0];
    assert_eq!(refusal.step_name, "__workspace__");
    assert!(
        refusal.output.contains("git fetch origin main"),
        "{}",
        refusal.output
    );
    assert!(
        refusal.output.contains("does not start from `origin/main`"),
        "{}",
        refusal.output
    );
    assert_eq!(run.workspace_path, None);
}
