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
    tree_of(id, refusal, refusals, levels, |_| {}, |_| {}).await
}

/// [`tree`], with `leaf_shape` and `root_shape` applied to the leaf and the
/// root workflow before they are stored.
async fn tree_of(
    id: &str,
    refusal: &str,
    refusals: usize,
    levels: usize,
    leaf_shape: impl FnOnce(&mut Workflow),
    root_shape: impl FnOnce(&mut Workflow),
) -> Tree {
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
    leaf_shape(&mut leaf);
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
    root_shape(&mut workflow);
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

/// The parent follows its own child, whichever wait the child is parked on: a
/// record naming an older wait still reclaims that child, never a fresh one.
#[test]
fn a_stale_wait_in_the_record_still_reclaims_the_same_child() {
    on_big_stack(async {
        let tree = tree("single-quota-stale-wait", SESSION_LIMIT, 1, 1).await;
        let root = stored(&tree.state, &tree.root_id).await;
        let waiting = tree.leaf().await;
        set_record(&tree, Some(&waiting.id), Some("an-older-wait")).await;

        let state = restarted(&tree.state);
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_of(&root)).await,
            vec![tree.root_id.clone()]
        );
        let done = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
        let children = tree.descendants().await;
        assert_eq!(children.len(), 1, "no second child");
        assert_eq!(children[0].id, waiting.id);
        assert_eq!(children[0].status, RunStatus::Success);
        assert_eq!(
            tree.prompts(),
            names(&["PREP-T1", "PREP-T2", "REVIEW-T2", "REVIEW-T2", "REVIEW-T3"])
        );
    });
}

/// A backend that died while the parent resumed its child leaves that child
/// `Interrupted`: the recorded child is resumed, not replaced by a fresh one.
#[test]
fn an_interrupted_recorded_child_is_resumed_not_replaced() {
    on_big_stack(async {
        let tree = tree("single-quota-interrupted-child", SESSION_LIMIT, 1, 1).await;
        let root = stored(&tree.state, &tree.root_id).await;
        let waiting = tree.leaf().await;
        let child_id = waiting.id.clone();
        tree.state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "UPDATE workflow_runs SET status = 'Interrupted' WHERE id = ?1",
                    rusqlite::params![child_id],
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
        assert_eq!(children.len(), 1, "no second child");
        assert_eq!(children[0].id, waiting.id);
        assert_eq!(children[0].status, RunStatus::Success);
        assert_eq!(
            tree.prompts()
                .iter()
                .filter(|p| p.as_str() == "PREP-T2")
                .count(),
            1,
            "the child's earlier step is not replayed"
        );
    });
}

/// A record whose child is not one of this parent's starts a fresh child.
#[test]
fn a_record_naming_no_child_of_this_parent_starts_a_fresh_one() {
    on_big_stack(async {
        let tree = tree("single-quota-unknown-child", SESSION_LIMIT, 1, 1).await;
        let root = stored(&tree.state, &tree.root_id).await;
        let waiting = tree.leaf().await;
        set_record(&tree, Some("no-such-run"), Some("whatever")).await;

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
    });
}

/// Overwrites the parent's mirrored-child record for step `work`.
async fn set_record(tree: &Tree, child: Option<&str>, wait: Option<&str>) {
    let mut record = std::collections::HashMap::new();
    crate::workflows::quota_wait::remember_mirrored_child(&mut record, "work", child, wait);
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
}

fn wait_id_of(run: &WorkflowRun) -> Option<String> {
    run.step_results
        .last()
        .and_then(|step| step.quota_wait.as_ref())
        .and_then(|wait| wait.id.clone())
}

/// The child, woken at the reset, is refused again and parks under a NEW wait
/// identity: the parent's record follows it, so the next wake reclaims the
/// same child. One child runs in the end and no step is replayed.
#[test]
fn a_child_refused_again_at_the_reset_is_followed_to_its_new_wait() {
    on_big_stack(async {
        let tree = tree("single-quota-repark", SESSION_LIMIT, 2, 1).await;
        let first_root = stored(&tree.state, &tree.root_id).await;
        let first = tree.leaf().await;
        let first_wait = wait_id_of(&first).expect("the child's first wait");

        let state = restarted(&tree.state);
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_of(&first_root)).await,
            vec![tree.root_id.clone()]
        );
        let again = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(
            again.status,
            RunStatus::WaitingQuota,
            "{:?}",
            again.step_results
        );
        let second = tree.leaf().await;
        assert_eq!(second.id, first.id, "no second child");
        assert_eq!(second.status, RunStatus::WaitingQuota);
        assert_eq!(second.step_results.len(), 2, "the step is retried in place");
        let second_wait = wait_id_of(&second).expect("the child's second wait");
        assert_ne!(first_wait, second_wait, "re-parked under a new identity");
        assert_eq!(
            crate::workflows::quota_wait::mirrored_child(&again.state, "work"),
            Some((first.id.clone(), Some(second_wait))),
            "the parent's record follows the child"
        );
        assert_ne!(
            wait_id_of(&again),
            wait_id_of(&first_root),
            "its own wait too"
        );

        // The parent's second wake reclaims that same child.
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_of(&again)).await,
            vec![tree.root_id.clone()]
        );
        let done = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
        let children = tree.descendants().await;
        assert_eq!(children.len(), 1, "one child run in the end");
        assert_eq!(children[0].id, first.id);
        assert_eq!(children[0].status, RunStatus::Success);
        assert_eq!(
            tree.prompts(),
            names(&[
                "PREP-T1",
                "PREP-T2",
                "REVIEW-T2",
                "REVIEW-T2",
                "REVIEW-T2",
                "REVIEW-T3"
            ]),
            "setup and prepare ran once, only the refused step was retried"
        );
    });
}

/// A child a human cancelled while its parent waited ends the parent's wait
/// with a clear failed step; no fresh child starts in its place.
#[test]
fn a_child_cancelled_while_its_parent_waits_ends_the_wait_cleanly() {
    on_big_stack(async {
        let tree = tree("single-quota-child-cancelled", SESSION_LIMIT, 1, 1).await;
        let root = stored(&tree.state, &tree.root_id).await;
        let waiting = tree.leaf().await;
        let outcome = crate::workflows::cancellation::cancel_run_tree(
            &tree.state,
            &waiting.id,
            crate::workflows::cancellation::CancellationScope::RunTree,
            "cancelled_by_operator",
        )
        .await
        .unwrap();
        assert!(outcome.run_cancelled);
        assert_eq!(
            stored(&tree.state, &tree.root_id).await.status,
            RunStatus::WaitingQuota,
            "cancelling the child alone leaves the parent waiting"
        );

        let state = restarted(&tree.state);
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_of(&root)).await,
            vec![tree.root_id.clone()]
        );
        let done = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Failed, "{:?}", done.step_results);
        assert_eq!(done.step_results.len(), 2, "wrap never ran");
        let work = done.step_results.last().unwrap();
        assert_eq!(work.step_name, "work");
        assert_eq!(work.status, RunStatus::Failed);
        assert_eq!(work.quota_wait, None);
        assert!(
            work.output
                .contains("was cancelled while its parent waited"),
            "{}",
            work.output
        );
        assert!(
            !work.output.contains("claim"),
            "no claim error: {}",
            work.output
        );
        let children = tree.descendants().await;
        assert_eq!(children.len(), 1, "no fresh child");
        assert_eq!(children[0].status, RunStatus::Cancelled);
        assert_eq!(tree.prompts(), names(&["PREP-T1", "PREP-T2", "REVIEW-T2"]));
    });
}

/// What `wake_one` does before it spawns the resume: the parent is claimed.
async fn claim_parent_like_the_wake(
    tree: &Tree,
    state: &crate::AppState,
) -> (Workflow, WorkflowRun) {
    let mut root = stored(state, &tree.root_id).await;
    let workflow_id = root.workflow_id.clone();
    let workflow = state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &workflow_id))
        .await
        .unwrap()
        .expect("root workflow");
    check_resume_preconditions(&mut root, false).unwrap();
    try_claim_interrupted_run_row(state, &mut root, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored(state, &tree.root_id).await.status,
        RunStatus::Running
    );
    (workflow, root)
}

async fn cancel_tree(state: &crate::AppState, root_id: &str) {
    crate::workflows::cancellation::cancel_run_tree(
        state,
        root_id,
        crate::workflows::cancellation::CancellationScope::RunTree,
        "cancelled_by_operator",
    )
    .await
    .unwrap();
}

/// The tree is cancelled after the parent's wake claimed it and before its
/// resume ran: the parent never starts, so neither does the child.
#[test]
fn a_cancel_after_the_parents_wake_claim_runs_nothing() {
    on_big_stack(async {
        let tree = tree("single-quota-cancel-after-claim", SESSION_LIMIT, 1, 1).await;
        let state = restarted(&tree.state);
        let (workflow, mut root) = claim_parent_like_the_wake(&tree, &state).await;
        cancel_tree(&state, &tree.root_id).await;

        let (_, tokens, agents) = test_state_and_configs();
        resume_interrupted_run(state.clone(), &workflow, &mut root, &tokens, &agents, None)
            .await
            .unwrap();
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

/// The cancel lands after the step read its waiting child and before the
/// child's claim: the claim is refused (the parent no longer accepts children,
/// and the wait it names is gone), so the child does not resume.
#[test]
fn a_cancel_between_the_childs_read_and_its_claim_refuses_the_claim() {
    on_big_stack(async {
        let tree = tree("single-quota-cancel-before-claim", SESSION_LIMIT, 1, 1).await;
        let state = restarted(&tree.state);
        claim_parent_like_the_wake(&tree, &state).await;
        let mut read = tree.leaf().await;
        assert_eq!(read.status, RunStatus::WaitingQuota);
        cancel_tree(&state, &tree.root_id).await;

        assert!(try_claim_interrupted_run_row(&state, &mut read, None)
            .await
            .is_err());
        assert_eq!(tree.leaf().await.status, RunStatus::Cancelled);
        assert_eq!(tree.prompts(), names(&["PREP-T1", "PREP-T2", "REVIEW-T2"]));
    });
}

/// The cancel lands after the child was claimed and before it resumed: the
/// runner's start guard sees the cancelled row and runs no step.
#[test]
fn a_cancel_between_the_childs_claim_and_its_resume_runs_no_step() {
    on_big_stack(async {
        let tree = tree("single-quota-cancel-after-child-claim", SESSION_LIMIT, 1, 1).await;
        let state = restarted(&tree.state);
        claim_parent_like_the_wake(&tree, &state).await;
        let mut child = tree.leaf().await;
        try_claim_interrupted_run_row(&state, &mut child, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tree.leaf().await.status, RunStatus::Running);
        cancel_tree(&state, &tree.root_id).await;

        let workflow_id = child.workflow_id.clone();
        let child_workflow = state
            .db
            .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &workflow_id))
            .await
            .unwrap()
            .expect("child workflow");
        let (_, tokens, agents) = test_state_and_configs();
        execute_run(
            state.clone(),
            &child_workflow,
            &mut child,
            &tokens,
            &agents,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(child.status, RunStatus::Cancelled);
        assert_eq!(tree.leaf().await.status, RunStatus::Cancelled);
        assert_eq!(tree.prompts(), names(&["PREP-T1", "PREP-T2", "REVIEW-T2"]));
    });
}

fn guards_with_timeout(timeout_seconds: u64) -> Option<WorkflowGuards> {
    Some(WorkflowGuards {
        timeout_seconds: Some(timeout_seconds),
        max_llm_calls: Some(50),
        ..Default::default()
    })
}

/// The child's own Timeout counts its active time, whatever its parent has
/// left: a child that spent its limit before parking is stopped by its guard at
/// the resume, not run past it.
#[test]
fn a_child_never_resumes_past_its_own_timeout() {
    on_big_stack(async {
        let tree = tree_of(
            "single-quota-child-timeout",
            SESSION_LIMIT,
            1,
            1,
            |leaf| leaf.guards = guards_with_timeout(10),
            |_| {},
        )
        .await;
        let root = stored(&tree.state, &tree.root_id).await;
        let waiting = tree.leaf().await;
        // 11 s of active time against a 10 s limit; the parent's budget is untouched.
        let child_id = waiting.id.clone();
        tree.state
            .db
            .with_conn(move |conn| {
                let mut child = crate::db::workflows::get_run(conn, &child_id)?.unwrap();
                child
                    .state
                    .insert(TIMEOUT_ACTIVE_MS_STATE_KEY.into(), "11000".into());
                conn.execute(
                    "UPDATE workflow_runs SET state = ?2 WHERE id = ?1",
                    rusqlite::params![child_id, serde_json::to_string(&child.state)?],
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
        assert_ne!(done.status, RunStatus::Success, "{:?}", done.step_results);
        let stopped = tree.leaf().await;
        assert_eq!(stopped.id, waiting.id);
        assert_eq!(stopped.status, RunStatus::StoppedByGuard);
        assert_eq!(
            stopped.step_results.last().unwrap().step_name,
            "__guard_timeout__"
        );
        assert_eq!(
            tree.prompts(),
            names(&["PREP-T1", "PREP-T2", "REVIEW-T2"]),
            "the refused step was not replayed past the limit"
        );
        assert_eq!(done.step_results.len(), 2, "wrap never ran");
    });
}

/// A child below its own limit resumes and carries on its active time, though
/// that limit is shorter than the parent's remaining budget.
#[test]
fn a_child_with_a_shorter_timeout_than_its_parent_resumes_within_it() {
    on_big_stack(async {
        let tree = tree_of(
            "single-quota-child-short-timeout",
            SESSION_LIMIT,
            1,
            1,
            |leaf| leaf.guards = guards_with_timeout(600),
            |_| {},
        )
        .await;
        let root = stored(&tree.state, &tree.root_id).await;
        let state = restarted(&tree.state);
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_of(&root)).await,
            vec![tree.root_id.clone()]
        );
        let done = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
        let resumed = tree.leaf().await;
        assert_eq!(resumed.status, RunStatus::Success);
        let child_ms: u64 = resumed.state[TIMEOUT_ACTIVE_MS_STATE_KEY].parse().unwrap();
        assert!(child_ms < 60_000, "{child_ms}");
    });
}

/// What the child accumulated in its trigger context survives the resume, next
/// to the markers the parent sets again. The context is only read in memory, so
/// a probe step at the end of the child reports what the resumed child sees.
#[test]
fn a_resumed_child_keeps_its_own_trigger_context() {
    on_big_stack(async {
        let tree = tree_of(
            "single-quota-context",
            SESSION_LIMIT,
            2,
            1,
            |leaf| {
                let mut probe = fake_step("probe");
                probe.step_type = StepType::TransformData;
                probe.transform_data = Some(
                    serde_json::from_value(serde_json::json!({
                        "input_from": "{{accumulated_while_waiting}}",
                        "fields": [{ "target": "seen", "source": "$" }],
                    }))
                    .unwrap(),
                );
                leaf.steps.push(probe);
            },
            |_| {},
        )
        .await;
        let root = stored(&tree.state, &tree.root_id).await;
        let waiting = tree.leaf().await;
        // The marker is dropped in the row: a parent that rebuilt the context
        // would not set it again unless it meant to.
        let child_id = waiting.id.clone();
        tree.state
            .db
            .with_conn(move |conn| {
                let mut child = crate::db::workflows::get_run(conn, &child_id)?.unwrap();
                let mut context = child.trigger_context.take().unwrap();
                context["accumulated_while_waiting"] = serde_json::json!("kept");
                context[crate::workflows::quota_wait::PARENT_RESUMES_KEY] =
                    serde_json::json!(false);
                conn.execute(
                    "UPDATE workflow_runs SET trigger_context = ?2 WHERE id = ?1",
                    rusqlite::params![child_id, serde_json::to_string(&context)?],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        // The woken child is refused once more. It parks again, rather than
        // failing its parent: the parent marked it resumable again.
        let state = restarted(&tree.state);
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_of(&root)).await,
            vec![tree.root_id.clone()]
        );
        let again = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(
            again.status,
            RunStatus::WaitingQuota,
            "{:?}",
            again.step_results
        );
        assert_eq!(tree.leaf().await.status, RunStatus::WaitingQuota);

        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_of(&again)).await,
            vec![tree.root_id.clone()]
        );
        let done = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
        let resumed = tree.leaf().await;
        assert_eq!(resumed.id, waiting.id);
        assert_eq!(resumed.status, RunStatus::Success);
        let probe = resumed.step_results.last().unwrap();
        assert_eq!(probe.step_name, "probe");
        assert!(probe.output.contains("kept"), "{}", probe.output);
    });
}

/// A child with launch variables keeps the snapshot it started with: the
/// resume resolves nothing again, even if the parent's mapping changed.
#[test]
fn a_resumed_child_keeps_its_pinned_variables() {
    on_big_stack(async {
        let tree = tree_of(
            "single-quota-pinned",
            SESSION_LIMIT,
            1,
            1,
            |leaf| {
                leaf.variables = vec![serde_json::from_value(serde_json::json!({
                    "name": "ticketKey", "label": "Ticket", "placeholder": "",
                }))
                .unwrap()];
            },
            |root| {
                root.steps[1]
                    .sub_workflow_variables
                    .insert("ticketKey".into(), "EW-42".into());
            },
        )
        .await;
        let root = stored(&tree.state, &tree.root_id).await;
        assert_eq!(
            root.status,
            RunStatus::WaitingQuota,
            "{:?}",
            root.step_results
        );
        let waiting = tree.leaf().await;
        let snapshot_of = |state: crate::AppState, id: String| async move {
            state
                .db
                .with_conn(move |conn| {
                    crate::db::execution_variable_snapshots::snapshot_id_for_run(
                        conn, "workflow", &id,
                    )
                })
                .await
                .unwrap()
        };
        let pinned = snapshot_of(tree.state.clone(), waiting.id.clone())
            .await
            .expect("the child started with a snapshot");

        // The parent's mapping changed while it waited.
        let workflow_id = root.workflow_id.clone();
        tree.state
            .db
            .with_conn(move |conn| {
                let mut workflow = crate::db::workflows::get_workflow(conn, &workflow_id)?.unwrap();
                workflow.steps[1]
                    .sub_workflow_variables
                    .insert("ticketKey".into(), "EW-99".into());
                crate::db::workflows::update_workflow(conn, &workflow)?;
                Ok(())
            })
            .await
            .unwrap();

        // Not `restarted`: a snapshot's key lives in the live config.
        let state = tree.state.clone();
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_of(&root)).await,
            vec![tree.root_id.clone()]
        );
        let done = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
        let resumed = tree.leaf().await;
        assert_eq!(resumed.id, waiting.id);
        assert_eq!(resumed.status, RunStatus::Success);
        assert_eq!(
            snapshot_of(state.clone(), resumed.id.clone()).await,
            Some(pinned.clone())
        );
        assert_eq!(
            resumed.trigger_context.unwrap()["execution_snapshot_id"],
            serde_json::json!(pinned)
        );
    });
}
