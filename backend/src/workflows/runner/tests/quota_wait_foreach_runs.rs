//! KT-1556 — a foreach child refused for quota waits instead of failing; its
//! parent mirrors the wait and, once woken, resumes the child at its step.

use super::gate_guard_runs::on_big_stack;
use super::quota_wait_runs::{restarted, stored, SESSION_LIMIT};
use super::*;
use crate::models::{QuotaParkReason, WorkflowGuards};
use std::sync::{Arc, Mutex};

const ITEMS: [&str; 3] = ["T1", "T2", "T3"];

/// Answers every child prompt, except T2's review while `refusals` remain and
/// the `failing` prompt, which always fails like a broken agent.
pub(super) struct ReviewRefused {
    pub(super) refusal: String,
    pub(super) refusals: Mutex<usize>,
    pub(super) failing: Option<&'static str>,
    pub(super) resumed_probe: Option<ActiveTimeProbe>,
    pub(super) prompts: Arc<Mutex<Vec<String>>>,
}

/// Holds T2's answered review for `hold`, then reads the active time its
/// child row holds while that step is still in flight.
pub(super) struct ActiveTimeProbe {
    db: Arc<crate::db::Database>,
    root_id: String,
    hold: std::time::Duration,
    seen_ms: Arc<Mutex<Option<u64>>>,
}

impl ActiveTimeProbe {
    async fn read(&self) {
        tokio::time::sleep(self.hold).await;
        let root = self.root_id.clone();
        let children = self
            .db
            .with_conn(move |conn| descendants(conn, &root))
            .await
            .unwrap();
        let active = children
            .iter()
            .find(|child| item_of(child) == Some("T2"))
            .and_then(|child| child.state.get(TIMEOUT_ACTIVE_MS_STATE_KEY))
            .and_then(|raw| raw.parse().ok());
        *self.seen_ms.lock().unwrap() = active;
    }
}

fn item_of(run: &WorkflowRun) -> Option<&str> {
    run.trigger_context
        .as_ref()
        .and_then(|ctx| ctx.get("__subwf_item_id__"))
        .and_then(serde_json::Value::as_str)
}

/// Every sub-workflow run under `root`, at any depth.
pub(super) fn descendants(
    conn: &rusqlite::Connection,
    root: &str,
) -> anyhow::Result<Vec<WorkflowRun>> {
    let mut found = Vec::new();
    let mut parents = vec![root.to_string()];
    while let Some(parent) = parents.pop() {
        for child in crate::db::workflows::subworkflow_children(conn, &parent)? {
            parents.push(child.id.clone());
            found.push(child);
        }
    }
    Ok(found)
}

#[async_trait::async_trait]
impl crate::acp::AcpTransport for ReviewRefused {
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
        crate::acp::AcpSessionTarget::new(crate::acp::AcpAgent::ClaudeCode, "review-turn")
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
        let marker = ITEMS
            .iter()
            .flat_map(|id| [format!("PREP-{id}"), format!("REVIEW-{id}")])
            .find(|marker| prompt.contains(marker.as_str()))
            .unwrap_or_else(|| "other".into());
        self.prompts.lock().unwrap().push(marker.clone());
        if self.failing == Some(marker.as_str()) {
            return Err(crate::acp::AcpError::Transport(
                "Agent exited with code 1: panic in tool".into(),
            ));
        }
        if marker == "REVIEW-T2" {
            let refused = {
                let mut left = self.refusals.lock().unwrap();
                let refused = *left > 0;
                *left = left.saturating_sub(1);
                refused
            };
            if refused {
                return Err(crate::acp::AcpError::Transport(self.refusal.clone()));
            }
            if let Some(probe) = &self.resumed_probe {
                probe.read().await;
            }
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

struct Tree {
    state: crate::AppState,
    root_id: String,
    prompts: Arc<Mutex<Vec<String>>>,
    seen_ms: Arc<Mutex<Option<u64>>>,
    _route: crate::agents::runner::test_acp_routes::RouteGuard,
    _repo: tempfile::TempDir,
}

impl Tree {
    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    async fn child(&self, item: &str) -> WorkflowRun {
        let (root, item) = (self.root_id.clone(), item.to_string());
        let mut matching: Vec<WorkflowRun> = self
            .state
            .db
            .with_conn(move |conn| descendants(conn, &root))
            .await
            .unwrap()
            .into_iter()
            .filter(|child| item_of(child) == Some(item.as_str()))
            .collect();
        assert_eq!(matching.len(), 1, "one child run per item");
        matching.remove(0)
    }
}

#[derive(Default)]
struct Shape {
    failing: Option<&'static str>,
    resumed_hold: Option<std::time::Duration>,
    /// The foreach runs inside a single-mode sub-workflow.
    under_single_mode: bool,
    child_timeout_seconds: Option<u64>,
}

/// A root (linear, its own worktree, default guards) whose foreach runs
/// `prepare` then `review` per item; the provider refuses T2's review
/// `refusals` times.
async fn tree(id: &str, refusal: &str, refusals: usize) -> Tree {
    tree_shaped(id, refusal, refusals, Shape::default()).await
}

async fn tree_shaped(id: &str, refusal: &str, refusals: usize, shape: Shape) -> Tree {
    let (state, _, _) = test_state_and_configs();
    let repo = git_project(&state, &format!("proj-{id}")).await;
    std::fs::write(
        repo.path().join("tasks.json"),
        r#"[{"id":"T1"},{"id":"T2"},{"id":"T3"}]"#,
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
    child.name = "review one".into();
    let mut prepare = fake_step("prepare");
    prepare.prompt_template = "PREP-{{current_task.id}}".into();
    let mut review = fake_step("review");
    review.prompt_template = "REVIEW-{{current_task.id}}".into();
    child.steps = vec![prepare, review];
    child.guards = shape.child_timeout_seconds.map(|seconds| WorkflowGuards {
        timeout_seconds: Some(seconds),
        ..Default::default()
    });
    let child_db = child.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &child_db))
        .await
        .unwrap();

    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = format!("wf-{id}");
    workflow.project_id = Some(format!("proj-{id}"));
    workflow.guards = Some(WorkflowGuards {
        max_llm_calls: Some(50),
        ..Default::default()
    });
    let mut fanout = fake_step("review_each");
    fanout.step_type = StepType::SubWorkflow;
    fanout.sub_workflow_id = Some(child.id.clone());
    fanout.sub_workflow_foreach_file = Some("tasks.json".into());
    if shape.under_single_mode {
        let mut middle = make_workflow_with_artifacts(Default::default());
        middle.id = format!("middle-{id}");
        middle.name = "sweep".into();
        middle.steps = vec![fanout];
        let middle_db = middle.clone();
        state
            .db
            .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &middle_db))
            .await
            .unwrap();
        let mut single = fake_step("sweep");
        single.step_type = StepType::SubWorkflow;
        single.sub_workflow_id = Some(middle.id.clone());
        workflow.steps = vec![single];
    } else {
        workflow.steps = vec![fanout];
    }
    let root_id = format!("run-{id}");

    let prompts = Arc::new(Mutex::new(Vec::new()));
    let seen_ms = Arc::new(Mutex::new(None));
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
            failing: shape.failing,
            resumed_probe: shape.resumed_hold.map(|hold| ActiveTimeProbe {
                db: state.db.clone(),
                root_id: root_id.clone(),
                hold,
                seen_ms: seen_ms.clone(),
            }),
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
        prompts,
        seen_ms,
        _route: route,
        _repo: repo,
    }
}

fn names(prompts: &[&str]) -> Vec<String> {
    prompts.iter().map(|p| p.to_string()).collect()
}

#[test]
fn a_foreach_child_refused_for_quota_waits_and_resumes_at_its_step_through_its_parent() {
    on_big_stack(async {
        let tree = tree("subrun-quota", SESSION_LIMIT, 1).await;

        // The parent waits with its child: neither failed nor finished.
        let root = stored(&tree.state, &tree.root_id).await;
        assert_eq!(root.status, RunStatus::WaitingQuota);
        assert_eq!(root.finished_at, None);
        let fanout = root.step_results.last().unwrap();
        assert_eq!(fanout.status, RunStatus::WaitingQuota);
        assert!(
            fanout.output.contains("en attente de quota"),
            "{}",
            fanout.output
        );
        let root_wait = fanout
            .quota_wait
            .clone()
            .expect("the parent mirrors the wait");

        let waiting = tree.child("T2").await;
        assert_eq!(waiting.status, RunStatus::WaitingQuota);
        assert_eq!(waiting.finished_at, None);
        assert_eq!(fanout.child_run_id.as_deref(), Some(waiting.id.as_str()));
        assert_eq!(waiting.step_results.len(), 2);
        assert_eq!(waiting.step_results[0].status, RunStatus::Success);
        assert_eq!(waiting.step_results[1].step_name, "review");
        assert_eq!(waiting.step_results[1].status, RunStatus::WaitingQuota);
        let child_wait = waiting.step_results[1].quota_wait.clone().unwrap();
        let wake_at = child_wait.wake_at.expect("9:40pm Europe/Paris is a reset");
        assert_eq!(root_wait.wake_at, Some(wake_at));
        assert_eq!(root_wait.reset_at, child_wait.reset_at);
        assert_ne!(root_wait.id, child_wait.id, "each run has its own wait");
        assert_eq!(tree.child("T1").await.status, RunStatus::Success);
        // T3 is not dispatched into the same limit.
        assert_eq!(
            tree.prompts(),
            names(&["PREP-T1", "REVIEW-T1", "PREP-T2", "REVIEW-T2"])
        );

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
        let summary = &done.step_results.last().unwrap().output;
        assert!(summary.contains("3 ok / 0 échec(s)"), "{summary}");
        let resumed = tree.child("T2").await;
        assert_eq!(resumed.id, waiting.id, "the same child run resumes");
        assert_eq!(resumed.status, RunStatus::Success);
        assert_eq!(resumed.step_results.len(), 2);
        assert_eq!(resumed.step_results[1].quota_wait, None);
        // T1 and T2's prepare are not replayed.
        assert_eq!(
            tree.prompts(),
            names(&[
                "PREP-T1",
                "REVIEW-T1",
                "PREP-T2",
                "REVIEW-T2",
                "REVIEW-T2",
                "PREP-T3",
                "REVIEW-T3"
            ])
        );

        // A stale copy of the old child wait claims nothing.
        let mut stale = waiting.clone();
        assert!(try_claim_interrupted_run_row(&state, &mut stale, None)
            .await
            .is_err());
        assert_eq!(tree.child("T2").await.status, RunStatus::Success);
    });
}

#[test]
fn a_foreach_child_without_a_reset_parks_with_its_parent_until_a_manual_resume() {
    on_big_stack(async {
        let refusal = "[Agent provider error] You've hit your usage limit. Upgrade to Pro or try again at 3:05 PM. (HTTP 429)";
        let tree = tree("subrun-quota-parked", refusal, 1).await;
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
        let child_wait = tree.child("T2").await.step_results[1]
            .quota_wait
            .clone()
            .unwrap();
        assert_eq!(child_wait.parked, Some(QuotaParkReason::NoResetTime));
        assert_eq!(child_wait.reset_at, None);

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
        assert_eq!(tree.child("T2").await.status, RunStatus::Success);
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
fn a_tree_cancelled_while_a_child_waits_is_never_woken() {
    on_big_stack(async {
        let tree = tree("subrun-quota-cancel", SESSION_LIMIT, 1).await;
        let wake_at = stored(&tree.state, &tree.root_id)
            .await
            .step_results
            .last()
            .and_then(|step| step.quota_wait.as_ref())
            .and_then(|wait| wait.wake_at)
            .unwrap();
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
        assert_eq!(tree.child("T2").await.status, RunStatus::Cancelled);
        assert_eq!(
            tree.prompts(),
            names(&["PREP-T1", "REVIEW-T1", "PREP-T2", "REVIEW-T2"])
        );
    });
}

/// A waiting child holds its workflow's concurrency slot: once its parent can
/// no longer resume it, the engine fails it instead of leaving it waiting.
#[test]
fn a_waiting_child_whose_parent_ended_fails_instead_of_holding_its_slot() {
    on_big_stack(async {
        let tree = tree("subrun-quota-orphan", SESSION_LIMIT, 1).await;
        let wake_at = tree.child("T2").await.step_results[1]
            .quota_wait
            .as_ref()
            .and_then(|wait| wait.wake_at)
            .unwrap();

        // While the parent waits, the child is left alone.
        crate::workflows::quota_wait::wake_due_runs(
            &tree.state,
            wake_at - chrono::Duration::seconds(1),
        )
        .await;
        assert_eq!(tree.child("T2").await.status, RunStatus::WaitingQuota);

        let root_id = tree.root_id.clone();
        assert!(tree
            .state
            .db
            .with_conn(move |conn| crate::db::workflows::claim_run_status(
                conn,
                &root_id,
                &RunStatus::WaitingQuota,
                &RunStatus::Failed,
            ))
            .await
            .unwrap());
        let woken = crate::workflows::quota_wait::wake_due_runs(&tree.state, wake_at).await;
        assert!(woken.is_empty());
        let orphan = tree.child("T2").await;
        assert_eq!(orphan.status, RunStatus::Failed);
        assert!(orphan.finished_at.is_some());
        let step = &orphan.step_results[1];
        assert_eq!(step.status, RunStatus::Failed);
        let wait = step.quota_wait.clone().unwrap();
        assert_eq!(wait.parked, Some(QuotaParkReason::NotResumable));
        assert_eq!(wait.wake_at, None);
        assert_eq!(
            tree.prompts(),
            names(&["PREP-T1", "REVIEW-T1", "PREP-T2", "REVIEW-T2"])
        );
    });
}

fn wake_of(run: &WorkflowRun) -> chrono::DateTime<Utc> {
    run.step_results
        .last()
        .and_then(|step| step.quota_wait.as_ref())
        .and_then(|wait| wait.wake_at)
        .expect("a scheduled wake")
}

/// Each wake resumes the waiting item only: an item that genuinely failed
/// earlier in the sweep keeps its failure and is never replayed.
#[test]
fn an_item_failed_before_the_quota_wait_is_counted_not_replayed_on_each_wake() {
    on_big_stack(async {
        let shape = Shape {
            failing: Some("REVIEW-T1"),
            ..Shape::default()
        };
        let tree = tree_shaped("subrun-quota-failed", SESSION_LIMIT, 2, shape).await;
        let failed = tree.child("T1").await;
        assert_eq!(failed.status, RunStatus::Failed);
        let first = stored(&tree.state, &tree.root_id).await;
        assert_eq!(first.status, RunStatus::WaitingQuota);

        let state = restarted(&tree.state);
        let wake_at = wake_of(&first);
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_at).await,
            vec![tree.root_id.clone()]
        );
        // Refused again: the parent waits a second time.
        let second = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(second.status, RunStatus::WaitingQuota);
        let wake_at = wake_of(&second);
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_at).await,
            vec![tree.root_id.clone()]
        );

        let done = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
        let summary = &done.step_results.last().unwrap().output;
        assert!(summary.contains("2 ok / 1 échec(s)"), "{summary}");
        let still_failed = tree.child("T1").await;
        assert_eq!(still_failed.id, failed.id);
        assert_eq!(still_failed.status, RunStatus::Failed);
        assert_eq!(
            tree.prompts(),
            names(&[
                "PREP-T1",
                "REVIEW-T1",
                "PREP-T2",
                "REVIEW-T2",
                "REVIEW-T2",
                "REVIEW-T2",
                "PREP-T3",
                "REVIEW-T3"
            ])
        );
    });
}

/// Default two-hour Timeout, reset three hours away: the wake is scheduled, the
/// pause spends none of it, for the child as for its parent, and the child's
/// step checkpoints.
#[test]
fn a_child_with_the_default_timeout_resumes_after_a_reset_past_it() {
    on_big_stack(async {
        let hold = std::time::Duration::from_millis(600);
        let shape = Shape {
            resumed_hold: Some(hold),
            ..Shape::default()
        };
        let refusal = "HTTP 429 Too Many Requests; retry-after: 10800";
        let tree = tree_shaped("subrun-quota-default-timeout", refusal, 1, shape).await;
        let root = stored(&tree.state, &tree.root_id).await;
        assert_eq!(root.status, RunStatus::WaitingQuota);
        let wake_at = wake_of(&root);
        assert!(wake_at > Utc::now() + chrono::Duration::hours(2));
        let waiting = tree.child("T2").await;
        assert_eq!(waiting.status, RunStatus::WaitingQuota);
        assert_eq!(wake_of(&waiting), wake_at);
        let carried_ms: u64 = waiting.state[TIMEOUT_ACTIVE_MS_STATE_KEY].parse().unwrap();

        let state = restarted(&tree.state);
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_at).await,
            vec![tree.root_id.clone()]
        );
        let done = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
        let resumed = tree.child("T2").await;
        assert_eq!(resumed.id, waiting.id);
        assert_eq!(resumed.status, RunStatus::Success);

        let in_flight_ms = tree.seen_ms.lock().unwrap().expect("checkpointed");
        assert!(
            in_flight_ms >= carried_ms + 200,
            "{in_flight_ms} vs {carried_ms}"
        );
        let child_ms: u64 = resumed.state[TIMEOUT_ACTIVE_MS_STATE_KEY].parse().unwrap();
        let root_ms: u64 = done.state[TIMEOUT_ACTIVE_MS_STATE_KEY].parse().unwrap();
        assert!(child_ms >= in_flight_ms && child_ms < 60_000, "{child_ms}");
        assert!(root_ms < 60_000, "{root_ms}");
    });
}

/// A foreach inside a single sub-workflow waits too: the whole chain waits and
/// resumes through the root, the refused item at its step.
#[test]
fn a_foreach_child_under_a_single_sub_workflow_waits_through_the_whole_chain() {
    on_big_stack(async {
        let shape = Shape {
            under_single_mode: true,
            ..Shape::default()
        };
        let tree = tree_shaped("subrun-quota-single", SESSION_LIMIT, 1, shape).await;
        let root = stored(&tree.state, &tree.root_id).await;
        assert_eq!(root.status, RunStatus::WaitingQuota);
        let waiting = tree.child("T2").await;
        assert_eq!(waiting.status, RunStatus::WaitingQuota);
        let middle = tree
            .state
            .db
            .with_conn({
                let root_id = tree.root_id.clone();
                move |conn| crate::db::workflows::subworkflow_children(conn, &root_id)
            })
            .await
            .unwrap();
        assert_eq!(middle.len(), 1);
        assert_eq!(middle[0].status, RunStatus::WaitingQuota);
        assert_eq!(
            tree.prompts(),
            names(&["PREP-T1", "REVIEW-T1", "PREP-T2", "REVIEW-T2"])
        );

        let state = restarted(&tree.state);
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_of(&root)).await,
            vec![tree.root_id.clone()]
        );
        let done = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
        let resumed = tree.child("T2").await;
        assert_eq!(resumed.id, waiting.id);
        assert_eq!(resumed.status, RunStatus::Success);
        assert_eq!(
            tree.prompts(),
            names(&[
                "PREP-T1",
                "REVIEW-T1",
                "PREP-T2",
                "REVIEW-T2",
                "REVIEW-T2",
                "PREP-T3",
                "REVIEW-T3"
            ])
        );
    });
}

async fn set_state_key(tree: &Tree, run_id: &str, key: &'static str, value: Option<String>) {
    let run_id = run_id.to_string();
    tree.state
        .db
        .with_conn(move |conn| {
            let mut run = crate::db::workflows::get_run(conn, &run_id)?
                .ok_or_else(|| anyhow::anyhow!("run {run_id} is missing"))?;
            match value {
                Some(value) => run.state.insert(key.to_string(), value),
                None => run.state.remove(key),
            };
            conn.execute(
                "UPDATE workflow_runs SET state = ?2 WHERE id = ?1",
                rusqlite::params![run_id, serde_json::to_string(&run.state)?],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

/// A child that committed its item before its quota wait still has steps to
/// run: the ledger does not count it done, the parent resumes it.
#[test]
fn a_waiting_child_whose_item_reached_the_ledger_still_resumes_its_steps() {
    on_big_stack(async {
        let tree = tree("subrun-quota-ledger", SESSION_LIMIT, 1).await;
        let root = stored(&tree.state, &tree.root_id).await;
        let worktree = root.workspace_path.clone().unwrap();
        git_in(
            std::path::Path::new(&worktree),
            &["commit", "--allow-empty", "-q", "-m", "[T2] prepared"],
        )
        .await;

        let state = restarted(&tree.state);
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_of(&root)).await,
            vec![tree.root_id.clone()]
        );
        let done = wait_until_settled(&state, &tree.root_id).await;
        assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
        assert_eq!(tree.child("T2").await.status, RunStatus::Success);
        assert_eq!(
            tree.prompts(),
            names(&[
                "PREP-T1",
                "REVIEW-T1",
                "PREP-T2",
                "REVIEW-T2",
                "REVIEW-T2",
                "PREP-T3",
                "REVIEW-T3"
            ])
        );
    });
}

/// Wakes a sweep whose T1 failed and T2 waits, after `record` rewrote the
/// parent's mirrored-child record, and checks that T1 is retried.
async fn wake_retries_earlier_failures(
    id: &str,
    record: impl FnOnce(&WorkflowRun) -> Option<String>,
) {
    let shape = Shape {
        failing: Some("REVIEW-T1"),
        ..Shape::default()
    };
    let tree = tree_shaped(id, SESSION_LIMIT, 1, shape).await;
    let root = stored(&tree.state, &tree.root_id).await;
    assert!(crate::workflows::quota_wait::mirrored_child(&root.state, "review_each").is_some());
    let value = record(&tree.child("T2").await);
    let key = "__kronn.quota_mirrored_child.review_each";
    set_state_key(&tree, &tree.root_id, key, value).await;

    let state = restarted(&tree.state);
    assert_eq!(
        crate::workflows::quota_wait::wake_due_runs(&state, wake_of(&root)).await,
        vec![tree.root_id.clone()]
    );
    let done = wait_until_settled(&state, &tree.root_id).await;
    assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
    assert!(crate::workflows::quota_wait::mirrored_child(&done.state, "review_each").is_none());
    assert_eq!(
        tree.prompts(),
        names(&[
            "PREP-T1",
            "REVIEW-T1",
            "PREP-T2",
            "REVIEW-T2",
            "PREP-T1",
            "REVIEW-T1",
            "REVIEW-T2",
            "PREP-T3",
            "REVIEW-T3"
        ]),
        "{id}"
    );
}

fn mirrored_record(child_run_id: &str, wait_id: Option<&str>) -> Option<String> {
    let mut state = std::collections::HashMap::new();
    crate::workflows::quota_wait::remember_mirrored_child(
        &mut state,
        "review_each",
        Some(child_run_id),
        wait_id,
    );
    state.into_values().next()
}

/// Only the wait the parent mirrored marks a quota resume. A child an earlier
/// pass left waiting (its reclaim refused) does not stop a fresh pass from
/// retrying the items that failed.
#[test]
fn a_child_left_waiting_by_another_pass_does_not_spare_earlier_failures() {
    on_big_stack(wake_retries_earlier_failures(
        "subrun-quota-leftover",
        |_| None,
    ));
}

/// The record must name both the waiting child and its current wait.
#[test]
fn a_record_naming_another_child_or_another_wait_spares_no_failure() {
    on_big_stack(async {
        wake_retries_earlier_failures("subrun-quota-other-child", |waiting| {
            let wait = waiting.step_results[1].quota_wait.as_ref().unwrap();
            mirrored_record("another-child", wait.id.as_deref())
        })
        .await;
        wake_retries_earlier_failures("subrun-quota-other-wait", |waiting| {
            mirrored_record(&waiting.id, Some("another-wait"))
        })
        .await;
    });
}

/// The pause is not counted, but active time is: a child whose carried time
/// plus its resumed step overruns its Timeout is still stopped.
#[test]
fn a_resumed_child_that_overruns_its_timeout_is_still_stopped() {
    on_big_stack(async {
        let shape = Shape {
            resumed_hold: Some(std::time::Duration::from_secs(3)),
            child_timeout_seconds: Some(30),
            ..Shape::default()
        };
        let tree = tree_shaped("subrun-quota-overrun", SESSION_LIMIT, 1, shape).await;
        let root = stored(&tree.state, &tree.root_id).await;
        let waiting = tree.child("T2").await;
        set_state_key(
            &tree,
            &waiting.id,
            TIMEOUT_ACTIVE_MS_STATE_KEY,
            Some("29500".into()),
        )
        .await;

        let state = restarted(&tree.state);
        assert_eq!(
            crate::workflows::quota_wait::wake_due_runs(&state, wake_of(&root)).await,
            vec![tree.root_id.clone()]
        );
        let done = wait_until_settled(&state, &tree.root_id).await;
        let summary = &done.step_results.last().unwrap().output;
        assert!(summary.contains("2 ok / 1 échec(s)"), "{summary}");
        let stopped = tree.child("T2").await;
        assert_eq!(stopped.id, waiting.id);
        assert_eq!(stopped.status, RunStatus::StoppedByGuard);
        let review = stopped.step_results.last().unwrap();
        assert!(review.output.contains("Timeout guard"), "{}", review.output);
    });
}
