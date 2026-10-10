//! KT-1096 — a run executes the revision it pinned at its first execution:
//! after a restart, a Gate approval, and in its sub-workflow children.

use super::*;

async fn pin_now(state: &crate::AppState, workflow: &Workflow, run: &WorkflowRun) {
    let (workflow, run) = (workflow.clone(), run.clone());
    state
        .db
        .with_conn(move |conn| crate::workflows::run_pins::pin_or_load(conn, &workflow, &run))
        .await
        .unwrap()
        .expect("pinned");
}

async fn store_edit(state: &crate::AppState, workflow: &Workflow) {
    let edited = workflow.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::update_workflow_as_agent(conn, &edited))
        .await
        .unwrap();
}

fn output_of(run: &WorkflowRun, step: &str) -> String {
    run.step_results
        .iter()
        .rev()
        .find(|result| result.step_name == step)
        .map(|result| result.output.clone())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_restarted_run_resumes_on_its_pinned_revision() {
    let (state, tokens, agents) = test_state_and_configs();
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-pin-restart".into();
    wf.steps = vec![
        json_data_step("first", serde_json::json!({ "v": "one" })),
        json_data_step("second", serde_json::json!({ "v": "APPROVED" })),
    ];
    let mut run = pending_run("run-pin-restart", &wf.id);
    run.status = RunStatus::Interrupted;
    run.step_results.push(fake_result("first"));
    insert_wf_and_run(&state, &wf, &run).await;
    pin_now(&state, &wf, &run).await;

    let mut edited = wf.clone();
    edited.steps[1] = json_data_step("second", serde_json::json!({ "v": "INJECTED" }));
    store_edit(&state, &edited).await;

    claim_interrupted_run(&state, &mut run, false)
        .await
        .expect("claim");
    resume_interrupted_run(state.clone(), &edited, &mut run, &tokens, &agents, None)
        .await
        .expect("resume");
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let second = output_of(&run, "second");
    assert!(
        second.contains("APPROVED") && !second.contains("INJECTED"),
        "{second}"
    );
    let pinned = state
        .db
        .with_conn(|conn| crate::db::workflow_run_pins::has_pin(conn, "run-pin-restart"))
        .await
        .unwrap();
    assert!(!pinned, "a finished run drops its pin");
}

#[tokio::test]
async fn a_gate_approval_continues_on_the_pinned_revision() {
    let (state, tokens, agents) = test_state_and_configs();
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-pin-gate".into();
    let mut gate = fake_step("review");
    gate.step_type = StepType::Gate;
    wf.steps = vec![
        gate,
        json_data_step("after", serde_json::json!({ "v": "APPROVED" })),
    ];
    let mut run = pending_run("run-pin-gate", &wf.id);
    run.status = RunStatus::Running;
    let mut paused = fake_result("review");
    paused.status = RunStatus::WaitingApproval;
    run.step_results = vec![paused];
    insert_wf_and_run(&state, &wf, &run).await;
    pin_now(&state, &wf, &run).await;

    let mut edited = wf.clone();
    edited.steps[1] = json_data_step("after", serde_json::json!({ "v": "INJECTED" }));
    store_edit(&state, &edited).await;

    resume_run(
        state.clone(),
        &edited,
        &mut run,
        GateDecision::Approve { comment: None },
        &tokens,
        &agents,
        None,
    )
    .await
    .expect("approval resumes");
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let after = output_of(&run, "after");
    assert!(
        after.contains("APPROVED") && !after.contains("INJECTED"),
        "{after}"
    );
}

async fn parent_and_child(state: &crate::AppState, id: &str) -> (Workflow, WorkflowRun) {
    let mut child = make_workflow_with_artifacts(Default::default());
    child.id = format!("{id}-child");
    child.steps = vec![json_data_step(
        "leaf",
        serde_json::json!({ "v": "APPROVED" }),
    )];
    let stored_child = child.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &stored_child))
        .await
        .unwrap();
    let mut parent = make_workflow_with_artifacts(Default::default());
    parent.id = format!("{id}-parent");
    let mut sub = fake_step("sub");
    sub.step_type = StepType::SubWorkflow;
    sub.sub_workflow_id = Some(child.id.clone());
    parent.steps = vec![sub];
    let run = pending_run(&format!("{id}-run"), &parent.id);
    insert_wf_and_run(state, &parent, &run).await;
    pin_now(state, &parent, &run).await;
    (parent, run)
}

#[tokio::test]
async fn a_child_sub_workflow_runs_the_revision_its_parent_pinned() {
    let (state, tokens, agents) = test_state_and_configs();
    let (parent, mut run) = parent_and_child(&state, "pin-child").await;
    // An agent rewrites the child mid-run: the live row is edited and disabled.
    state
        .db
        .with_conn(|conn| {
            let mut child = crate::db::workflows::get_workflow(conn, "pin-child-child")?
                .ok_or_else(|| anyhow::anyhow!("the child workflow is missing"))?;
            child.steps[0].json_data_payload = Some(serde_json::json!({ "v": "INJECTED" }));
            crate::db::workflows::update_workflow_as_agent(conn, &child)?;
            crate::core::resource_refs::disable_workflows(conn, &[child.id.clone()])?;
            crate::db::workflows::mark_auto_disabled(
                conn,
                &child.id,
                crate::models::AutoDisableReason::AgentEdit,
                "codex",
                "steps changed by codex",
            )
        })
        .await
        .unwrap();

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
    .expect("run");
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let child_id = run.step_results[0].child_run_id.clone().expect("child run");
    let child = state
        .db
        .with_conn(move |conn| crate::db::workflows::get_run(conn, &child_id))
        .await
        .unwrap()
        .unwrap();
    let leaf = output_of(&child, "leaf");
    assert!(
        leaf.contains("APPROVED") && !leaf.contains("INJECTED"),
        "{leaf}"
    );
}

#[tokio::test]
async fn a_sub_workflow_deleted_mid_run_fails_its_step_cleanly() {
    let (state, tokens, agents) = test_state_and_configs();
    let (parent, mut run) = parent_and_child(&state, "pin-gone").await;
    state
        .db
        .with_conn(|conn| crate::db::workflows::delete_workflow(conn, "pin-gone-child"))
        .await
        .unwrap();

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
    .expect("a failed step is not an engine error");
    assert_eq!(run.status, RunStatus::Failed);
    let output = output_of(&run, "sub");
    assert!(
        output.contains("was deleted while this run was in progress"),
        "{output}"
    );
}

/// An isolated run whose first step optionally leaves an untracked file, then
/// starts a sub-workflow deleted after the run pinned it.
#[derive(Clone, Copy, PartialEq)]
enum Leftover {
    Clean,
    /// An untracked file in the run's worktree.
    Dirty,
    /// A nested worktree `<run>/.kronn/pr-1995` with a modified tracked file
    /// and an untracked one; the run's own checkout is clean.
    DirtyNested,
    /// The same nested worktree, clean.
    CleanNested,
}

/// Steps that leave `leftover` in the run's worktree.
fn leftover_steps(leftover: Leftover) -> (Vec<WorkflowStep>, Vec<String>) {
    let nested = || {
        vec![exec_step(
            "nest",
            "git",
            &["worktree", "add", "-q", "-b", "pr-1995", ".kronn/pr-1995"],
        )]
    };
    match leftover {
        Leftover::Clean => (
            vec![json_data_step("work", serde_json::json!({ "v": 0 }))],
            vec![],
        ),
        Leftover::Dirty => (
            vec![exec_step("work", "touch", &["wip.txt"])],
            vec!["touch".into()],
        ),
        Leftover::CleanNested => (nested(), vec!["git".into()]),
        Leftover::DirtyNested => {
            let mut steps = nested();
            steps.push(exec_step(
                "edit",
                "bash",
                &[
                    "-c",
                    "echo changed >> .kronn/pr-1995/README.md && echo new > .kronn/pr-1995/wip.txt",
                ],
            ));
            (steps, vec!["git".into(), "bash".into()])
        }
    }
}

fn holds_leftover(repo: &std::path::Path, leftover: Leftover) -> bool {
    kronn_worktrees(repo).iter().any(|path| match leftover {
        Leftover::Dirty => path.join("wip.txt").exists(),
        Leftover::DirtyNested => {
            path.join(".kronn/pr-1995/wip.txt").exists()
                && std::fs::read_to_string(path.join(".kronn/pr-1995/README.md"))
                    .is_ok_and(|text| text.contains("changed"))
        }
        _ => true,
    })
}

async fn run_stopped_by_a_deleted_child(
    id: &str,
    leftover: Leftover,
) -> (WorkflowRun, tempfile::TempDir) {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, &format!("proj-{id}")).await;
    let mut child = make_workflow_with_artifacts(Default::default());
    child.id = format!("{id}-child");
    child.steps = vec![json_data_step("leaf", serde_json::json!({ "v": 1 }))];
    let stored_child = child.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &stored_child))
        .await
        .unwrap();
    let mut parent = make_workflow_with_artifacts(Default::default());
    parent.id = format!("{id}-parent");
    parent.project_id = Some(format!("proj-{id}"));
    parent.workspace_config = Some(WorkspaceConfig {
        hooks: WorkspaceHooks::default(),
        require_isolation: true,
        main_tree_read_only: false,
        base_ref: None,
    });
    let (mut steps, allowlist) = leftover_steps(leftover);
    parent.exec_allowlist = allowlist;
    let mut sub = fake_step("sub");
    sub.step_type = StepType::SubWorkflow;
    sub.sub_workflow_id = Some(child.id.clone());
    steps.push(sub);
    parent.steps = steps;
    let mut run = pending_run(&format!("{id}-run"), &parent.id);
    insert_wf_and_run(&state, &parent, &run).await;
    pin_now(&state, &parent, &run).await;
    let gone = child.id.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::delete_workflow(conn, &gone))
        .await
        .unwrap();

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
    .expect("a failed step is not an engine error");
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    assert!(output_of(&run, "sub").contains("was deleted while this run"));
    let row_id = run.id.clone();
    let stored = state
        .db
        .with_conn(move |conn| crate::db::workflows::get_run(conn, &row_id))
        .await
        .unwrap()
        .unwrap();
    (stored, repo)
}

fn kronn_worktrees(repo: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(repo.join(".kronn").join("worktrees"))
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_failed_run_keeps_a_worktree_holding_uncommitted_work() {
    let (run, repo) = run_stopped_by_a_deleted_child("pin-keep", Leftover::Dirty).await;
    let kept: Vec<_> = kronn_worktrees(repo.path())
        .into_iter()
        .filter(|path| path.join("wip.txt").exists())
        .collect();
    assert_eq!(
        kept.len(),
        1,
        "the worktree and its uncommitted file survive"
    );
    let output = output_of(&run, "sub");
    assert!(output.contains("kept with its branch"), "{output}");
    assert!(output.contains("1 uncommitted change"), "{output}");
    let recorded = run.workspace_path.as_deref().expect("still on the run");
    assert!(output.contains(recorded), "{output}");
}

#[tokio::test]
async fn a_failed_run_still_removes_a_clean_worktree() {
    let (run, repo) = run_stopped_by_a_deleted_child("pin-clean", Leftover::Clean).await;
    assert!(
        kronn_worktrees(repo.path()).is_empty(),
        "{:?}",
        kronn_worktrees(repo.path())
    );
    assert!(!output_of(&run, "sub").contains("kept with its branch"));
}

#[tokio::test]
async fn a_failed_run_keeps_a_dirty_nested_worktree() {
    let (run, repo) = run_stopped_by_a_deleted_child("pin-nested", Leftover::DirtyNested).await;
    assert!(holds_leftover(repo.path(), Leftover::DirtyNested));
    let output = output_of(&run, "sub");
    assert!(output.contains("nested worktree"), "{output}");
}

#[tokio::test]
async fn a_failed_run_still_removes_a_clean_nested_worktree() {
    let (_run, repo) =
        run_stopped_by_a_deleted_child("pin-nested-clean", Leftover::CleanNested).await;
    assert!(
        kronn_worktrees(repo.path()).is_empty(),
        "{:?}",
        kronn_worktrees(repo.path())
    );
}

async fn cancelled_run(id: &str, leftover: Leftover) -> (WorkflowRun, tempfile::TempDir) {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, &format!("proj-{id}")).await;
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = format!("wf-{id}");
    wf.project_id = Some(format!("proj-{id}"));
    wf.workspace_config = Some(WorkspaceConfig {
        hooks: WorkspaceHooks::default(),
        require_isolation: true,
        main_tree_read_only: false,
        base_ref: None,
    });
    let (mut steps, mut allowlist) = leftover_steps(leftover);
    steps.push(exec_step("wait", "sleep", &["2"]));
    steps.push(json_data_step("after", serde_json::json!({ "v": 1 })));
    allowlist.push("sleep".into());
    wf.exec_allowlist = allowlist;
    wf.steps = steps;
    let run_id = format!("run-{id}");
    let mut run = pending_run(&run_id, &wf.id);
    insert_wf_and_run(&state, &wf, &run).await;
    let canceller = {
        let (state, repo, run_id) = (state.clone(), repo.path().to_path_buf(), run_id.clone());
        async move {
            // Cancel once the leftover exists.
            for _ in 0..400 {
                if holds_leftover(&repo, leftover) && !kronn_worktrees(&repo).is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            crate::workflows::cancellation::cancel_run_tree(
                &state,
                &run_id,
                crate::workflows::cancellation::CancellationScope::RunTree,
                "cancelled_by_operator",
            )
            .await
            .unwrap();
        }
    };
    let (result, ()) = tokio::join!(
        execute_run(
            state.clone(),
            &wf,
            &mut run,
            &tokens,
            &agents,
            None,
            None,
            None,
        ),
        canceller
    );
    result.expect("a cancel is not an engine error");
    assert_eq!(run.status, RunStatus::Cancelled, "{:?}", run.step_results);
    (run, repo)
}

#[tokio::test]
async fn a_cancelled_run_keeps_a_worktree_holding_uncommitted_work() {
    let (run, repo) = cancelled_run("cancel-keep", Leftover::Dirty).await;
    assert!(holds_leftover(repo.path(), Leftover::Dirty));
    let last = &run.step_results.last().expect("a step result").output;
    assert!(last.contains("kept with its branch"), "{last}");
}

#[tokio::test]
async fn a_cancelled_run_keeps_a_dirty_nested_worktree() {
    let (run, repo) = cancelled_run("cancel-nested", Leftover::DirtyNested).await;
    assert!(holds_leftover(repo.path(), Leftover::DirtyNested));
    let last = &run.step_results.last().expect("a step result").output;
    assert!(last.contains("nested worktree"), "{last}");
}

// ─── Review round 1: recorded resolutions, rollback hydration ─────────────

/// Answers every prompt at once and records it.
struct Recording(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

#[async_trait::async_trait]
impl crate::acp::AcpTransport for Recording {
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
        crate::acp::AcpSessionTarget::new(crate::acp::AcpAgent::ClaudeCode, "pin-turn")
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
        prompt: &str,
        events: tokio::sync::mpsc::Sender<crate::acp::AcpSessionEvent>,
    ) -> Result<(), crate::acp::AcpError> {
        self.0.lock().unwrap().push(prompt.to_string());
        let _ = events
            .send(crate::acp::AcpSessionEvent::TextDelta("done".into()))
            .await;
        let _ = events.send(crate::acp::AcpSessionEvent::Completed).await;
        Ok(())
    }
    async fn cancel(&self, _: &crate::acp::AcpSessionTarget) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
}

struct AgentFixture {
    state: crate::AppState,
    prompts: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    _repo: tempfile::TempDir,
    _route: crate::agents::runner::test_acp_routes::RouteGuard,
}

async fn agent_fixture(project: &str) -> AgentFixture {
    let (state, _, _) = test_state_and_configs();
    let repo = git_project(&state, project).await;
    let repo_path = repo.path().to_string_lossy().into_owned();
    let work_dir =
        crate::agents::runner::resolve_agent_work_dir(Some(&repo_path), &repo_path).unwrap();
    let prompts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let route = crate::agents::runner::test_acp_routes::route(
        &work_dir,
        std::sync::Arc::new(Recording(prompts.clone())),
    );
    AgentFixture {
        state,
        prompts,
        _repo: repo,
        _route: route,
    }
}

async fn seed_prompt(state: &crate::AppState, id: &str, template: &str, project: &str) {
    let prompt: crate::models::QuickPrompt = serde_json::from_value(serde_json::json!({
        "id": id, "name": id, "icon": "P", "prompt_template": template, "variables": [],
        "agent": "ClaudeCode", "project_id": project,
        "created_at": Utc::now(), "updated_at": Utc::now()
    }))
    .unwrap();
    state
        .db
        .with_conn(move |conn| crate::db::quick_prompts::insert_quick_prompt(conn, &prompt))
        .await
        .unwrap();
}

async fn edit_prompt(state: &crate::AppState, id: &str, template: &str) {
    let (id, template) = (id.to_string(), template.to_string());
    state
        .db
        .with_conn(move |conn| {
            let mut prompt = crate::db::quick_prompts::get_quick_prompt(conn, &id)?
                .ok_or_else(|| anyhow::anyhow!("quick prompt {id} is missing"))?;
            prompt.prompt_template = template;
            crate::db::quick_prompts::update_quick_prompt(conn, &prompt)
        })
        .await
        .unwrap();
}

async fn seed_workflow(state: &crate::AppState, id: &str, name: &str, project: &str) {
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = id.into();
    workflow.name = name.into();
    workflow.project_id = Some(project.into());
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &workflow))
        .await
        .unwrap();
}

#[derive(Clone, Copy)]
enum Resume {
    Gate,
    Restart,
}

#[derive(Clone, Copy)]
enum Change {
    /// The live Quick Prompt no longer names the reference.
    PromptEdited,
    /// The slug now names another workflow.
    SlugReassigned,
}

/// A pinned Agent step whose Quick Prompt names `{{ref:workflow:nightly-triage}}`
/// resumes after `change`: its prompt carries the id resolved at pin time.
async fn resumed_prompt_after(tag: &str, resume: Resume, change: Change) -> (String, String) {
    let project = format!("proj-{tag}");
    let fx = agent_fixture(&project).await;
    let (_, tokens, agents) = test_state_and_configs();
    let target = format!("{tag}-target");
    seed_workflow(&fx.state, &target, "Nightly Triage", &project).await;
    let qp = format!("{tag}-qp");
    seed_prompt(
        &fx.state,
        &qp,
        "PINNED-TEMPLATE target={{ref:workflow:nightly-triage}}",
        &project,
    )
    .await;

    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = format!("{tag}-wf");
    wf.project_id = Some(project.clone());
    let mut agent = fake_step("write");
    agent.prompt_template = String::new();
    agent.quick_prompt_id = Some(qp.clone());
    let mut run = pending_run(&format!("{tag}-run"), &wf.id);
    match resume {
        Resume::Gate => {
            let mut gate = fake_step("review");
            gate.step_type = StepType::Gate;
            wf.steps = vec![gate, agent];
            run.status = RunStatus::Running;
            let mut paused = fake_result("review");
            paused.status = RunStatus::WaitingApproval;
            run.step_results = vec![paused];
        }
        Resume::Restart => {
            wf.steps = vec![
                json_data_step("first", serde_json::json!({ "v": 1 })),
                agent,
            ];
            run.status = RunStatus::Interrupted;
            run.step_results.push(fake_result("first"));
        }
    }
    insert_wf_and_run(&fx.state, &wf, &run).await;
    pin_now(&fx.state, &wf, &run).await;

    match change {
        Change::PromptEdited => {
            edit_prompt(&fx.state, &qp, "LIVE-TEMPLATE without reference").await
        }
        Change::SlugReassigned => {
            let renamed = target.clone();
            fx.state
                .db
                .with_conn(move |conn| {
                    let mut old = crate::db::workflows::get_workflow(conn, &renamed)?
                        .ok_or_else(|| anyhow::anyhow!("workflow {renamed} is missing"))?;
                    old.name = "Renamed Elsewhere".into();
                    crate::db::workflows::update_workflow(conn, &old).map(|_| ())
                })
                .await
                .unwrap();
            seed_workflow(
                &fx.state,
                &format!("{tag}-impostor"),
                "Nightly Triage",
                &project,
            )
            .await;
        }
    }

    match resume {
        Resume::Gate => resume_run(
            fx.state.clone(),
            &wf,
            &mut run,
            GateDecision::Approve { comment: None },
            &tokens,
            &agents,
            None,
        )
        .await
        .expect("approval resumes"),
        Resume::Restart => {
            claim_interrupted_run(&fx.state, &mut run, false)
                .await
                .expect("claim");
            resume_interrupted_run(fx.state.clone(), &wf, &mut run, &tokens, &agents, None)
                .await
                .expect("resume");
        }
    }
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let prompt = fx
        .prompts
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("prompted");
    (prompt, target)
}

fn assert_pinned_resolution(prompt: &str, target: &str) {
    assert!(prompt.contains("PINNED-TEMPLATE"), "{prompt}");
    assert!(prompt.contains(&format!("target={target}")), "{prompt}");
}

#[tokio::test]
async fn a_gate_resume_keeps_template_references_after_the_prompt_is_edited() {
    let (prompt, target) =
        resumed_prompt_after("ref-gate-qp", Resume::Gate, Change::PromptEdited).await;
    assert_pinned_resolution(&prompt, &target);
}

#[tokio::test]
async fn a_gate_resume_keeps_its_resolution_after_a_slug_is_reassigned() {
    let (prompt, target) =
        resumed_prompt_after("ref-gate-slug", Resume::Gate, Change::SlugReassigned).await;
    assert_pinned_resolution(&prompt, &target);
}

#[tokio::test]
async fn a_restart_keeps_template_references_after_the_prompt_is_edited() {
    let (prompt, target) =
        resumed_prompt_after("ref-restart-qp", Resume::Restart, Change::PromptEdited).await;
    assert_pinned_resolution(&prompt, &target);
}

#[tokio::test]
async fn a_restart_keeps_its_resolution_after_a_slug_is_reassigned() {
    let (prompt, target) =
        resumed_prompt_after("ref-restart-slug", Resume::Restart, Change::SlugReassigned).await;
    assert_pinned_resolution(&prompt, &target);
}

#[tokio::test]
async fn a_structured_reference_keeps_the_target_resolved_at_pin_time() {
    let fx = agent_fixture("proj-ref-structured").await;
    let (_, tokens, agents) = test_state_and_configs();
    let project = "proj-ref-structured";
    seed_prompt(&fx.state, "qp-structured-a", "PINNED-STRUCTURED", project).await;
    // The prompt's slug is its name: `ref:prompt:pinned-target`.
    fx.state
        .db
        .with_conn(|conn| {
            let mut prompt = crate::db::quick_prompts::get_quick_prompt(conn, "qp-structured-a")?
                .ok_or_else(|| anyhow::anyhow!("qp-structured-a is missing"))?;
            prompt.name = "Pinned Target".into();
            crate::db::quick_prompts::update_quick_prompt(conn, &prompt)
        })
        .await
        .unwrap();
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-ref-structured".into();
    wf.project_id = Some(project.into());
    let mut gate = fake_step("review");
    gate.step_type = StepType::Gate;
    let mut agent = fake_step("write");
    agent.prompt_template = String::new();
    agent.quick_prompt_id = Some("ref:prompt:pinned-target".into());
    wf.steps = vec![gate, agent];
    let mut run = pending_run("run-ref-structured", &wf.id);
    run.status = RunStatus::Running;
    let mut paused = fake_result("review");
    paused.status = RunStatus::WaitingApproval;
    run.step_results = vec![paused];
    insert_wf_and_run(&fx.state, &wf, &run).await;
    pin_now(&fx.state, &wf, &run).await;

    // The slug now names another prompt, absent from the pin.
    fx.state
        .db
        .with_conn(|conn| {
            let mut prompt = crate::db::quick_prompts::get_quick_prompt(conn, "qp-structured-a")?
                .ok_or_else(|| anyhow::anyhow!("qp-structured-a is missing"))?;
            prompt.name = "Somewhere Else".into();
            crate::db::quick_prompts::update_quick_prompt(conn, &prompt)
        })
        .await
        .unwrap();
    seed_prompt(&fx.state, "qp-structured-b", "IMPOSTOR", project).await;
    fx.state
        .db
        .with_conn(|conn| {
            let mut prompt = crate::db::quick_prompts::get_quick_prompt(conn, "qp-structured-b")?
                .ok_or_else(|| anyhow::anyhow!("qp-structured-b is missing"))?;
            prompt.name = "Pinned Target".into();
            crate::db::quick_prompts::update_quick_prompt(conn, &prompt)
        })
        .await
        .unwrap();

    resume_run(
        fx.state.clone(),
        &wf,
        &mut run,
        GateDecision::Approve { comment: None },
        &tokens,
        &agents,
        None,
    )
    .await
    .expect("approval resumes");
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let prompt = fx
        .prompts
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("prompted");
    assert!(prompt.contains("PINNED-STRUCTURED"), "{prompt}");
    assert!(!prompt.contains("IMPOSTOR"), "{prompt}");
}

#[tokio::test]
async fn a_rollback_agent_step_runs_its_pinned_quick_prompt() {
    let fx = agent_fixture("proj-rollback-qp").await;
    let (_, tokens, agents) = test_state_and_configs();
    seed_prompt(
        &fx.state,
        "qp-rollback",
        "ROLLBACK-PINNED",
        "proj-rollback-qp",
    )
    .await;
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-rollback-qp".into();
    wf.project_id = Some("proj-rollback-qp".into());
    wf.exec_allowlist = vec!["false".into()];
    wf.steps = vec![exec_step("break", "false", &[])];
    let mut compensate = fake_step("compensate");
    compensate.prompt_template = String::new();
    compensate.quick_prompt_id = Some("qp-rollback".into());
    wf.on_failure = vec![compensate];
    let mut run = pending_run("run-rollback-qp", &wf.id);
    insert_wf_and_run(&fx.state, &wf, &run).await;
    pin_now(&fx.state, &wf, &run).await;
    edit_prompt(&fx.state, "qp-rollback", "ROLLBACK-LIVE").await;

    execute_run(
        fx.state.clone(),
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
    assert_eq!(run.status, RunStatus::Failed);
    let prompts = fx.prompts.lock().unwrap().clone();
    let rollback = prompts.last().expect("the rollback step prompted");
    assert!(rollback.contains("ROLLBACK-PINNED"), "{rollback}");
    assert!(prompts.iter().all(|p| !p.contains("ROLLBACK-LIVE")));
}

// ─── KT-1103 — a run renders its dates in the zone it pinned ─────────────

/// Rewrites the zone recorded in `run_id`'s pin, as if it started under it.
async fn set_pinned_zone(state: &crate::AppState, run_id: &str, zone: Option<&str>) {
    let (run_id, zone) = (run_id.to_string(), zone.map(String::from));
    state
        .db
        .with_conn(move |conn| {
            let json = crate::db::workflow_run_pins::get(
                conn,
                &run_id,
                crate::db::workflow_run_pins::RUN_KIND,
                "",
            )?
            .expect("pinned header");
            let mut header: serde_json::Value = serde_json::from_str(&json)?;
            match zone {
                Some(zone) => header["timezone"] = serde_json::json!(zone),
                None => {
                    if let Some(header) = header.as_object_mut() {
                        header.remove("timezone");
                    }
                }
            }
            conn.execute(
                "UPDATE workflow_run_pins SET content_json = ?1
                 WHERE run_id = ?2 AND kind = ?3 AND resource_id = ''",
                rusqlite::params![
                    header.to_string(),
                    run_id,
                    crate::db::workflow_run_pins::RUN_KIND
                ],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

async fn pinned_zone(state: &crate::AppState, run_id: &str) -> Option<chrono_tz::Tz> {
    let run_id = run_id.to_string();
    state
        .db
        .with_conn(move |conn| crate::workflows::run_pins::pinned_timezone(conn, &run_id))
        .await
        .unwrap()
}

/// The prompt an Agent step renders after a Gate resume, for a run anchored
/// at 2026-08-14T22:30Z whose pin records `zone` (the live zone stays UTC).
async fn date_prompt_after_gate(tag: &str, zone: Option<&str>) -> String {
    let project = format!("proj-{tag}");
    let fx = agent_fixture(&project).await;
    let (_, tokens, agents) = test_state_and_configs();
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = format!("{tag}-wf");
    wf.project_id = Some(project.clone());
    let mut gate = fake_step("review");
    gate.step_type = StepType::Gate;
    let mut agent = fake_step("write");
    agent.prompt_template =
        "DATE={{time.now|fmt:date}} FROM={{time.now|floor:day|fmt:rfc3339}}".into();
    wf.steps = vec![gate, agent];
    // The fixed anchor is in the past: keep the timeout guard out of the way.
    wf.guards = Some(crate::models::WorkflowGuards {
        timeout_seconds: Some(10 * 365 * 24 * 3600),
        ..Default::default()
    });
    let mut run = pending_run(&format!("{tag}-run"), &wf.id);
    run.started_at = "2026-08-14T22:30:00Z".parse().unwrap();
    run.status = RunStatus::Running;
    let mut paused = fake_result("review");
    paused.status = RunStatus::WaitingApproval;
    run.step_results = vec![paused];
    insert_wf_and_run(&fx.state, &wf, &run).await;
    pin_now(&fx.state, &wf, &run).await;
    set_pinned_zone(&fx.state, &run.id, zone).await;

    resume_run(
        fx.state.clone(),
        &wf,
        &mut run,
        GateDecision::Approve { comment: None },
        &tokens,
        &agents,
        None,
    )
    .await
    .expect("approval resumes");
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let prompts = fx.prompts.lock().unwrap().clone();
    prompts.last().cloned().expect("prompted")
}

#[tokio::test]
async fn a_gate_resume_renders_dates_in_the_zone_the_run_pinned() {
    // Started under Paris; Settings now say UTC (this binary's live zone).
    let prompt = date_prompt_after_gate("tz-gate-paris", Some("Europe/Paris")).await;
    assert!(prompt.contains("DATE=2026-08-15"), "{prompt}");
    assert!(
        prompt.contains("FROM=2026-08-15T00:00:00.000+02:00"),
        "{prompt}"
    );
}

#[tokio::test]
async fn a_pin_older_than_zones_renders_in_utc() {
    let prompt = date_prompt_after_gate("tz-gate-legacy", None).await;
    assert!(prompt.contains("DATE=2026-08-14"), "{prompt}");
    assert!(prompt.contains("FROM=2026-08-14T00:00:00.000Z"), "{prompt}");
}

#[tokio::test]
async fn a_child_run_inherits_its_parent_pinned_zone() {
    let (state, _, _) = test_state_and_configs();
    let (_, parent_run) = parent_and_child(&state, "tz-child").await;
    // The parent started under Tokyo; the live zone is UTC.
    set_pinned_zone(&state, &parent_run.id, Some("Asia/Tokyo")).await;
    let child_wf = state
        .db
        .with_conn(|conn| crate::db::workflows::get_workflow(conn, "tz-child-child"))
        .await
        .unwrap()
        .unwrap();
    let mut child = pending_run("tz-child-sub", &child_wf.id);
    child.parent_run_id = Some(parent_run.id.clone());
    let stored = child.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_run(conn, &stored))
        .await
        .unwrap();
    pin_now(&state, &child_wf, &child).await;
    assert_eq!(
        pinned_zone(&state, "tz-child-sub").await,
        Some(chrono_tz::Asia::Tokyo)
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_run_pins_the_live_zone_and_a_pre_pin_run_keeps_utc() {
    let (state, _, _) = test_state_and_configs();
    // An Etc alias renders like UTC, so concurrent tests never see a shift.
    crate::core::timezone::apply_with(Some("Etc/UTC"), chrono_tz::UTC);
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "tz-live-wf".into();
    wf.steps = vec![json_data_step("only", serde_json::json!({}))];
    let fresh = pending_run("tz-live-fresh", &wf.id);
    insert_wf_and_run(&state, &wf, &fresh).await;
    pin_now(&state, &wf, &fresh).await;
    // Started before pins existed and already ran a step under UTC.
    let mut started = pending_run("tz-live-started", &wf.id);
    started.step_results.push(fake_result("only"));
    let stored = started.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_run(conn, &stored))
        .await
        .unwrap();
    pin_now(&state, &wf, &started).await;
    let (fresh_zone, started_zone) = (
        pinned_zone(&state, "tz-live-fresh").await,
        pinned_zone(&state, "tz-live-started").await,
    );
    crate::core::timezone::apply_with(None, chrono_tz::UTC);
    assert_eq!(fresh_zone, Some(chrono_tz::Etc::UTC));
    assert_eq!(started_zone, Some(chrono_tz::UTC));
}
