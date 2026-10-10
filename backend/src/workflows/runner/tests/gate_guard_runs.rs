//! KT-1046 — the anti-loop guards stay cumulative across Gate and quota
//! resumes. KT-1042 — a Gate checkpoint never commits the operator's checkout.

use super::quota_wait_runs::{fixture, restarted, stored, Fixture, SESSION_LIMIT};
use super::*;
use crate::models::WorkflowGuards;
use std::sync::{Arc, Mutex};

const LONG: u64 = 3 * 24 * 3600;

fn guards(max_llm_calls: u32, max_revisits: usize) -> Option<WorkflowGuards> {
    Some(WorkflowGuards {
        timeout_seconds: Some(LONG),
        max_llm_calls: Some(max_llm_calls),
        loop_detection_max_revisits: Some(max_revisits),
    })
}

/// `prepare` → [`review` Gate] → `back`, which always loops to `prepare`.
fn loop_steps(with_gate: bool) -> Vec<WorkflowStep> {
    let mut prepare = fake_step("prepare");
    prepare.prompt_template = "PREPARE-MARKER".into();
    // An Exec step ends with `[SIGNAL: OK]`; the fake agent emits no signal.
    let mut back = exec_step("back", "echo", &["again"]);
    back.on_result = vec![StepConditionRule {
        contains: "OK".into(),
        action: ConditionAction::Goto {
            step_name: "prepare".into(),
            max_iterations: None,
        },
    }];
    let mut steps = vec![prepare];
    if with_gate {
        let mut gate = fake_step("review");
        gate.step_type = StepType::Gate;
        steps.push(gate);
    }
    steps.push(back);
    steps
}

/// Runs to the end, approving every Gate pause the way the auto-approve timer
/// does (claim, then resume), optionally across a backend restart each time.
async fn run_approving_gates(fx: &Fixture, run_id: &str, restart: bool) -> (WorkflowRun, usize) {
    run_deciding_gates(fx, run_id, restart, GateDecision::Approve { comment: None }).await
}

async fn run_deciding_gates(
    fx: &Fixture,
    run_id: &str,
    restart: bool,
    decision: GateDecision,
) -> (WorkflowRun, usize) {
    let (_, tokens, agents) = test_state_and_configs();
    let mut run = pending_run(run_id, &fx.workflow.id);
    insert_wf_and_run(&fx.state, &fx.workflow, &run).await;
    execute_run(
        fx.state.clone(),
        &fx.workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let mut approvals = 0;
    for _ in 0..30 {
        let state = if restart {
            restarted(&fx.state)
        } else {
            fx.state.clone()
        };
        if stored(&state, run_id).await.status != RunStatus::WaitingApproval {
            break;
        }
        let id = run_id.to_string();
        assert!(state
            .db
            .with_conn(move |conn| crate::db::workflows::claim_waiting_run(
                conn,
                &id,
                &RunStatus::Running
            ))
            .await
            .unwrap());
        let mut run = stored(&state, run_id).await;
        resume_run(
            state.clone(),
            &fx.workflow,
            &mut run,
            decision.clone(),
            &tokens,
            &agents,
            None,
        )
        .await
        .unwrap();
        approvals += 1;
    }
    (stored(&fx.state, run_id).await, approvals)
}

fn guard_row(run: &WorkflowRun) -> &StepResult {
    let last = run.step_results.last().expect("a guard row");
    assert_eq!(
        run.status,
        RunStatus::StoppedByGuard,
        "{:?}",
        run.step_results
    );
    last
}

#[tokio::test]
async fn a_goto_loop_through_approved_gates_trips_loop_detection_where_it_would_without_one() {
    let plain = {
        let mut fx = fixture("loop-plain", SESSION_LIMIT, 0, LONG).await;
        fx.workflow.guards = guards(100, 3);
        fx.workflow.exec_allowlist = vec!["echo".into()];
        fx.workflow.steps = loop_steps(false);
        let (run, approvals) = run_approving_gates(&fx, "run-loop-plain", false).await;
        assert_eq!(approvals, 0);
        (guard_row(&run).clone(), fx.prompt_markers())
    };
    assert_eq!(plain.0.step_name, "__guard_loop_detection__");
    assert_eq!(
        plain.1.iter().filter(|m| **m == "prepare").count(),
        3,
        "{:?}",
        plain.1
    );

    for restart in [false, true] {
        let id = format!("loop-gate-{restart}");
        let mut fx = fixture(&id, SESSION_LIMIT, 0, LONG).await;
        fx.workflow.guards = guards(100, 3);
        fx.workflow.exec_allowlist = vec!["echo".into()];
        fx.workflow.steps = loop_steps(true);
        let (run, approvals) = run_approving_gates(&fx, &format!("run-{id}"), restart).await;
        let row = guard_row(&run);
        assert_eq!(row.step_name, plain.0.step_name, "restart={restart}");
        assert_eq!(row.output, plain.0.output, "restart={restart}");
        assert_eq!(fx.prompt_markers(), plain.1, "restart={restart}");
        assert_eq!(approvals, 3, "one approval per cycle, restart={restart}");
    }
}

#[tokio::test]
async fn after_a_gate_resume_the_llm_budget_counts_the_calls_already_made() {
    let plain = {
        let mut fx = fixture("budget-plain", SESSION_LIMIT, 0, LONG).await;
        fx.workflow.guards = guards(3, 10);
        fx.workflow.exec_allowlist = vec!["echo".into()];
        fx.workflow.steps = loop_steps(false);
        let (run, _) = run_approving_gates(&fx, "run-budget-plain", false).await;
        (guard_row(&run).clone(), fx.prompt_markers())
    };
    assert_eq!(plain.0.step_name, "__guard_max_llm_calls__");
    assert!(plain.0.output.contains("3 LLM calls"), "{}", plain.0.output);

    for restart in [false, true] {
        let id = format!("budget-gate-{restart}");
        let mut fx = fixture(&id, SESSION_LIMIT, 0, LONG).await;
        fx.workflow.guards = guards(3, 10);
        fx.workflow.exec_allowlist = vec!["echo".into()];
        fx.workflow.steps = loop_steps(true);
        let (run, approvals) = run_approving_gates(&fx, &format!("run-{id}"), restart).await;
        let row = guard_row(&run);
        assert_eq!(row.step_name, plain.0.step_name, "restart={restart}");
        assert_eq!(row.output, plain.0.output, "restart={restart}");
        assert_eq!(fx.prompt_markers(), plain.1, "restart={restart}");
        assert_eq!(approvals, 2, "restart={restart}");
    }
}

/// Wakes a run parked for quota until it settles elsewhere.
async fn wake_until_settled(fx: &Fixture, run_id: &str) -> WorkflowRun {
    for _ in 0..5 {
        let paused = stored(&fx.state, run_id).await;
        if paused.status != RunStatus::WaitingQuota {
            return paused;
        }
        let wake_at = paused
            .step_results
            .last()
            .unwrap()
            .quota_wait
            .as_ref()
            .unwrap()
            .wake_at
            .unwrap();
        let state = restarted(&fx.state);
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_at).await,
            vec![run_id.to_string()]
        );
        wait_until_settled(&state, run_id).await;
    }
    panic!("{run_id} kept waiting for quota");
}

async fn quota_run(
    id: &str,
    refusals: usize,
    guards_: Option<WorkflowGuards>,
) -> (WorkflowRun, Vec<&'static str>) {
    let mut fx = fixture(id, SESSION_LIMIT, refusals, LONG).await;
    fx.workflow.guards = guards_;
    let mut extra = fake_step("extra");
    extra.prompt_template = "EXTRA".into();
    fx.workflow.steps.push(extra);
    let (_, tokens, agents) = test_state_and_configs();
    let run_id = format!("run-{id}");
    let mut run = pending_run(&run_id, &fx.workflow.id);
    insert_wf_and_run(&fx.state, &fx.workflow, &run).await;
    execute_run(
        fx.state.clone(),
        &fx.workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let done = wake_until_settled(&fx, &run_id).await;
    (done, fx.prompt_markers())
}

#[tokio::test]
async fn quota_wake_ups_keep_the_llm_budget_and_do_not_count_the_refused_call() {
    let (plain, plain_prompts) = quota_run("quota-budget-plain", 0, guards(2, 10)).await;
    assert_eq!(guard_row(&plain).step_name, "__guard_max_llm_calls__");
    assert_eq!(plain_prompts, vec!["prepare", "analyse"]);

    let (waited, prompts) = quota_run("quota-budget-wait", 2, guards(2, 10)).await;
    assert_eq!(guard_row(&waited).output, guard_row(&plain).output);
    assert_eq!(prompts, vec!["prepare", "analyse", "analyse", "analyse"]);
    let analyse: Vec<_> = waited
        .step_results
        .iter()
        .filter(|r| r.step_name == "analyse")
        .collect();
    assert_eq!(analyse.len(), 1);
    assert_eq!(analyse[0].status, RunStatus::Success);
}

#[tokio::test]
async fn a_quota_replay_is_not_counted_as_a_revisit() {
    let (done, prompts) = quota_run("quota-revisit", 2, guards(100, 1)).await;
    assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
    assert_eq!(
        prompts,
        vec!["prepare", "analyse", "analyse", "analyse", "other"]
    );
}

// ─── KT-1042 — Gate checkpoints stay out of the operator's checkout ──────

/// Everything an operator could lose: HEAD, index, tracked and untracked files.
async fn checkout_snapshot(repo: &std::path::Path) -> (String, String, String, String) {
    (
        git_in(repo, &["rev-parse", "HEAD"]).await,
        git_in(repo, &["write-tree"]).await,
        git_in(repo, &["diff", "--binary"]).await,
        git_in(repo, &["status", "--porcelain=v1", "--untracked-files=all"]).await,
    )
}

fn checkpoint_gate_steps() -> Vec<WorkflowStep> {
    let mut gate = fake_step("review");
    gate.step_type = StepType::Gate;
    gate.gate_checkpoint_before = Some(true);
    vec![exec_step("produce", "touch", &["run-output.txt"]), gate]
}

async fn decide(
    state: &crate::AppState,
    workflow: &Workflow,
    run_id: &str,
    decision: GateDecision,
) {
    let (_, tokens, agents) = test_state_and_configs();
    let id = run_id.to_string();
    assert!(state
        .db
        .with_conn(move |conn| crate::db::workflows::claim_waiting_run(
            conn,
            &id,
            &RunStatus::Running
        ))
        .await
        .unwrap());
    let mut run = stored(state, run_id).await;
    resume_run(
        state.clone(),
        workflow,
        &mut run,
        decision,
        &tokens,
        &agents,
        None,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_shared_mode_gate_checkpoint_never_touches_the_operator_checkout() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, "proj-ckp-shared").await;
    std::fs::write(repo.path().join("README.md"), "operator edit\n").unwrap();
    std::fs::write(repo.path().join("wip.txt"), "operator draft\n").unwrap();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-ckp-shared".into();
    workflow.project_id = Some("proj-ckp-shared".into());
    workflow.exec_allowlist = vec!["touch".into()];
    workflow.steps = checkpoint_gate_steps();
    let mut run = pending_run("run-ckp-shared", &workflow.id);
    insert_wf_and_run(&state, &workflow, &run).await;
    std::fs::write(repo.path().join("run-output.txt"), "").unwrap();
    let before = checkout_snapshot(repo.path()).await;

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
    let paused = stored(&state, "run-ckp-shared").await;
    assert_eq!(paused.status, RunStatus::WaitingApproval);
    assert_eq!(paused.workspace_path, None, "shared mode");
    assert_eq!(checkout_snapshot(repo.path()).await, before);
    assert!(!paused.state.keys().any(|k| k.starts_with("checkpoint:")));
    let gate_output = &paused.step_results.last().unwrap().output;
    assert!(
        gate_output.contains("Checkpoint not taken"),
        "{gate_output}"
    );

    // A checkpoint recorded by an older version never touches the checkout.
    let old_head = git_in(repo.path(), &["rev-parse", "HEAD"]).await;
    git_in(
        repo.path(),
        &["commit", "-q", "--allow-empty", "-m", "operator commit"],
    )
    .await;
    state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::set_run_state_key(
                conn,
                "run-ckp-shared",
                "checkpoint:review",
                old_head.trim(),
                &[RunStatus::WaitingApproval],
            )
        })
        .await
        .unwrap();
    let before = checkout_snapshot(repo.path()).await;
    decide(
        &state,
        &workflow,
        "run-ckp-shared",
        GateDecision::RequestChanges {
            comment: "again".into(),
        },
    )
    .await;
    assert_eq!(
        stored(&state, "run-ckp-shared").await.status,
        RunStatus::WaitingApproval
    );
    assert_eq!(checkout_snapshot(repo.path()).await, before);
}

#[tokio::test]
async fn an_isolated_gate_checkpoints_the_run_worktree_and_leaves_the_checkout_alone() {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, "proj-ckp-isolated").await;
    std::fs::write(repo.path().join("README.md"), "operator edit\n").unwrap();
    std::fs::write(repo.path().join("wip.txt"), "operator draft\n").unwrap();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-ckp-isolated".into();
    workflow.project_id = Some("proj-ckp-isolated".into());
    workflow.workspace_config = Some(WorkspaceConfig {
        hooks: WorkspaceHooks::default(),
        require_isolation: true,
        main_tree_read_only: false,
        base_ref: None,
    });
    workflow.exec_allowlist = vec!["touch".into()];
    workflow.steps = checkpoint_gate_steps();
    let mut run = pending_run("run-ckp-isolated", &workflow.id);
    insert_wf_and_run(&state, &workflow, &run).await;
    let before = checkout_snapshot(repo.path()).await;

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
    let paused = stored(&state, "run-ckp-isolated").await;
    assert_eq!(
        paused.status,
        RunStatus::WaitingApproval,
        "{:?}",
        paused.step_results
    );
    let worktree = std::path::PathBuf::from(paused.workspace_path.clone().expect("isolated"));
    let sha = paused
        .state
        .get("checkpoint:review")
        .expect("checkpoint")
        .clone();
    assert_eq!(git_in(&worktree, &["rev-parse", "HEAD"]).await.trim(), sha);
    assert_eq!(git_in(&worktree, &["status", "--porcelain"]).await, "");
    assert!(git_in(&worktree, &["ls-files"])
        .await
        .contains("run-output.txt"));
    // Opening the worktree writes `.gitignore` into the checkout: a pre-existing
    // workspace defect tracked in its own ticket, not the checkpoint's doing.
    let mut after = checkout_snapshot(repo.path()).await;
    after.3 = after.3.replace("?? .gitignore\n", "");
    assert_eq!(after, before);
    let before = checkout_snapshot(repo.path()).await;

    decide(
        &state,
        &workflow,
        "run-ckp-isolated",
        GateDecision::RequestChanges {
            comment: "again".into(),
        },
    )
    .await;
    let again = stored(&state, "run-ckp-isolated").await;
    assert_eq!(
        again.status,
        RunStatus::WaitingApproval,
        "{:?}",
        again.step_results
    );
    let second = again
        .state
        .get("checkpoint:review")
        .expect("new checkpoint");
    assert_ne!(second, &sha);
    assert_eq!(
        git_in(&worktree, &["rev-parse", "HEAD"]).await.trim(),
        second
    );
    assert_eq!(checkout_snapshot(repo.path()).await, before);
}

#[tokio::test]
async fn a_goto_edge_cap_counts_the_fires_made_before_each_gate_resume() {
    let capped = |with_gate| {
        let mut steps = loop_steps(with_gate);
        let back = steps.last_mut().unwrap();
        back.on_result[0].action = ConditionAction::Goto {
            step_name: "prepare".into(),
            max_iterations: Some(2),
        };
        steps
    };
    let mut plain = fixture("cap-plain", SESSION_LIMIT, 0, LONG).await;
    plain.workflow.guards = guards(100, 10);
    plain.workflow.exec_allowlist = vec!["echo".into()];
    plain.workflow.steps = capped(false);
    let (run, _) = run_approving_gates(&plain, "run-cap-plain", false).await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert_eq!(plain.prompt_markers(), vec!["prepare"; 3]);

    for restart in [false, true] {
        let id = format!("cap-gate-{restart}");
        let mut fx = fixture(&id, SESSION_LIMIT, 0, LONG).await;
        fx.workflow.guards = guards(100, 10);
        fx.workflow.exec_allowlist = vec!["echo".into()];
        fx.workflow.steps = capped(true);
        let (run, approvals) = run_approving_gates(&fx, &format!("run-{id}"), restart).await;
        assert_eq!(run.status, RunStatus::Success, "restart={restart}");
        assert_eq!(fx.prompt_markers(), vec!["prepare"; 3], "restart={restart}");
        assert_eq!(approvals, 3, "restart={restart}");
    }
}

#[tokio::test]
async fn request_changes_loops_trip_loop_detection_even_though_history_is_truncated() {
    for restart in [false, true] {
        let id = format!("rc-gate-{restart}");
        let mut fx = fixture(&id, SESSION_LIMIT, 0, LONG).await;
        fx.workflow.guards = guards(100, 3);
        let mut steps = loop_steps(true);
        steps.pop();
        steps[1].gate_request_changes_target = Some("prepare".into());
        fx.workflow.steps = steps;
        let (run, decisions) = run_deciding_gates(
            &fx,
            &format!("run-{id}"),
            restart,
            GateDecision::RequestChanges {
                comment: "again".into(),
            },
        )
        .await;
        assert_eq!(guard_row(&run).step_name, "__guard_loop_detection__");
        assert_eq!(fx.prompt_markers(), vec!["prepare"; 3], "restart={restart}");
        assert_eq!(decisions, 3, "restart={restart}");
    }
}

// ─── KT-1046 — the tree's LLM budget survives a crash inside a foreach ────

/// Answers every child prompt at once, except the one naming `block`, which
/// never returns: the backend dies while that child is in flight.
struct ItemTurn {
    block: Option<&'static str>,
    prompts: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl crate::acp::AcpTransport for ItemTurn {
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
        crate::acp::AcpSessionTarget::new(crate::acp::AcpAgent::ClaudeCode, "item-turn")
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
        let item = ["T1", "T2", "T3", "T4", "T5"]
            .into_iter()
            .find(|id| prompt.contains(&format!("CHILD-ITEM-{id}")))
            .unwrap_or("other");
        self.prompts.lock().unwrap().push(item.to_string());
        if self.block == Some(item) {
            std::future::pending::<()>().await;
        }
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

struct ForeachTree {
    state: crate::AppState,
    workflow: Workflow,
    worktree: std::path::PathBuf,
    _repo: tempfile::TempDir,
}

/// A root run (LLM ceiling 3) whose foreach runs one Agent child per item of
/// five, all in the root's own worktree.
async fn foreach_tree(id: &str) -> ForeachTree {
    foreach_tree_with(id, false).await
}

/// A compensation Agent step, whose prompt the fake provider would record.
fn rollback_step(name: &str) -> WorkflowStep {
    let mut step = fake_step(name);
    step.prompt_template = format!("ROLLBACK-{name}");
    step
}

async fn foreach_tree_with(id: &str, child_rollback: bool) -> ForeachTree {
    let (state, _, _) = test_state_and_configs();
    let repo = git_project(&state, &format!("proj-{id}")).await;
    std::fs::write(
        repo.path().join("tasks.json"),
        r#"[{"id":"T1"},{"id":"T2"},{"id":"T3"},{"id":"T4"},{"id":"T5"}]"#,
    )
    .unwrap();
    git_in(repo.path(), &["add", "tasks.json"]).await;
    git_in(repo.path(), &["commit", "-q", "-m", "items"]).await;
    let worktree = repo.path().join("run-worktree");
    git_in(
        repo.path(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "run-branch",
            worktree.to_str().unwrap(),
        ],
    )
    .await;

    let mut child = make_workflow_with_artifacts(Default::default());
    child.id = format!("child-{id}");
    child.name = "child".into();
    let mut work = fake_step("work");
    work.prompt_template = "CHILD-ITEM-{{current_task.id}}".into();
    child.steps = vec![work];
    if child_rollback {
        child.on_failure = vec![rollback_step("child-rollback-count-error")];
    }
    let child_db = child.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &child_db))
        .await
        .unwrap();

    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = format!("wf-{id}");
    workflow.project_id = Some(format!("proj-{id}"));
    workflow.guards = guards(3, 10);
    let mut fanout = fake_step("fanout");
    fanout.step_type = StepType::SubWorkflow;
    fanout.sub_workflow_id = Some(child.id.clone());
    fanout.sub_workflow_foreach_file = Some("tasks.json".into());
    workflow.steps = vec![fanout];
    ForeachTree {
        state,
        workflow,
        worktree,
        _repo: repo,
    }
}

fn item_route(
    worktree: &std::path::Path,
    block: Option<&'static str>,
) -> (
    crate::agents::runner::test_acp_routes::RouteGuard,
    Arc<Mutex<Vec<String>>>,
) {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let work_dir = crate::agents::runner::resolve_agent_work_dir(
        Some(&worktree.to_string_lossy()),
        &worktree.to_string_lossy(),
    )
    .unwrap();
    let guard = crate::agents::runner::test_acp_routes::route(
        &work_dir,
        Arc::new(ItemTurn {
            block,
            prompts: prompts.clone(),
        }),
    );
    (guard, prompts)
}

async fn tree_count(state: &crate::AppState, run_id: &str) -> u32 {
    let id = run_id.to_string();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::tree_llm_calls(conn, &id))
        .await
        .unwrap()
}

async fn child_succeeded(state: &crate::AppState, root: &str, item: &str) -> bool {
    let (root, item) = (root.to_string(), item.to_string());
    state
        .db
        .with_conn(move |conn| crate::db::workflows::successful_child_item_ids(conn, &root))
        .await
        .unwrap()
        .contains(&item)
}

/// A root, its foreach and a child nest three runner futures: more than the
/// default test-thread stack holds.
fn on_big_stack<F: std::future::Future<Output = ()> + Send + 'static>(test: F) {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(test)
        })
        .unwrap()
        .join()
        .unwrap();
}

/// Limit of this proof: it covers calls that FINISHED before the crash. T2,
/// in flight when the backend died, is dispatched again on resume, so an
/// interrupted call can be billed twice; the cap holds on completed calls.
#[test]
fn a_crash_inside_a_foreach_keeps_the_llm_calls_its_finished_children_spent() {
    on_big_stack(crash_inside_a_foreach());
}

async fn crash_inside_a_foreach() {
    let (_, tokens, agents) = test_state_and_configs();

    // Control: the same tree, never interrupted.
    let control = foreach_tree("budget-tree-control").await;
    let (_route, control_prompts) = item_route(&control.worktree, None);
    let mut run = pending_run("run-budget-tree-control", &control.workflow.id);
    run.workspace_path = Some(control.worktree.to_string_lossy().into_owned());
    insert_wf_and_run(&control.state, &control.workflow, &run).await;
    execute_run(
        control.state.clone(),
        &control.workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let control_prompts = control_prompts.lock().unwrap().clone();
    assert_eq!(control_prompts, vec!["T1", "T2", "T3"]);
    assert_eq!(
        tree_count(&control.state, "run-budget-tree-control").await,
        3
    );

    // The backend dies while T2's child is in flight, after T1's success.
    let tree = foreach_tree("budget-tree-crash").await;
    let (route, before_prompts) = item_route(&tree.worktree, Some("T2"));
    let run_id = "run-budget-tree-crash";
    let mut run = pending_run(run_id, &tree.workflow.id);
    run.workspace_path = Some(tree.worktree.to_string_lossy().into_owned());
    insert_wf_and_run(&tree.state, &tree.workflow, &run).await;
    let task = {
        let (state, workflow, tokens, agents) = (
            tree.state.clone(),
            tree.workflow.clone(),
            tokens.clone(),
            agents.clone(),
        );
        tokio::spawn(async move {
            let _ = execute_run(
                state, &workflow, &mut run, &tokens, &agents, None, None, None,
            )
            .await;
        })
    };
    for _ in 0..500 {
        if before_prompts.lock().unwrap().contains(&"T2".to_string())
            && child_succeeded(&tree.state, run_id, "T1").await
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(*before_prompts.lock().unwrap(), vec!["T1", "T2"]);
    assert!(child_succeeded(&tree.state, run_id, "T1").await);
    task.abort();
    let _ = task.await;
    drop(route);
    tree.state
        .db
        .with_conn(|conn| crate::db::workflows::reconcile_stale_runs(conn, -60).map(|_| ()))
        .await
        .unwrap();

    let state = restarted(&tree.state);
    let (_route, after_prompts) = item_route(&tree.worktree, None);
    let mut run = stored(&state, run_id).await;
    assert_eq!(run.status, RunStatus::Interrupted);
    claim_interrupted_run(&state, &mut run, false)
        .await
        .expect("claimable");
    resume_interrupted_run(
        state.clone(),
        &tree.workflow,
        &mut run,
        &tokens,
        &agents,
        None,
    )
    .await
    .unwrap();

    let after_prompts = after_prompts.lock().unwrap().clone();
    assert!(
        !after_prompts.contains(&"T1".to_string()),
        "T1 is not replayed"
    );
    assert_eq!(
        after_prompts,
        vec!["T2", "T3"],
        "stops at the control's ceiling"
    );
    assert_eq!(tree_count(&state, run_id).await, 3);
}

#[tokio::test]
async fn an_older_parent_snapshot_never_lowers_the_tree_count_a_child_recorded() {
    let (state, _, _) = test_state_and_configs();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-tree-race".into();
    let mut parent = pending_run("run-tree-race", &workflow.id);
    parent.status = RunStatus::Running;
    insert_wf_and_run(&state, &workflow, &parent).await;

    // The parent loads its counters (1 call)...
    let counters = crate::workflows::guard_counters::GuardCounters {
        llm_calls: 1,
        ..Default::default()
    };
    counters.store(&mut parent.state);
    // ...a child of its tree spends and records the shared total...
    let budget = SharedBudget::root(10).recorded_on("run-tree-race");
    budget.add_llm_calls(3);
    record_tree_llm_calls(&state, &budget).await.unwrap();
    // ...then the parent saves its older snapshot, and a stale writer tries 2.
    let snap = crate::db::workflows::RunProgressSnapshot::from_run(&parent);
    state
        .db
        .with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
        .await
        .unwrap();
    state
        .db
        .with_conn(|conn| crate::db::workflows::raise_tree_llm_calls(conn, "run-tree-race", 2))
        .await
        .unwrap();

    assert_eq!(tree_count(&state, "run-tree-race").await, 3);
    let reloaded = stored(&state, "run-tree-race").await;
    let resumed = crate::workflows::guard_counters::GuardCounters::resume(
        &reloaded.state,
        &workflow.steps,
        &reloaded.step_results,
        None,
    );
    assert_eq!(
        resumed.llm_calls, 1,
        "the snapshot is the parent's older view"
    );
    assert_eq!(
        root_llm_calls_spent(&state, &reloaded, &resumed)
            .await
            .unwrap(),
        3
    );
}

#[tokio::test]
async fn a_run_from_before_the_counters_counts_its_finished_sub_workflow_children() {
    let (state, _, _) = test_state_and_configs();
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = "wf-legacy-tree".into();
    let mut work = fake_step("work");
    work.prompt_template = "x".into();
    let mut fanout = fake_step("fanout");
    fanout.step_type = StepType::SubWorkflow;
    workflow.steps = vec![work, fanout];
    let mut parent = pending_run("run-legacy-tree", &workflow.id);
    parent.status = RunStatus::Running;
    parent.step_results = vec![fake_result("work"), fake_result("fanout")];
    insert_wf_and_run(&state, &workflow, &parent).await;

    let agent_row = |status: RunStatus| {
        let mut row = fake_result("child-step");
        row.step_kind = Some("Agent".into());
        row.status = status;
        row
    };
    let mut child = pending_run("run-legacy-child", &workflow.id);
    child.status = RunStatus::Success;
    child.run_type = "subworkflow".into();
    child.parent_run_id = Some("run-legacy-tree".into());
    child.step_results = vec![agent_row(RunStatus::Success), agent_row(RunStatus::Failed)];
    let mut grandchild = child.clone();
    grandchild.id = "run-legacy-grandchild".into();
    grandchild.parent_run_id = Some("run-legacy-child".into());
    grandchild.step_results = vec![
        agent_row(RunStatus::Success),
        agent_row(RunStatus::WaitingQuota),
    ];
    state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::insert_run(conn, &child)?;
            crate::db::workflows::insert_run(conn, &grandchild)
        })
        .await
        .unwrap();

    let counters = crate::workflows::guard_counters::GuardCounters::resume(
        &parent.state,
        &workflow.steps,
        &parent.step_results,
        None,
    );
    assert_eq!(counters.llm_calls, 1, "the parent's own Agent step");
    // 1 (parent) + 2 (child) + 1 (grandchild; the quota refusal spent nothing).
    assert_eq!(
        root_llm_calls_spent(&state, &parent, &counters)
            .await
            .unwrap(),
        4
    );
}

/// Makes every write of a spent call to the tree count fail, as a database
/// error would (the run's opening write of 0 still lands).
async fn fail_tree_count_writes(state: &crate::AppState) {
    state
        .db
        .with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER fail_tree_count BEFORE UPDATE OF tree_llm_calls ON workflow_runs
                 WHEN NEW.tree_llm_calls > 0 BEGIN SELECT RAISE(FAIL, 'injected tree count failure'); END;",
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

#[test]
fn a_tree_count_that_cannot_be_written_stops_the_tree_before_its_next_call() {
    on_big_stack(async {
        let (_, tokens, agents) = test_state_and_configs();
        // Compensation on the root and on every child: a terminal stop runs none.
        let mut tree = foreach_tree_with("budget-tree-write-fails", true).await;
        tree.workflow.on_failure = vec![rollback_step("rollback-count-error")];
        let (_route, prompts) = item_route(&tree.worktree, None);
        let run_id = "run-budget-tree-write-fails";
        let mut run = pending_run(run_id, &tree.workflow.id);
        run.workspace_path = Some(tree.worktree.to_string_lossy().into_owned());
        insert_wf_and_run(&tree.state, &tree.workflow, &run).await;
        fail_tree_count_writes(&tree.state).await;

        let _ = execute_run(
            tree.state.clone(),
            &tree.workflow,
            &mut run,
            &tokens,
            &agents,
            None,
            None,
            None,
        )
        .await;

        assert_eq!(
            *prompts.lock().unwrap(),
            vec!["T1"],
            "no call, compensation included, is dispatched on top of an unrecorded one"
        );
        assert!(
            !child_succeeded(&tree.state, run_id, "T1").await,
            "an uncounted child is not durably done: a resume re-runs it"
        );
        // Every later child stopped before dispatching, on the guard row.
        let stored = stored(&tree.state, run_id).await;
        assert_ne!(stored.status, RunStatus::Success);
        let fanout = stored.step_results.last().unwrap();
        assert_eq!(fanout.status, RunStatus::Failed);
        assert!(
            fanout.output.contains("could not be recorded")
                && fanout.output.contains("\"succeeded\":0"),
            "{}",
            fanout.output
        );
    });
}

#[test]
fn a_resume_that_cannot_read_the_tree_count_does_not_run() {
    on_big_stack(async {
        let (_, tokens, agents) = test_state_and_configs();
        let tree = foreach_tree("budget-tree-read-fails").await;
        let (_route, prompts) = item_route(&tree.worktree, None);
        let run_id = "run-budget-tree-read-fails";
        let mut run = pending_run(run_id, &tree.workflow.id);
        run.status = RunStatus::Interrupted;
        run.workspace_path = Some(tree.worktree.to_string_lossy().into_owned());
        insert_wf_and_run(&tree.state, &tree.workflow, &run).await;
        tree.state
            .db
            .with_conn(|conn| {
                conn.execute_batch(
                    "ALTER TABLE workflow_runs RENAME COLUMN tree_llm_calls TO tree_llm_calls_gone",
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let state = restarted(&tree.state);
        let mut run = stored(&state, run_id).await;
        claim_interrupted_run(&state, &mut run, false)
            .await
            .expect("claimable");
        let error = resume_interrupted_run(
            state.clone(),
            &tree.workflow,
            &mut run,
            &tokens,
            &agents,
            None,
        )
        .await
        .expect_err("a stale total must not run");
        assert!(error.to_string().contains("LLM-call count"), "{error}");
        assert!(prompts.lock().unwrap().is_empty(), "nothing dispatched");
    });
}

#[test]
fn a_terminal_stop_overrides_a_quota_wait_and_a_recovery_rule() {
    let mut result = fake_result("work");
    result.quota_wait = Some(crate::models::QuotaWait {
        id: None,
        reset_at: None,
        wake_at: None,
        attempt: 0,
        parked: None,
        detail: None,
    });
    result.condition_result = Some("Goto:work".into());
    result.terminal_stop = Some("counter down".into());
    let mut outcome = crate::workflows::steps::StepOutcome {
        result,
        condition_action: Some(ConditionAction::Goto {
            step_name: "work".into(),
            max_iterations: None,
        }),
    };
    apply_terminal_stop(&mut outcome);
    assert_eq!(outcome.result.status, RunStatus::Failed);
    assert_eq!(outcome.result.quota_wait, None);
    assert_eq!(outcome.result.condition_result, None);
    assert!(outcome.condition_action.is_none());
    assert!(outcome.result.output.starts_with("counter down"));
}
