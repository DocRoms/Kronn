//! KT-1119 — a single sub-workflow child refused for quota waits instead of
//! failing; its parent mirrors the wait and, once woken, reclaims the child at
//! its refused step, through any depth of single sub-workflows.

use super::gate_guard_runs::on_big_stack;
use super::quota_wait_foreach_runs::{descendants, ReviewRefused};
use super::quota_wait_runs::{restarted, stored, SESSION_LIMIT};
use super::*;
use crate::models::{QuotaParkReason, WorkflowGuards};
use std::sync::{Arc, Mutex};

struct Tree {
    state: crate::AppState,
    root_id: String,
    child_workflow_id: String,
    prompts: Arc<Mutex<Vec<String>>>,
    _route: crate::agents::runner::test_acp_routes::RouteGuard,
    _repo: tempfile::TempDir,
}

impl Tree {
    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    /// The runs below the root, root's own child first.
    async fn descendants(&self) -> Vec<WorkflowRun> {
        let root = self.root_id.clone();
        self.state
            .db
            .with_conn(move |conn| descendants(conn, &root))
            .await
            .unwrap()
    }

    /// The only run of the leaf workflow, the one whose review is refused.
    async fn leaf(&self) -> WorkflowRun {
        let mut leaves: Vec<WorkflowRun> = self
            .descendants()
            .await
            .into_iter()
            .filter(|run| run.workflow_id == self.child_workflow_id)
            .collect();
        assert_eq!(leaves.len(), 1, "one leaf run");
        leaves.remove(0)
    }
}

fn names(prompts: &[&str]) -> Vec<String> {
    prompts.iter().map(|p| p.to_string()).collect()
}

fn wake_of(run: &WorkflowRun) -> chrono::DateTime<Utc> {
    run.step_results
        .last()
        .and_then(|step| step.quota_wait.as_ref())
        .and_then(|wait| wait.wake_at)
        .expect("a scheduled wake")
}

fn single_step(name: &str, target: &str) -> crate::models::WorkflowStep {
    let mut step = fake_step(name);
    step.step_type = StepType::SubWorkflow;
    step.sub_workflow_id = Some(target.to_string());
    step
}

fn agent_step(name: &str, prompt: &str) -> crate::models::WorkflowStep {
    let mut step = fake_step(name);
    step.prompt_template = prompt.into();
    step
}

async fn insert_workflow(state: &crate::AppState, workflow: &Workflow) {
    let workflow = workflow.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &workflow))
        .await
        .unwrap();
}

/// A root (linear, its own worktree) running `setup`, then a single
/// sub-workflow chain `levels` deep ending in a child that runs `prepare` then
/// `review`, then `wrap`. The provider refuses the child's review `refusals`
/// times.
async fn tree(id: &str, refusal: &str, refusals: usize, levels: usize) -> Tree {
    let (state, _, _) = test_state_and_configs();
    let repo = git_project(&state, &format!("proj-{id}")).await;
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

    let mut leaf = make_workflow_with_artifacts(Default::default());
    leaf.id = format!("child-{id}");
    leaf.name = "review one".into();
    leaf.steps = vec![
        agent_step("prepare", "PREP-T2"),
        agent_step("review", "REVIEW-T2"),
    ];
    insert_workflow(&state, &leaf).await;

    let mut target = leaf.id.clone();
    for level in 1..levels {
        let mut middle = make_workflow_with_artifacts(Default::default());
        middle.id = format!("middle{level}-{id}");
        middle.name = format!("middle {level}");
        middle.steps = vec![single_step("delegate", &target)];
        insert_workflow(&state, &middle).await;
        target = middle.id.clone();
    }

    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = format!("wf-{id}");
    workflow.project_id = Some(format!("proj-{id}"));
    workflow.guards = Some(WorkflowGuards {
        max_llm_calls: Some(50),
        ..Default::default()
    });
    workflow.steps = vec![
        agent_step("setup", "PREP-T1"),
        single_step("work", &target),
        agent_step("wrap", "REVIEW-T3"),
    ];
    let root_id = format!("run-{id}");

    let prompts = Arc::new(Mutex::new(Vec::new()));
    let work_dir = crate::agents::runner::resolve_agent_work_dir(
        Some(&worktree.to_string_lossy()),
        &worktree.to_string_lossy(),
    )
    .unwrap();
    let route = crate::agents::runner::test_acp_routes::route(
        &work_dir,
        Arc::new(ReviewRefused {
            refusal: refusal.to_string(),
            refusals: Mutex::new(refusals),
            failing: None,
            resumed_probe: None,
            prompts: prompts.clone(),
        }),
    );

    let mut run = pending_run(&root_id, &workflow.id);
    run.workspace_path = Some(worktree.to_string_lossy().into_owned());
    insert_wf_and_run(&state, &workflow, &run).await;
    let (_, tokens, agents) = test_state_and_configs();
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
    .expect("the root pauses, it does not error");
    Tree {
        state,
        root_id,
        child_workflow_id: leaf.id,
        prompts,
        _route: route,
        _repo: repo,
    }
}

#[test]
fn a_single_child_refused_for_quota_waits_its_parent_and_resumes_at_its_step() {
    on_big_stack(async {
        let tree = tree("single-quota", SESSION_LIMIT, 1, 1).await;

        // The parent waits with its child: neither failed nor finished.
        let root = stored(&tree.state, &tree.root_id).await;
        assert_eq!(root.status, RunStatus::WaitingQuota);
        assert_eq!(root.finished_at, None);
        assert_eq!(root.step_results.len(), 2, "wrap has not run");
        let work = root.step_results.last().unwrap();
        assert_eq!(work.status, RunStatus::WaitingQuota);
        assert!(
            work.output.contains("en attente de quota"),
            "{}",
            work.output
        );
        let root_wait = work
            .quota_wait
            .clone()
            .expect("the parent mirrors the wait");

        let waiting = tree.leaf().await;
        assert_eq!(waiting.status, RunStatus::WaitingQuota);
        assert_eq!(waiting.finished_at, None);
        assert_eq!(work.child_run_id.as_deref(), Some(waiting.id.as_str()));
        assert_eq!(waiting.step_results.len(), 2);
        assert_eq!(waiting.step_results[0].status, RunStatus::Success);
        assert_eq!(waiting.step_results[1].status, RunStatus::WaitingQuota);
        let child_wait = waiting.step_results[1].quota_wait.clone().unwrap();
        let wake_at = child_wait.wake_at.expect("9:40pm Europe/Paris is a reset");
        assert_eq!(root_wait.wake_at, Some(wake_at));
        assert_eq!(root_wait.reset_at, child_wait.reset_at);
        assert_ne!(root_wait.id, child_wait.id, "each run has its own wait");
        assert_eq!(tree.prompts(), names(&["PREP-T1", "PREP-T2", "REVIEW-T2"]));

        // Only the parent is woken, and only once; the row alone drives it.
        let state = restarted(&tree.state);
        assert!(crate::workflows::quota_wait::wake_due_runs(
            &state,
            wake_at - chrono::Duration::seconds(1)
        )
        .await
        .is_empty());
        let woken = crate::workflows::quota_wait::wake_due_runs(&state, wake_at).await;
        assert_eq!(woken, vec![tree.root_id.clone()]);
        assert!(crate::workflows::quota_wait::wake_due_runs(&state, wake_at)
            .await
            .is_empty());

        let done = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
        assert_eq!(done.step_results.len(), 3);
        assert!(
            crate::workflows::quota_wait::mirrored_child(&done.state, "work").is_none(),
            "the record is dropped once the step ends"
        );
        let resumed = tree.leaf().await;
        assert_eq!(resumed.id, waiting.id, "the same child run resumes");
        assert_eq!(resumed.status, RunStatus::Success);
        assert_eq!(resumed.step_results.len(), 2);
        assert_eq!(resumed.step_results[1].quota_wait, None);
        // Neither the parent's setup nor the child's prepare is replayed.
        assert_eq!(
            tree.prompts(),
            names(&["PREP-T1", "PREP-T2", "REVIEW-T2", "REVIEW-T2", "REVIEW-T3"])
        );

        // A stale copy of the old child wait claims nothing.
        let mut stale = waiting.clone();
        assert!(try_claim_interrupted_run_row(&state, &mut stale, None)
            .await
            .is_err());
        assert_eq!(tree.leaf().await.status, RunStatus::Success);
    });
}

#[test]
fn a_refusal_two_sub_workflows_deep_waits_the_whole_chain_and_resumes_only_the_leaf() {
    on_big_stack(async {
        let tree = tree("single-quota-deep", SESSION_LIMIT, 1, 2).await;

        let root = stored(&tree.state, &tree.root_id).await;
        assert_eq!(root.status, RunStatus::WaitingQuota);
        let chain = tree.descendants().await;
        assert_eq!(chain.len(), 2);
        assert!(chain
            .iter()
            .all(|run| run.status == RunStatus::WaitingQuota && run.finished_at.is_none()));
        let wake_at = wake_of(&root);
        assert!(chain.iter().all(|run| wake_of(run) == wake_at));
        let waiting = tree.leaf().await;
        assert_eq!(tree.prompts(), names(&["PREP-T1", "PREP-T2", "REVIEW-T2"]));

        // Only the root is woken; the middle child is reclaimed through it.
        let state = restarted(&tree.state);
        let woken = crate::workflows::quota_wait::wake_due_runs(&state, wake_at).await;
        assert_eq!(woken, vec![tree.root_id.clone()]);
        let done = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
        let after = tree.descendants().await;
        assert_eq!(after.len(), 2, "no child is created by the resume");
        assert!(after.iter().all(|run| run.status == RunStatus::Success));
        assert_eq!(tree.leaf().await.id, waiting.id);
        assert_eq!(
            tree.prompts(),
            names(&["PREP-T1", "PREP-T2", "REVIEW-T2", "REVIEW-T2", "REVIEW-T3"])
        );
    });
}

#[test]
fn a_refusal_without_a_reset_parks_the_chain_until_a_manual_resume() {
    on_big_stack(async {
        let refusal = "[Agent provider error] You've hit your usage limit. Upgrade to Pro or try again at 3:05 PM. (HTTP 429)";
        let tree = tree("single-quota-parked", refusal, 1, 1).await;
        let root = stored(&tree.state, &tree.root_id).await;
        assert_eq!(root.status, RunStatus::WaitingQuota);
        let root_wait = root
            .step_results
            .last()
            .unwrap()
            .quota_wait
            .clone()
            .unwrap();
        assert_eq!(root_wait.parked, Some(QuotaParkReason::NoResetTime));
        assert_eq!(root_wait.wake_at, None);

        assert!(crate::workflows::quota_wait::wake_due_runs(
            &tree.state,
            Utc::now() + chrono::Duration::days(30)
        )
        .await
        .is_empty());
        assert_eq!(
            stored(&tree.state, &tree.root_id).await.status,
            RunStatus::WaitingQuota
        );

        let axum::Json(response) = crate::api::workflows::resume_interrupted(
            axum::extract::State(tree.state.clone()),
            axum::extract::Path(tree.root_id.clone()),
            None,
            None,
            axum::body::Bytes::new(),
        )
        .await;
        assert!(response.success, "{:?}", response.error);
        let done = wait_until_settled(&tree.state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
        assert_eq!(tree.leaf().await.status, RunStatus::Success);
        assert_eq!(
            tree.prompts()
                .iter()
                .filter(|p| p.as_str() == "PREP-T2")
                .count(),
            1
        );
    });
}

#[test]
fn a_tree_cancelled_while_a_single_child_waits_is_never_woken() {
    on_big_stack(async {
        let tree = tree("single-quota-cancel", SESSION_LIMIT, 1, 2).await;
        let wake_at = wake_of(&stored(&tree.state, &tree.root_id).await);
        let outcome = crate::workflows::cancellation::cancel_run_tree(
            &tree.state,
            &tree.root_id,
            crate::workflows::cancellation::CancellationScope::RunTree,
            "cancelled_by_operator",
        )
        .await
        .unwrap();
        assert!(outcome.run_cancelled);

        let state = restarted(&tree.state);
        let woken = crate::workflows::quota_wait::wake_due_runs(
            &state,
            wake_at + chrono::Duration::days(1),
        )
        .await;
        assert!(woken.is_empty());
        assert_eq!(
            stored(&state, &tree.root_id).await.status,
            RunStatus::Cancelled
        );
        assert!(tree
            .descendants()
            .await
            .iter()
            .all(|run| run.status == RunStatus::Cancelled));
        assert_eq!(tree.prompts(), names(&["PREP-T1", "PREP-T2", "REVIEW-T2"]));
    });
}

/// The wake is an absolute instant from the reset, whatever the parent's
/// `started_at`: three real hours past a two-hour Timeout, the pause spent
/// none of it, for the parent and for the child it reclaims.
#[test]
fn a_reset_three_hours_after_the_parent_started_still_resumes_the_child() {
    on_big_stack(async {
        let refusal = "HTTP 429 Too Many Requests; retry-after: 10800";
        let tree = tree("single-quota-delay", refusal, 1, 1).await;
        let root = stored(&tree.state, &tree.root_id).await;
        assert_eq!(root.status, RunStatus::WaitingQuota);
        let wake_at = wake_of(&root);
        assert!(wake_at > Utc::now() + chrono::Duration::hours(2));
        let waiting = tree.leaf().await;
        assert_eq!(wake_of(&waiting), wake_at);

        let three_hours_ago = (Utc::now() - chrono::Duration::hours(3)).to_rfc3339();
        let (root_id, child_id) = (tree.root_id.clone(), waiting.id.clone());
        tree.state
            .db
            .with_conn(move |conn| {
                for id in [root_id, child_id] {
                    conn.execute(
                        "UPDATE workflow_runs SET started_at = ?2 WHERE id = ?1",
                        rusqlite::params![id, three_hours_ago],
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();

        let state = restarted(&tree.state);
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_at).await,
            vec![tree.root_id.clone()]
        );
        let done = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
        assert!(Utc::now() - done.started_at >= chrono::Duration::hours(3));
        let resumed = tree.leaf().await;
        assert_eq!(resumed.id, waiting.id);
        assert_eq!(resumed.status, RunStatus::Success);
        let root_ms: u64 = done.state[TIMEOUT_ACTIVE_MS_STATE_KEY].parse().unwrap();
        assert!(root_ms < 60_000, "{root_ms}");
        assert_eq!(
            tree.prompts(),
            names(&["PREP-T1", "PREP-T2", "REVIEW-T2", "REVIEW-T2", "REVIEW-T3"])
        );
    });
}

/// The parent's record names the child AND its wait: a child waiting under
/// another identity is not reclaimed, and the step runs a fresh child.
#[test]
fn a_record_naming_another_wait_reclaims_no_child() {
    on_big_stack(async {
        let tree = tree("single-quota-other-wait", SESSION_LIMIT, 1, 1).await;
        let root = stored(&tree.state, &tree.root_id).await;
        let waiting = tree.leaf().await;
        let mut record = std::collections::HashMap::new();
        crate::workflows::quota_wait::remember_mirrored_child(
            &mut record,
            "work",
            Some(&waiting.id),
            Some("another-wait"),
        );
        let (key, value) = record.into_iter().next().unwrap();
        let root_id = tree.root_id.clone();
        tree.state
            .db
            .with_conn(move |conn| {
                let mut run = crate::db::workflows::get_run(conn, &root_id)?.unwrap();
                run.state.insert(key, value);
                conn.execute(
                    "UPDATE workflow_runs SET state = ?2 WHERE id = ?1",
                    rusqlite::params![root_id, serde_json::to_string(&run.state)?],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let state = restarted(&tree.state);
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_of(&root)).await,
            vec![tree.root_id.clone()]
        );
        let done = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
        let children = tree.descendants().await;
        assert_eq!(children.len(), 2, "a fresh child ran");
        let old = children.iter().find(|run| run.id == waiting.id).unwrap();
        assert_eq!(old.status, RunStatus::WaitingQuota, "never claimed");
        assert_eq!(
            tree.prompts(),
            names(&[
                "PREP-T1",
                "PREP-T2",
                "REVIEW-T2",
                "PREP-T2",
                "REVIEW-T2",
                "REVIEW-T3"
            ])
        );
    });
}
