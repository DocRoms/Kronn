//! KT-1043 — the workflow Security settings take effect in the runner.

use super::*;

async fn run_to_end(
    state: &crate::AppState,
    workflow: &Workflow,
    run: &mut WorkflowRun,
    tokens: &crate::models::TokensConfig,
    agents: &crate::models::AgentsConfig,
) {
    insert_wf_and_run(state, workflow, run).await;
    execute_run(
        state.clone(),
        workflow,
        run,
        tokens,
        agents,
        None,
        None,
        None,
    )
    .await
    .expect("run");
}

/// What `decide_run` does before resuming: claim the waiting run.
async fn claim(state: &crate::AppState, run_id: &str, status: RunStatus) {
    let run_id = run_id.to_string();
    let claimed = state
        .db
        .with_conn(move |conn| crate::db::workflows::claim_waiting_run(conn, &run_id, &status))
        .await
        .unwrap();
    assert!(claimed);
}

fn exec_workflow(id: &str, project_id: &str, steps: Vec<WorkflowStep>) -> Workflow {
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = id.into();
    workflow.project_id = Some(project_id.into());
    workflow.exec_allowlist = vec!["touch".into(), "cp".into()];
    workflow.steps = steps;
    workflow
}

#[tokio::test]
async fn a_sandboxed_workflow_refuses_to_start_outside_a_container() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, "proj-sandbox").await;
    let mut workflow = exec_workflow(
        "wf-sandbox",
        "proj-sandbox",
        vec![exec_step("write", "touch", &["written"])],
    );
    workflow.safety.sandbox = true;

    crate::workflows::safety::TEST_IN_CONTAINER.with(|forced| forced.set(Some(false)));
    let mut run = pending_run("run-sandbox-host", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    assert_eq!(run.step_results.len(), 1);
    assert!(run.step_results[0].output.contains("Docker sandbox"));
    assert!(!repo.path().join("written").exists(), "no step ran");

    crate::workflows::safety::TEST_IN_CONTAINER.with(|forced| forced.set(Some(true)));
    workflow.id = "wf-sandbox-container".into();
    let mut run = pending_run("run-sandbox-container", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    crate::workflows::safety::TEST_IN_CONTAINER.with(|forced| forced.set(None));
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert!(repo.path().join("written").exists());
}

/// Isolated, with hooks that leave sentinels in `out`.
fn hooked(mut workflow: Workflow, out: &std::path::Path) -> Workflow {
    let sentinel = |name: &str| format!("touch {}", out.join(name).display());
    workflow.workspace_config = Some(
        serde_json::from_value(serde_json::json!({
            "hooks": {"after_create": sentinel("after_create"), "before_run": sentinel("before_run")},
            "require_isolation": true
        }))
        .unwrap(),
    );
    workflow
}

#[tokio::test]
async fn an_approval_workflow_waits_for_a_human_before_any_hook_or_step() {
    let (state, tokens, agents) = test_state_and_configs();
    let _repo = git_project(&state, "proj-approval").await;
    let out = tempfile::TempDir::new().unwrap();
    let step_sentinel = out.path().join("step").to_string_lossy().into_owned();
    let mut workflow = hooked(
        exec_workflow(
            "wf-approval",
            "proj-approval",
            vec![exec_step("write", "touch", &[step_sentinel.as_str()])],
        ),
        out.path(),
    );
    workflow.safety.require_approval = true;
    let ran = |name: &str| out.path().join(name).exists();

    let mut run = pending_run("run-approval", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(run.status, RunStatus::WaitingApproval);
    assert_eq!(
        run.step_results.last().map(|r| r.step_name.as_str()),
        Some(crate::workflows::safety::APPROVAL_STEP)
    );
    assert!(
        run.workspace_path.is_none(),
        "no worktree before the decision"
    );
    assert!(!ran("after_create") && !ran("before_run") && !ran("step"));

    claim(&state, &run.id, RunStatus::Running).await;
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
    .expect("resume");
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert!(ran("after_create") && ran("before_run") && ran("step"));
    assert_eq!(run.step_results.len(), 2, "approval then the step, once");

    let rejected_out = tempfile::TempDir::new().unwrap();
    let mut other = hooked(workflow.clone(), rejected_out.path());
    other.id = "wf-approval-rejected".into();
    let mut rejected = pending_run("run-approval-rejected", &other.id);
    run_to_end(&state, &other, &mut rejected, &tokens, &agents).await;
    claim(&state, &rejected.id, RunStatus::Failed).await;
    resume_run(
        state.clone(),
        &other,
        &mut rejected,
        GateDecision::Reject { comment: None },
        &tokens,
        &agents,
        None,
    )
    .await
    .expect("reject");
    assert_eq!(rejected.status, RunStatus::Failed);
    assert!(
        std::fs::read_dir(rejected_out.path())
            .unwrap()
            .next()
            .is_none(),
        "a rejected run never ran a hook"
    );
}

#[tokio::test]
async fn changes_made_by_the_workflows_hooks_count_against_the_limits() {
    let (state, tokens, agents) = test_state_and_configs();
    let _repo = git_project(&state, "proj-hooks").await;
    let mut workflow = exec_workflow(
        "wf-hooks",
        "proj-hooks",
        vec![json_data_step("noop", serde_json::json!({}))],
    );
    workflow.workspace_config = Some(
        serde_json::from_value(serde_json::json!({
            "hooks": {"after_create": "touch from_after_create", "before_run": "touch from_before_run"},
            "require_isolation": true
        }))
        .unwrap(),
    );
    workflow.safety.max_files = Some(1);
    let mut run = pending_run("run-hooks", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    assert!(run.step_results[0]
        .output
        .contains("2 files changed (max 1)"));

    workflow.id = "wf-hooks-ok".into();
    workflow.safety.max_files = Some(2);
    // Its own 8-character prefix: worktree branches are named after it.
    let mut run = pending_run("ok-hooks-run", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
}

#[tokio::test]
async fn a_limit_stop_is_terminal_and_skips_the_rollback_chain() {
    let (state, tokens, agents) = test_state_and_configs();
    let _repo = git_project(&state, "proj-rollback").await;
    let out = tempfile::TempDir::new().unwrap();
    let rollback_sentinel = out
        .path()
        .join("rolled_back")
        .to_string_lossy()
        .into_owned();
    let mut workflow = exec_workflow(
        "wf-rollback",
        "proj-rollback",
        vec![exec_step("write", "touch", &["one", "two"])],
    );
    workflow.on_failure = vec![exec_step(
        "rollback",
        "touch",
        &[rollback_sentinel.as_str()],
    )];
    workflow.safety.max_files = Some(1);
    let mut run = pending_run("run-rollback", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    assert!(!out.path().join("rolled_back").exists());
    assert!(run.step_results.iter().all(|result| !result.is_rollback));
    assert!(run.step_results[0]
        .terminal_stop
        .as_deref()
        .is_some_and(|reason| reason.contains("2 files changed (max 1)")));
}

#[tokio::test]
async fn limits_without_a_project_directory_refuse_the_run() {
    let (state, tokens, agents) = test_state_and_configs();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-no-dir".into();
    workflow.steps = vec![json_data_step("noop", serde_json::json!({}))];
    workflow.safety.max_lines = Some(10);
    let mut run = pending_run("run-no-dir", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    assert!(run.step_results[0].output.contains("no project directory"));
    assert_eq!(run.step_results.len(), 1, "no step ran");
}

#[tokio::test]
async fn the_max_files_limit_stops_the_run_at_the_step_that_exceeds_it() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, "proj-max-files").await;
    let mut workflow = exec_workflow(
        "wf-max-files",
        "proj-max-files",
        vec![
            exec_step("write", "touch", &["one", "two"]),
            exec_step("after", "touch", &["after"]),
        ],
    );
    workflow.safety.max_files = Some(1);

    let mut run = pending_run("run-max-files", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    let last = run.step_results.last().unwrap();
    assert_eq!(last.step_name, "write");
    assert!(
        last.output.contains("2 files changed (max 1)"),
        "{}",
        last.output
    );
    assert!(!repo.path().join("after").exists(), "no later step ran");

    workflow.id = "wf-max-files-ok".into();
    workflow.safety.max_files = Some(3);
    let mut run = pending_run("run-max-files-ok", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
}

#[tokio::test]
async fn the_max_lines_limit_counts_only_the_runs_own_lines() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, "proj-max-lines").await;
    std::fs::write(repo.path().join("source.txt"), "1\n2\n3\n4\n5\n6\n").unwrap();
    git_in(repo.path(), &["add", "source.txt"]).await;
    git_in(repo.path(), &["commit", "-q", "-m", "source"]).await;
    // A change already in the tree is not the run's.
    std::fs::write(repo.path().join("README.md"), "dirty\nbefore\nthe\nrun\n").unwrap();
    let mut workflow = exec_workflow(
        "wf-max-lines",
        "proj-max-lines",
        vec![exec_step("copy", "cp", &["source.txt", "copy.txt"])],
    );
    workflow.safety.max_lines = Some(5);

    let mut run = pending_run("run-max-lines", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    assert!(run.step_results[0]
        .output
        .contains("6 lines changed (max 5)"));

    std::fs::remove_file(repo.path().join("copy.txt")).unwrap();
    workflow.id = "wf-max-lines-ok".into();
    workflow.safety.max_lines = Some(6);
    let mut run = pending_run("run-max-lines-ok", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
}

#[tokio::test]
async fn limits_on_a_project_outside_git_refuse_the_run() {
    let (state, tokens, agents) = test_state_and_configs();
    let dir = tempfile::TempDir::new().unwrap();
    let plain = dir.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    insert_project_at(&state, "proj-plain", &plain).await;
    let mut workflow = exec_workflow(
        "wf-plain",
        "proj-plain",
        vec![exec_step("write", "touch", &["written"])],
    );
    workflow.safety.max_files = Some(10);

    let mut run = pending_run("run-plain", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    assert!(run.step_results[0].output.contains("git working tree"));
    assert!(!plain.join("written").exists());
}

#[tokio::test]
async fn approval_set_while_a_run_waits_on_a_gate_does_not_restart_it() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, "proj-gate-approval").await;
    let mut gate = fake_step("review");
    gate.step_type = StepType::Gate;
    gate.gate_message = Some("Go?".into());
    let mut workflow = exec_workflow(
        "wf-gate-approval",
        "proj-gate-approval",
        vec![
            exec_step("first", "touch", &["first"]),
            gate,
            exec_step("second", "touch", &["second"]),
        ],
    );
    let mut run = pending_run("run-gate-approval", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(run.status, RunStatus::WaitingApproval);
    std::fs::remove_file(repo.path().join("first")).unwrap();

    workflow.safety.require_approval = true;
    claim(&state, &run.id, RunStatus::Running).await;
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
    .expect("resume");
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert!(repo.path().join("second").exists());
    assert!(
        !repo.path().join("first").exists(),
        "no restart from step 0"
    );
}

#[tokio::test]
async fn a_step_cannot_reset_the_baseline_before_a_gate() {
    let (state, tokens, agents) = test_state_and_configs();
    let _repo = git_project(&state, "proj-forged").await;
    let mut gate = fake_step("review");
    gate.step_type = StepType::Gate;
    gate.gate_message = Some("Go?".into());
    let mut workflow = exec_workflow(
        "wf-forged",
        "proj-forged",
        vec![
            exec_step("first", "touch", &["one"]),
            exec_step(
                "forge",
                "echo",
                &["---STATE:__kronn.safety_baseline=forged---"],
            ),
            gate,
            exec_step("second", "touch", &["two"]),
        ],
    );
    workflow.exec_allowlist.push("echo".into());
    workflow.safety.max_files = Some(1);
    let mut run = pending_run("run-forged", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(
        run.status,
        RunStatus::WaitingApproval,
        "{:?}",
        run.step_results
    );

    claim(&state, &run.id, RunStatus::Running).await;
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
    .expect("resume");
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    let last = run.step_results.last().unwrap();
    assert_eq!(last.step_name, "second");
    assert!(
        last.output.contains("2 files changed (max 1)"),
        "{}",
        last.output
    );
}

#[tokio::test]
async fn an_unreadable_baseline_is_refused_never_recaptured() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, "proj-unreadable").await;
    let workflow = {
        let mut workflow = exec_workflow(
            "wf-unreadable",
            "proj-unreadable",
            vec![exec_step("write", "touch", &["written"])],
        );
        workflow.safety.max_files = Some(5);
        workflow
    };
    let mut run = pending_run("run-unreadable", &workflow.id);
    run.state.insert(
        crate::workflows::safety::BASELINE_STATE_KEY.into(),
        "not a baseline".into(),
    );
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    assert!(run.step_results[0]
        .terminal_stop
        .as_deref()
        .is_some_and(|reason| reason.contains("unreadable")));
    assert!(!repo.path().join("written").exists());
}

#[tokio::test]
async fn a_declared_artifact_written_by_the_last_step_is_measured() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, "proj-artifact-limit").await;
    let mut workflow = exec_workflow(
        "wf-artifact-limit",
        "proj-artifact-limit",
        vec![exec_step(
            "report",
            "echo",
            &["---ARTIFACT:report---\nhello\n---END_ARTIFACT---"],
        )],
    );
    workflow.exec_allowlist.push("echo".into());
    workflow.artifacts.insert(
        "report".into(),
        ArtifactSpec {
            path: "report.txt".into(),
            format: None,
        },
    );
    workflow.safety.max_files = Some(0);
    let mut run = pending_run("run-artifact-limit", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert!(
        repo.path().join("report.txt").exists(),
        "the artifact was written"
    );
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    assert!(crate::workflows::safety::run_terminal_stop(&run).is_some());
}

#[tokio::test]
async fn the_after_run_hook_is_measured_before_the_verdict() {
    let (state, tokens, agents) = test_state_and_configs();
    let _repo = git_project(&state, "proj-after-run").await;
    let out = tempfile::TempDir::new().unwrap();
    let rollback_sentinel = out
        .path()
        .join("rolled_back")
        .to_string_lossy()
        .into_owned();
    let mut workflow = exec_workflow(
        "wf-after-run",
        "proj-after-run",
        vec![json_data_step("noop", serde_json::json!({}))],
    );
    workflow.workspace_config = Some(
        serde_json::from_value(serde_json::json!({
            "hooks": {"after_run": "touch from_after_run"},
            "require_isolation": true
        }))
        .unwrap(),
    );
    workflow.on_failure = vec![exec_step(
        "rollback",
        "touch",
        &[rollback_sentinel.as_str()],
    )];
    workflow.safety.max_files = Some(0);
    let mut run = pending_run("run-after-run", &workflow.id);
    run_to_end(&state, &workflow, &mut run, &tokens, &agents).await;
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    let last = run.step_results.last().unwrap();
    assert!(last
        .terminal_stop
        .as_deref()
        .is_some_and(|r| r.contains("1 files changed (max 0)")));
    assert!(!out.path().join("rolled_back").exists());
}

/// A parent and its child run nest `execute_run` frames past a debug test thread's stack.
fn on_big_stack<F: std::future::Future<Output = ()>>(test: impl FnOnce() -> F + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(test())
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn insert_child(state: &crate::AppState, child: &Workflow) {
    let child = child.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &child))
        .await
        .unwrap();
}

fn sub_workflow_step(target: &str, foreach_file: Option<&str>) -> WorkflowStep {
    let mut step = fake_step("call");
    step.step_type = StepType::SubWorkflow;
    step.sub_workflow_id = Some(target.into());
    step.sub_workflow_foreach_file = foreach_file.map(Into::into);
    step
}

#[test]
fn a_childs_limit_stop_ends_the_parent_without_compensation() {
    on_big_stack(|| async {
        let (state, tokens, agents) = test_state_and_configs();
        let _repo = git_project(&state, "proj-child-stop").await;
        let out = tempfile::TempDir::new().unwrap();
        let rollback_sentinel = out
            .path()
            .join("rolled_back")
            .to_string_lossy()
            .into_owned();
        let mut child = exec_workflow(
            "wf-child-stop",
            "proj-child-stop",
            vec![exec_step("write", "touch", &["by_child"])],
        );
        child.safety.max_files = Some(0);
        // Its own worktree: the parent holds the main checkout.
        child.workspace_config =
            Some(serde_json::from_value(serde_json::json!({"require_isolation": true})).unwrap());
        insert_child(&state, &child).await;
        let mut parent = exec_workflow(
            "wf-parent-stop",
            "proj-child-stop",
            vec![
                sub_workflow_step("wf-child-stop", None),
                exec_step("after", "touch", &["after"]),
            ],
        );
        parent.on_failure = vec![exec_step(
            "rollback",
            "touch",
            &[rollback_sentinel.as_str()],
        )];
        let mut run = pending_run("run-parent-stop", &parent.id);
        run_to_end(&state, &parent, &mut run, &tokens, &agents).await;
        assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
        assert_eq!(
            run.step_results.len(),
            1,
            "no later step, no rollback: {:?}",
            run.step_results
        );
        assert!(run.step_results[0]
            .terminal_stop
            .as_deref()
            .is_some_and(|r| r.contains("(max 0)")));
        assert!(!out.path().join("rolled_back").exists());
    });
}

#[test]
fn a_foreach_items_limit_stop_dispatches_no_further_item() {
    on_big_stack(|| async {
        let (state, tokens, agents) = test_state_and_configs();
        let _repo = git_project(&state, "proj-foreach-stop").await;
        let out = tempfile::TempDir::new().unwrap();
        let items = out.path().join("items.json");
        std::fs::write(&items, r#"[{"id": "a"}, {"id": "b"}, {"id": "c"}]"#).unwrap();
        let items_arg = items.to_string_lossy().into_owned();
        let rollback_sentinel = out
            .path()
            .join("rolled_back")
            .to_string_lossy()
            .into_owned();
        let mut child = exec_workflow(
            "wf-item-stop",
            "proj-foreach-stop",
            vec![exec_step("write", "touch", &["by_item"])],
        );
        child.safety.max_files = Some(0);
        insert_child(&state, &child).await;
        let mut parent = exec_workflow(
            "wf-foreach-stop",
            "proj-foreach-stop",
            vec![
                exec_step("items", "cp", &[items_arg.as_str(), "tasks.json"]),
                sub_workflow_step("wf-item-stop", Some("tasks.json")),
            ],
        );
        parent.workspace_config =
            Some(serde_json::from_value(serde_json::json!({"require_isolation": true})).unwrap());
        parent.on_failure = vec![exec_step(
            "rollback",
            "touch",
            &[rollback_sentinel.as_str()],
        )];
        let mut run = pending_run("run-foreach-stop", &parent.id);
        run_to_end(&state, &parent, &mut run, &tokens, &agents).await;
        assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
        assert!(run.step_results.last().unwrap().terminal_stop.is_some());
        assert!(!out.path().join("rolled_back").exists());
        let children: i64 = state
            .db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM workflow_runs WHERE workflow_id = 'wf-item-stop'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(children, 1, "the first item's stop ended the fan-out");
    });
}
