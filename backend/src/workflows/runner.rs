//! Workflow runner: orchestrates a full workflow run.
//!
//! Creates workspace → runs hooks → executes steps sequentially →
//! executes actions → cleans up workspace.

use anyhow::Result;
use chrono::Utc;
use std::future::Future;

use crate::models::*;
use crate::AppState;

use super::steps::{execute_step, resolve_step_connection, step_model_override, StepOutcome};
use super::template::TemplateContext;
use super::workspace::Workspace;

pub fn fail_document_audit(outcome: &mut StepOutcome, reason: &str) {
    outcome.result.status = RunStatus::Failed;
    outcome.result.condition_result = None;
    outcome.condition_action = None;
    outcome.result.output = format!(
        "Document audit failed. Files and index were preserved; inspect these changes before committing or publishing.\n{reason}\n\nAgent result:\n{}",
        outcome.result.output
    );
    // A quota wait would run the step again over the rejected content.
    outcome.result.quota_wait = None;
    tracing::warn!(target: "kronn::docs_write_filter", "Document audit failed; files preserved: {reason}");
}

/// Engine-reserved state, one key per step: the possible secrets its
/// document audit reported. The run view reads it.
const DOCS_POSSIBLE_SECRETS_STATE_PREFIX: &str = "__kronn.docs_possible_secrets.";
const MAX_REPORTED_DOCUMENTS: usize = 50;
const MAX_REPORTED_LINES: usize = 5;

/// KT-1796 — what the run view shows of the possible secrets a step left:
/// documents and lines, bounded, never content. `None` when there is none.
///
/// A step runs more than once (a loop, a resume, a quota wake) and each audit
/// sees only what that execution changed: what `previous` says of a document
/// this audit did not inspect still holds, and is kept.
pub fn possible_secrets_report(
    previous: Option<&str>,
    audit: &crate::core::docs_write_filter::DocsAudit,
) -> Option<String> {
    let previous = previous.and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
    let known = previous
        .as_ref()
        .and_then(|report| report["documents"].as_array());
    let mut documents: Vec<serde_json::Value> = known
        .into_iter()
        .flatten()
        .filter(|document| {
            document["path"]
                .as_str()
                .is_some_and(|path| !audit.inspected.iter().any(|seen| seen == path))
        })
        .cloned()
        .collect();
    documents.extend(audit.possible_secrets.iter().map(|(path, lines)| {
        serde_json::json!({
            "path": path,
            "lines": lines.iter().take(MAX_REPORTED_LINES).collect::<Vec<_>>(),
            "more_lines": lines.len().saturating_sub(MAX_REPORTED_LINES),
        })
    }));
    // Documents beyond the bound are counted, and that count never goes down:
    // nothing says which of them a later execution cleaned.
    let uncounted = previous
        .as_ref()
        .and_then(|report| report["more_documents"].as_u64());
    let more =
        uncounted.unwrap_or(0) as usize + documents.len().saturating_sub(MAX_REPORTED_DOCUMENTS);
    documents.truncate(MAX_REPORTED_DOCUMENTS);
    if documents.is_empty() && more == 0 {
        return None;
    }
    Some(serde_json::json!({ "documents": documents, "more_documents": more }).to_string())
}

/// KT-1796 — the possible secrets of the runs a step spawned are said on
/// this run too: a child run is a row nobody opens.
async fn adopt_child_possible_secrets(
    state: &AppState,
    run_id: &str,
    step_name: &str,
    run_state: &mut std::collections::HashMap<String, String>,
) {
    let parent = run_id.to_string();
    let children = state
        .db
        .with_read_conn(move |conn| crate::db::workflows::subworkflow_children(conn, &parent))
        .await;
    let children = match children {
        Ok(children) => children,
        Err(error) => {
            tracing::warn!(run_id = %run_id, "possible secrets of the child runs not read: {error}");
            mark_child_possible_secrets_unread(run_state, step_name);
            return;
        }
    };
    replace_child_possible_secrets(run_state, &children);
}

/// The child runs of `step_name` could not be read: said on the run, as a
/// record its reader cannot open, until a later reading replaces it.
pub fn mark_child_possible_secrets_unread(
    run_state: &mut std::collections::HashMap<String, String>,
    step_name: &str,
) {
    run_state.insert(
        format!("{DOCS_CHILD_POSSIBLE_SECRETS_STATE_PREFIX}{step_name}"),
        CHILD_RUNS_UNREAD.to_string(),
    );
}

/// The records adopted before go, those of `children` come: a child run
/// again may have cleaned its documents.
pub fn replace_child_possible_secrets(
    run_state: &mut std::collections::HashMap<String, String>,
    children: &[WorkflowRun],
) {
    // Of the runs read, and the notes left when none could be: a child run
    // no longer listed keeps what it said.
    let read: Vec<String> = children
        .iter()
        .map(|child| format!("{SUB_RUN_MARK}{})", child.id))
        .collect();
    run_state.retain(|key, report| {
        !key.starts_with(DOCS_CHILD_POSSIBLE_SECRETS_STATE_PREFIX)
            || !(report == CHILD_RUNS_UNREAD || read.iter().any(|mark| key.ends_with(mark)))
    });
    run_state.extend(child_possible_secrets(children));
}

/// Run state keys of the records adopted from child runs: apart from the
/// run's own, whose steps can carry any name.
const DOCS_CHILD_POSSIBLE_SECRETS_STATE_PREFIX: &str = "__kronn.docs_child_possible_secrets.";

/// The record left when the child runs of a step could not be read.
const CHILD_RUNS_UNREAD: &str = "unread";

/// What the key of an adopted record carries after its step.
const SUB_RUN_MARK: &str = " (sub-run ";

/// The possible-secret records of `children`, keyed for their parent: the
/// step, then the child run it ran in.
pub fn child_possible_secrets(children: &[WorkflowRun]) -> Vec<(String, String)> {
    let mut adopted = Vec::new();
    for child in children {
        for (key, report) in &child.state {
            // Its own, and those it adopted from its own children.
            let step = key
                .strip_prefix(DOCS_POSSIBLE_SECRETS_STATE_PREFIX)
                .or_else(|| key.strip_prefix(DOCS_CHILD_POSSIBLE_SECRETS_STATE_PREFIX));
            if let Some(step) = step {
                let key = format!(
                    "{DOCS_CHILD_POSSIBLE_SECRETS_STATE_PREFIX}{step}{SUB_RUN_MARK}{})",
                    child.id
                );
                adopted.push((key, report.clone()));
            }
        }
    }
    adopted
}

/// Records a branch a cleanup kept, once: a run resumed in its kept worktree
/// ends on the same branch again.
pub fn record_produced_branch(run: &mut WorkflowRun, preserved: super::workspace::PreservedBranch) {
    let produced = crate::models::ProducedBranch {
        branch_name: preserved.branch_name,
        head_sha: preserved.head_sha,
        ahead: preserved.ahead,
        pushed_upstream: preserved.pushed_upstream,
    };
    match run
        .produced_branches
        .iter_mut()
        .find(|known| known.branch_name == produced.branch_name)
    {
        // A reading git could not make carries no commit: keep the one known.
        Some(_) if produced.head_sha.is_empty() => {}
        Some(known) => *known = produced,
        None => run.produced_branches.push(produced),
    }
}

/// The single place a `terminal_stop` takes effect: Failed, no quota wait, no
/// recovery rule. The rollback chain is skipped where it starts.
pub fn apply_terminal_stop(outcome: &mut StepOutcome) {
    let Some(reason) = outcome.result.terminal_stop.as_deref() else {
        return;
    };
    if !outcome.result.output.contains(reason) {
        outcome.result.output = format!("{reason}\n\n{}", outcome.result.output);
    }
    outcome.result.status = RunStatus::Failed;
    outcome.result.quota_wait = None;
    outcome.result.condition_result = None;
    outcome.condition_action = None;
}

/// A Security limit ends the run at this step, whatever the step's verdict.
fn fail_safety_limit(outcome: &mut StepOutcome, reason: &str) {
    outcome.result.output = format!("{reason}\n\nStep result:\n{}", outcome.result.output);
    outcome.result.terminal_stop = Some(reason.to_string());
    tracing::warn!(target: "kronn::workflow_safety", "{reason}");
}

/// Events emitted during a workflow run for real-time SSE streaming.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "event", content = "data")]
pub enum RunEvent {
    /// A step is about to start executing.
    StepStart {
        step_name: String,
        step_index: usize,
        total_steps: usize,
    },
    /// Partial output from the agent (streamed in real-time).
    StepProgress { text: String },
    /// A step has finished executing.
    StepDone { step_result: Box<StepResult> },
    /// 0.7.0 — A `WorkflowGuards` limit was hit and the run was halted.
    /// Distinct from `RunDone { Failed }`: the frontend uses this to
    /// render the orange shield "Stoppé par garde-fou" badge instead of
    /// the red "Échec" one. `actual` is the value at trigger time
    /// (e.g. seconds elapsed for Timeout, calls counted for MaxLlmCalls,
    /// revisit count for LoopDetection).
    GuardTriggered {
        kind: GuardKind,
        threshold: u64,
        actual: u64,
    },
    /// The entire run has finished.
    RunDone { status: RunStatus },
    /// An error occurred.
    #[allow(dead_code)]
    RunError { error: String },
}

/// Optional sender for real-time progress events.
pub type EventSender = tokio::sync::mpsc::Sender<RunEvent>;

/// Deliver one SSE event without ever coupling workflow liveness to the
/// browser's read rate.
///
/// The channel is deliberately best-effort: a full buffer drops the live
/// event, and a disconnected receiver is a no-op. Durable run state remains
/// available from the database and terminal/step transitions are also
/// broadcast over WebSocket. This includes terminal SSE events: stream close,
/// the WebSocket broadcast and the persisted row are the reliable completion
/// signals, so a future maintainer must not restore an awaited send here.
pub fn try_emit_run_event(events_tx: Option<&EventSender>, event: RunEvent) {
    if let Some(tx) = events_tx {
        let _ = tx.try_send(event);
    }
}

/// Store each tool call a running Agent step reports on its in-flight result,
/// until `stop` fires, the sink closes or the step is no longer in flight.
/// `step_index` is `None` for a step with no in-flight result (a rollback
/// step): nothing is stored then, only the room's live progress is fed.
pub fn spawn_step_activity_publisher(
    db: std::sync::Arc<crate::db::Database>,
    run_id: String,
    step_index: Option<usize>,
    step_name: String,
    mut activity: tokio::sync::watch::Receiver<Option<AgentActivity>>,
    stop: tokio_util::sync::CancellationToken,
    progress: Option<crate::agents::run_progress::RunProgress>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Calls already handed to the room's live progress.
        let mut reported = 0u32;
        loop {
            tokio::select! {
                biased;
                _ = stop.cancelled() => break,
                changed = activity.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
            }
            let Some(latest) = activity.borrow_and_update().clone() else {
                continue;
            };
            if let Some(progress) = &progress {
                // KT-1252 — the room's bubble reads the same activity. Calls
                // that came between two reads count with the latest category.
                let new_calls = latest.calls.saturating_sub(reported);
                reported = reported.max(latest.calls);
                if new_calls == 0 {
                    progress.beat();
                }
                for _ in 0..new_calls.min(64) {
                    progress.tool(&crate::agents::activity::ToolActivityUpdate::of_category(
                        None,
                        latest.category,
                    ));
                }
            }
            let Some(step_index) = step_index else {
                continue;
            };
            let (run_id, step_name) = (run_id.clone(), step_name.clone());
            match db
                .with_conn(move |conn| {
                    crate::db::workflows::set_in_flight_step_activity(
                        conn, &run_id, step_index, &step_name, &latest,
                    )
                })
                .await
            {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => tracing::warn!("step activity not recorded: {error}"),
            }
        }
    })
}

enum InFlightStepOutcome {
    Completed(Box<StepOutcome>),
    Cancelled,
    TimedOut,
}

/// Race one in-flight executor against both operator cancellation and the
/// workflow's monotonic active-execution deadline.
///
/// Ordering is deliberate when multiple branches become ready in the same
/// scheduler turn: an explicit cancellation wins, a completed step is kept,
/// and timeout is the final fallback. `checkpoint` runs every
/// [`timeout_checkpoint_period()`] while the step is in flight.
async fn await_in_flight_step<F, C, CF>(
    step_future: F,
    cancel_token: &tokio_util::sync::CancellationToken,
    deadline: tokio::time::Instant,
    mut checkpoint: C,
) -> InFlightStepOutcome
where
    F: Future<Output = StepOutcome>,
    C: FnMut() -> CF,
    CF: Future<Output = ()>,
{
    tokio::pin!(step_future);
    let mut ticks = tokio::time::interval_at(
        tokio::time::Instant::now() + timeout_checkpoint_period(),
        timeout_checkpoint_period(),
    );
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            _ = cancel_token.cancelled() => return InFlightStepOutcome::Cancelled,
            outcome = &mut step_future => {
                return InFlightStepOutcome::Completed(Box::new(outcome));
            }
            _ = tokio::time::sleep_until(deadline) => return InFlightStepOutcome::TimedOut,
            _ = ticks.tick() => checkpoint().await,
        }
    }
}

/// How much active time a backend crash mid-step can lose from the guard.
fn timeout_checkpoint_period() -> std::time::Duration {
    if crate::core::test_mode::on() {
        std::time::Duration::from_millis(100)
    } else {
        std::time::Duration::from_secs(30)
    }
}

pub const TIMEOUT_ACTIVE_MS_STATE_KEY: &str = "__kronn.timeout_active_ms";

/// Durable active time from earlier execution segments plus a monotonic clock
/// for this segment. A run starts a new segment after every persisted pause,
/// so quota and approval waits never enter the timeout budget.
struct WorkflowTimeoutClock {
    carried: std::time::Duration,
    segment_started: tokio::time::Instant,
}

impl WorkflowTimeoutClock {
    fn resume(run: &WorkflowRun) -> Self {
        let carried_ms = run
            .state
            .get(TIMEOUT_ACTIVE_MS_STATE_KEY)
            .and_then(|raw| raw.parse::<u64>().ok())
            .unwrap_or(0);
        Self {
            carried: std::time::Duration::from_millis(carried_ms),
            segment_started: tokio::time::Instant::now(),
        }
    }

    fn elapsed(&self) -> std::time::Duration {
        self.carried.saturating_add(self.segment_started.elapsed())
    }

    fn deadline(&self, timeout_seconds: u64) -> tokio::time::Instant {
        let remaining =
            std::time::Duration::from_secs(timeout_seconds).saturating_sub(self.carried);
        self.segment_started + remaining
    }

    fn deadline_utc(&self, timeout_seconds: u64) -> chrono::DateTime<Utc> {
        let remaining = self
            .deadline(timeout_seconds)
            .saturating_duration_since(tokio::time::Instant::now());
        Utc::now() + chrono::Duration::from_std(remaining).unwrap_or(chrono::Duration::MAX)
    }

    fn elapsed_ms(&self) -> String {
        u64::try_from(self.elapsed().as_millis())
            .unwrap_or(u64::MAX)
            .to_string()
    }

    fn store(&self, run: &mut WorkflowRun) {
        run.state
            .insert(TIMEOUT_ACTIVE_MS_STATE_KEY.to_string(), self.elapsed_ms());
    }

    /// Persists the active time of a step still in flight, so a backend crash
    /// cannot hand the resumed run the budget that step already spent.
    async fn checkpoint(&self, db: &crate::db::Database, run_id: &str) {
        let run_id = run_id.to_string();
        let elapsed_ms = self.elapsed_ms();
        let written = db
            .with_conn(move |conn| {
                crate::db::workflows::set_run_state_key(
                    conn,
                    &run_id,
                    TIMEOUT_ACTIVE_MS_STATE_KEY,
                    &elapsed_ms,
                    &[RunStatus::Running],
                )
            })
            .await;
        if let Err(error) = written {
            tracing::warn!("could not checkpoint the workflow active time: {error}");
        }
    }
}

/// Whether a fresh top-level run must create a dedicated worktree.
///
/// Isolation is explicitly opt-in. Workspace hooks are the compatibility
/// exception: workflows authored before the flag existed used the presence of
/// hooks to request a worktree, so dropping them would silently change their
/// behaviour.
/// what a worktree of this run is prepared with: the project's
/// recipe, with the workflow's own hooks taking precedence field by field.
/// `None` when neither declares anything, which is exactly today's behaviour.
fn resolved_workspace_hooks(
    project: Option<&crate::models::WorkspaceHooks>,
    workflow: Option<&WorkspaceConfig>,
) -> Option<crate::models::WorkspaceHooks> {
    let workflow_hooks = workflow.map(|config| &config.hooks);
    let merged = match project {
        Some(project) => crate::models::WorkspaceHooks::inherit(project, workflow_hooks),
        None => workflow_hooks.cloned().unwrap_or_default(),
    };
    (!merged.is_empty()).then_some(merged)
}

fn workflow_requests_workspace(config: Option<&WorkspaceConfig>) -> bool {
    config.is_some_and(|config| {
        config.require_isolation
            || workflow_base_ref(Some(config)).is_some()
            || config.hooks.after_create.is_some()
            || config.hooks.before_run.is_some()
            || config.hooks.after_run.is_some()
            || config.hooks.before_remove.is_some()
    })
}

/// The starting point a fresh run asked for. The main checkout can only serve
/// its own HEAD, so a base implies a worktree.
fn workflow_base_ref(config: Option<&WorkspaceConfig>) -> Option<&str> {
    config
        .and_then(|config| config.base_ref.as_deref())
        .map(str::trim)
        .filter(|base_ref| !base_ref.is_empty())
}

pub const UNCERTAIN_SIDE_EFFECT_STATE_KEY: &str = "__kronn.uncertain_side_effect";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct SideEffectIntent {
    version: u8,
    step_name: String,
    step_index: usize,
    step_type: String,
    started_at: chrono::DateTime<Utc>,
}

pub fn uncertain_side_effect_type(step_type: &StepType) -> Option<&'static str> {
    match step_type {
        StepType::ApiCall => Some("ApiCall"),
        StepType::BatchApiCall => Some("BatchApiCall"),
        StepType::Notify => Some("Notify"),
        StepType::Exec => Some("Exec"),
        StepType::PublishPageData => Some("PublishPageData"),
        StepType::CollectApiData => Some("CollectApiData"),
        StepType::TriggerWorkflow => Some("TriggerWorkflow"),
        // Replaying an add would create the task twice.
        StepType::TaskBoard => Some("TaskBoard"),
        StepType::Agent
        | StepType::BatchQuickPrompt
        | StepType::Gate
        // Asks a question; changes nothing.
        | StepType::Decide
        | StepType::JsonData
        | StepType::TransformData
        | StepType::SubWorkflow
        // Re-entry reads the campaign back; nothing launches twice.
        | StepType::DelegateSubtasks => None,
    }
}

pub fn record_side_effect_intent(
    run: &mut WorkflowRun,
    step: &WorkflowStep,
    step_index: usize,
) -> Result<bool> {
    let Some(step_type) = uncertain_side_effect_type(&step.step_type) else {
        return Ok(false);
    };
    let intent = SideEffectIntent {
        version: 1,
        step_name: step.name.clone(),
        step_index,
        step_type: step_type.to_string(),
        started_at: Utc::now(),
    };
    run.state.insert(
        UNCERTAIN_SIDE_EFFECT_STATE_KEY.to_string(),
        serde_json::to_string(&intent)?,
    );
    Ok(true)
}

fn uncertain_side_effect_intent(run: &WorkflowRun) -> Option<SideEffectIntent> {
    run.state
        .get(UNCERTAIN_SIDE_EFFECT_STATE_KEY)
        .and_then(|raw| serde_json::from_str(raw).ok())
}

/// 2026-06-11 (Phase 1b-ii) — LLM-calls budget shared across a sub-workflow
/// tree. Without it, each child run reset its own `max_llm_calls` counter, so
/// a nested orchestration could spend depth × cap LLM calls — a token bomb.
/// The ROOT run creates one (`root`) from its resolved guards; every
/// descendant `execute_run` is handed a CLONE (same `Arc` counter + same
/// cap), so the WHOLE tree shares one quota governed by the root's limit.
#[derive(Clone)]
pub struct SharedBudget {
    llm_calls: std::sync::Arc<std::sync::atomic::AtomicU32>,
    max_llm_calls: u32,
    /// The earliest timeout any runner of this tree enforces on the current
    /// branch, with that runner's timeout: guards captured when each run
    /// started, never re-read from saved config.
    deadline: Option<(chrono::DateTime<Utc>, u64)>,
    /// The run whose row durably records the tree's count (KT-1046).
    root_run_id: Option<std::sync::Arc<str>>,
    /// The highest total that row is known to hold.
    recorded: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

impl SharedBudget {
    /// Fresh budget for a top-level run, capped at the run's resolved limit.
    pub fn root(max_llm_calls: u32) -> Self {
        Self {
            llm_calls: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            max_llm_calls,
            deadline: None,
            root_run_id: None,
            recorded: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }
    /// The same budget, recorded durably on `root_run_id`'s row.
    pub fn recorded_on(mut self, root_run_id: &str) -> Self {
        self.root_run_id = Some(root_run_id.into());
        self
    }
    pub fn root_run_id(&self) -> Option<&str> {
        self.root_run_id.as_deref()
    }
    /// Calls spent that the root's row does not hold yet: none may be
    /// dispatched on top of them.
    pub fn has_unrecorded_calls(&self) -> bool {
        self.root_run_id.is_some()
            && self.recorded.load(std::sync::atomic::Ordering::SeqCst) < self.llm_calls()
    }
    /// The same budget, bounded also by a runner whose timeout guard of
    /// `timeout_seconds` ends at `deadline`; the earlier deadline wins.
    pub fn within_deadline(&self, deadline: chrono::DateTime<Utc>, timeout_seconds: u64) -> Self {
        let mut bounded = self.clone();
        if bounded
            .deadline
            .is_none_or(|(current, _)| deadline < current)
        {
            bounded.deadline = Some((deadline, timeout_seconds));
        }
        bounded
    }
    /// The earliest enforced deadline and the timeout it comes from.
    pub fn deadline(&self) -> Option<(chrono::DateTime<Utc>, u64)> {
        self.deadline
    }
    pub fn llm_calls(&self) -> u32 {
        self.llm_calls.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn add_llm_calls(&self, n: u32) {
        self.llm_calls
            .fetch_add(n, std::sync::atomic::Ordering::Relaxed);
    }
    /// The tree-wide cap (the root run's `max_llm_calls`, inherited by every
    /// descendant — a child's own `max_llm_calls` is ignored when nested).
    pub fn max_llm_calls(&self) -> u32 {
        self.max_llm_calls
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod shared_budget_deadline_tests {
    use super::SharedBudget;

    #[test]
    fn the_earliest_runner_deadline_wins_down_the_tree() {
        let now = chrono::Utc::now();
        let root = SharedBudget::root(5).within_deadline(now + chrono::Duration::seconds(60), 60);
        let child = root.within_deadline(now + chrono::Duration::seconds(3600), 3600);
        assert_eq!(
            child.deadline(),
            Some((now + chrono::Duration::seconds(60), 60))
        );
        let tighter = child.within_deadline(now + chrono::Duration::seconds(10), 10);
        assert_eq!(
            tighter.deadline(),
            Some((now + chrono::Duration::seconds(10), 10))
        );
        assert_eq!(
            root.deadline(),
            Some((now + chrono::Duration::seconds(60), 60))
        );
        assert_eq!(SharedBudget::root(5).deadline(), None);
    }
}

/// Compute the workflow-step index where execution should pick up.
///
/// Fresh runs (no prior results) → 0. Resuming runs → the index of the step
/// that follows the last recorded result *in the workflow definition*, not
/// in the result vector — Goto loops can produce more results than there
/// are declared steps, so `step_results.len()` is wrong on resume.
///
/// Returns `workflow.steps.len()` when the last result's step name is no
/// longer in the workflow (the workflow was edited mid-run): treat the run
/// as already past the end so the runner gracefully wraps up rather than
/// re-running steps the operator removed.
pub fn next_step_index_for_resume(
    steps: &[crate::models::WorkflowStep],
    step_results: &[crate::models::StepResult],
) -> usize {
    match step_results.last() {
        None => 0,
        // The pre-start approval names no step: the run begins at the first one.
        Some(last) if last.step_name == super::safety::APPROVAL_STEP => 0,
        Some(last) => steps
            .iter()
            .position(|s| s.name == last.step_name)
            .map(|i| {
                if matches!(
                    last.status,
                    RunStatus::Running | RunStatus::Pending | RunStatus::WaitingQuota
                ) {
                    // A durable StepStart placeholder means the process died
                    // before the step produced a terminal result. Resume must
                    // re-run that step, not skip it as if it had completed.
                    i
                } else {
                    i + 1
                }
            })
            .unwrap_or(steps.len()),
    }
}

/// How many `step_results` to KEEP when a Gate RequestChanges sends the
/// run back to a target step. Cut at the FIRST result that IS the target
/// step (by name): everything before the target's first execution is, by
/// construction, the clean linear prefix `steps[0..target]`, so the
/// resume cursor (`next_step_index_for_resume`, keyed on the LAST KEPT
/// row's name) lands exactly on the target.
///
/// 2026-06-13 (run-10 live bug) — this used to cut at the MOST RECENT
/// occurrence (2026-06-10 audit P1). After a Goto debate loop
/// (`triage → … → plan_review → Goto(triage) → …`), that kept the FIRST
/// round's downstream rows (`plan_lint`, `plan_review`) in the prefix;
/// the resume cursor then keyed off `plan_review` and re-entered the run
/// AT THE GATE — the human's "request changes" never reached the triage.
/// Falls back to the bounded positional index when the target never ran.
pub fn request_changes_cut(
    step_results: &[crate::models::StepResult],
    target_step_name: Option<&str>,
    fallback_idx: usize,
) -> usize {
    target_step_name
        .and_then(|tn| step_results.iter().position(|r| r.step_name == tn))
        .unwrap_or_else(|| fallback_idx.min(step_results.len()))
}

/// Gate feedback, runtime injection (approach B). When a human Gate sends a run
/// back with "request changes", `decide_run` stashes the comment in
/// `state["last_human_feedback"]`. This prepends it to the re-run target step's
/// prompt and CONSUMES it (removes from state) so only that one step gets it —
/// every preset and every hand-built workflow surfaces the feedback without a
/// `{{state.last_human_feedback}}` placeholder. Returns true when injected.
/// Extracted from the run loop so the consume-once contract is unit-testable.
pub fn inject_and_consume_gate_feedback(
    prompt_template: &mut String,
    state: &mut std::collections::HashMap<String, String>,
) -> bool {
    match state.remove("last_human_feedback") {
        Some(fb) if !fb.trim().is_empty() => {
            *prompt_template = format!(
                "⚠️ L'humain a relu et demandé ces changements — adresse-les EN PRIORITÉ avant le reste :\n{}\n\n---\n\n{}",
                fb.trim(),
                prompt_template
            );
            true
        }
        _ => false,
    }
}

/// Last word of a detached launcher whose `execute_run`/`resume_run` returned
/// `Err`: the run becomes Failed with the reason instead of staying Running,
/// where it would keep counting against `concurrency_limit`.
pub async fn settle_errored_run(
    state: &AppState,
    workflow: &Workflow,
    run: &mut WorkflowRun,
    error: &anyhow::Error,
) {
    tracing::error!(run_id = %run.id, "Workflow run failed: {error:#}");
    // The durable status decides, not the in-memory one: a gate resume
    // claimed Running in the DB before its copy left WaitingApproval.
    let reason = format!("Workflow run aborted: {error:#}");
    run.status = RunStatus::Failed;
    run.finished_at = Some(Utc::now());
    run.step_results.push(StepResult {
        step_name: "__run_error__".to_string(),
        status: RunStatus::Failed,
        output: reason,
        tokens_used: Some(0),
        duration_ms: 0,
        started_at: None,
        condition_result: None,
        envelope_detected: None,
        step_kind: None,
        step_api_plugin_slug: None,
        step_api_endpoint_path: None,
        is_rollback: false,
        child_run_id: None,
        agent_provenance: None,
        native_tool_calls: Box::default(),
        step_agent: None,
        step_model: None,
        cached_prompt_tokens: None,
        cache_write_prompt_tokens: None,
        last_activity: None,
        quota_wait: None,
        terminal_stop: None,
        output_bytes: None,
    });
    let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
    let run_id = run.id.clone();
    let settled = state
        .db
        .with_conn(move |conn| {
            let updated = crate::db::workflows::update_run_progress(conn, snap)?;
            if updated {
                crate::db::execution_variable_snapshots::purge_run_lifetime_snapshot(
                    conn,
                    "workflow",
                    &run_id,
                    Utc::now(),
                )?;
                return Ok(None);
            }
            // Blocked by the status guard (already terminal, paused, or a
            // Cancel won the race): keep the durable status.
            Ok(crate::db::workflows::get_run(conn, &run_id)?.map(|row| row.status))
        })
        .await;
    // The execute_run wrapper reclaimed the worktree before this settle made
    // the run terminal.
    if matches!(settled, Ok(None)) {
        forget_removed_workspace(state, run).await;
    }
    match settled {
        Ok(None) => {
            let _ = state
                .ws_broadcast
                .send(crate::models::WsMessage::WorkflowRunUpdated {
                    run_id: run.id.clone(),
                    workflow_id: workflow.id.clone(),
                    status: format!("{:?}", run.status),
                    step_index: run.step_results.len() as i32 - 1,
                    total_steps: workflow.steps.len() as u32,
                    current_step: None,
                });
            let _ = state
                .ws_broadcast
                .send(crate::models::WsMessage::SharedRunUpdated {
                    run_id: run.id.clone(),
                });
        }
        Ok(Some(status)) => {
            run.step_results.pop();
            run.status = status;
        }
        Err(persist_error) => {
            tracing::error!(run_id = %run.id, "could not record the run failure: {persist_error:#}");
        }
    }
}

/// Runs above this one in its sub-workflow chain, nearest first.
async fn ancestor_run_ids(state: &AppState, run: &WorkflowRun) -> Vec<String> {
    let Some(parent) = run.parent_run_id.clone() else {
        return Vec::new();
    };
    let lookup = state
        .db
        .with_conn(move |conn| {
            use rusqlite::OptionalExtension;
            let mut chain = Vec::new();
            let mut next = Some(parent);
            while let Some(id) = next.take() {
                if chain.contains(&id) || chain.len() >= 32 {
                    break;
                }
                next = conn
                    .query_row(
                        "SELECT parent_run_id FROM workflow_runs WHERE id = ?1",
                        [&id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten();
                chain.push(id);
            }
            Ok(chain)
        })
        .await;
    lookup.unwrap_or_else(|error| {
        tracing::warn!(run_id = %run.id, "could not read the run's ancestors: {error}");
        run.parent_run_id.clone().into_iter().collect()
    })
}

fn main_tree_refusal_message(refusal: &crate::workflows::workspace::MainTreeRefusal) -> String {
    use crate::workflows::workspace::MainTreeRefusal;
    match refusal {
        MainTreeRefusal::Busy { holder, waited } => format!(
            "Refusing to run in the main checkout: run {holder} still held it after {}s of waiting \
             (two non-isolated runs would contaminate each other's files and .kronn/ state). \
             Enable worktree isolation, declare the workflow as not writing the main tree, \
             or raise {}.",
            waited.as_secs(),
            crate::workflows::workspace::MAIN_TREE_WAIT_ENV
        ),
        MainTreeRefusal::HeldByAncestor { holder } => format!(
            "Refusing to run in the main checkout: parent run {holder} is executing there and \
             waits for this run. Enable worktree isolation on this workflow."
        ),
        MainTreeRefusal::Cancelled => {
            "Workflow run cancelled while waiting for the main checkout".into()
        }
    }
}

/// The failed preflight result recorded when a run cannot get its worktree.
fn workspace_failure_result(msg: &str) -> StepResult {
    StepResult {
        step_name: "__workspace__".to_string(),
        status: RunStatus::Failed,
        output: msg.to_string(),
        tokens_used: Some(0),
        duration_ms: 0,
        started_at: None,
        condition_result: None,
        envelope_detected: None,
        step_kind: Some("Preflight".into()),
        step_api_plugin_slug: None,
        step_api_endpoint_path: None,
        is_rollback: false,
        child_run_id: None,
        agent_provenance: None,
        native_tool_calls: Box::default(),
        step_agent: None,
        step_model: None,
        cached_prompt_tokens: None,
        cache_write_prompt_tokens: None,
        last_activity: None,
        quota_wait: None,
        terminal_stop: None,
        output_bytes: None,
    }
}

/// The Security limit the run's changes exceed; a failed measurement counts as one.
async fn safety_breach(
    workflow: &Workflow,
    work_dir: &str,
    baseline: &super::safety::ChangeBaseline,
) -> Option<String> {
    let dir = std::path::PathBuf::from(work_dir);
    let baseline = baseline.clone();
    let measured = tokio::task::spawn_blocking(move || super::safety::measure(&dir, &baseline))
        .await
        .map_err(anyhow::Error::from)
        .and_then(|measured| measured);
    match measured {
        Ok(stats) => super::safety::limit_breach(&workflow.safety, stats),
        Err(error) => Some(format!(
            "Could not measure the run's changes against the Security limits: {error}"
        )),
    }
}

/// Keeps the tree's starting state on the run, so a resumed run measures from it.
async fn record_safety_baseline(
    run: &mut WorkflowRun,
    dir: &std::path::Path,
) -> std::result::Result<(), String> {
    let dir = dir.to_path_buf();
    let baseline = tokio::task::spawn_blocking(move || super::safety::capture_baseline(&dir))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    let raw = serde_json::to_string(&baseline).map_err(|error| error.to_string())?;
    run.state
        .insert(super::safety::BASELINE_STATE_KEY.to_string(), raw);
    Ok(())
}

/// Ends a run the Security settings refuse, before any step executes.
async fn refuse_run_for_safety(
    state: &AppState,
    run: &mut WorkflowRun,
    msg: &str,
    emit: &impl Fn(RunEvent),
) -> Result<()> {
    run.status = RunStatus::Failed;
    run.finished_at = Some(Utc::now());
    let mut refusal = super::safety::refusal_result(msg);
    refusal.terminal_stop = Some(msg.to_string());
    run.step_results.push(refusal);
    let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
    state
        .db
        .with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
        .await?;
    emit(RunEvent::RunError {
        error: msg.to_string(),
    });
    Ok(())
}

/// A run its preflight refuses: failed and finished, with nothing run.
async fn refuse_at_preflight(
    state: &AppState,
    run: &mut WorkflowRun,
    msg: String,
    step_kind: &str,
    emit: &impl Fn(RunEvent),
) -> Result<()> {
    run.status = RunStatus::Failed;
    run.finished_at = Some(Utc::now());
    run.step_results.push(StepResult {
        step_name: "__preflight__".to_string(),
        status: RunStatus::Failed,
        output: msg.clone(),
        tokens_used: Some(0),
        duration_ms: 0,
        started_at: None,
        condition_result: None,
        envelope_detected: None,
        step_kind: Some(step_kind.into()),
        step_api_plugin_slug: None,
        step_api_endpoint_path: None,
        is_rollback: false,
        child_run_id: None,
        agent_provenance: None,
        native_tool_calls: Box::default(),
        step_agent: None,
        step_model: None,
        cached_prompt_tokens: None,
        cache_write_prompt_tokens: None,
        last_activity: None,
        quota_wait: None,
        terminal_stop: None,
        output_bytes: None,
    });
    let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
    state
        .db
        .with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
        .await?;
    emit(RunEvent::RunError { error: msg });
    Ok(())
}

/// The definition a run executes against: the run's launch project wins over
/// the workflow's, so a global workflow keeps its project across a gate pause,
/// a restart, or an edit of the workflow while the run waited.
pub fn workflow_in_run_project<'a>(
    workflow: &'a Workflow,
    run: &WorkflowRun,
) -> std::borrow::Cow<'a, Workflow> {
    match run.project_id.as_deref() {
        Some(project_id) if workflow.project_id.as_deref() != Some(project_id) => {
            let mut resolved = workflow.clone();
            resolved.project_id = Some(project_id.to_string());
            std::borrow::Cow::Owned(resolved)
        }
        _ => std::borrow::Cow::Borrowed(workflow),
    }
}

/// Execute a complete workflow run.
#[allow(clippy::too_many_arguments)]
pub async fn execute_run(
    state: AppState,
    workflow: &Workflow,
    run: &mut WorkflowRun,
    tokens_config: &TokensConfig,
    agents_config: &AgentsConfig,
    events_tx: Option<EventSender>,
    // 2026-06-11 Phase 1b-ii — `Some` when this run is a sub-workflow child
    // (inherits the parent tree's shared LLM-calls budget). `None` for a
    // top-level run → a fresh budget is created from its resolved guards.
    shared_budget: Option<SharedBudget>,
    // 2026-06-11 Phase 2 (worktree handoff) — `Some(path)` when this run is a
    // sub-workflow child that must SHARE the parent's git worktree instead of
    // creating its own. The child then commits to the parent's branch, so a
    // later parent step (e.g. `create_pr`) sees the child's implementation.
    // The child ATTACHES (no create), skips the `before_run` hook, and NEVER
    // cleans up / preserves the worktree — the parent owns its lifecycle.
    // `None` for a top-level run or an isolated child.
    inherited_workspace: Option<String>,
) -> Result<()> {
    execute_run_with_notify_policy(
        state,
        workflow,
        run,
        tokens_config,
        agents_config,
        events_tx,
        shared_budget,
        inherited_workspace,
        NotifySecurityPolicy::Production,
    )
    .await
}

/// The gate's webhook URL is templated, so it may come from step output:
/// sent through the guarded transport, no redirect may leave its origin.
async fn fire_gate_webhook(
    url: &str,
    payload: &serde_json::Value,
    policy: crate::core::safe_http::SafeHttpPolicy,
) -> Result<u16, String> {
    use crate::core::safe_http::{self, ClientOptions, Redirects};
    let url = reqwest::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    let client = safe_http::client(
        policy,
        ClientOptions::new(Redirects::SameOrigin).timeout(std::time::Duration::from_secs(10)),
    )?;
    let response = client
        .post(url)
        .json(payload)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(response.status().as_u16())
}

/// The production binary has only the enforcing variant. The loopback variant
/// is compiled exclusively into Rust tests, so no environment variable, API
/// payload or runtime configuration can weaken Notify's SSRF boundary.
#[derive(Clone, Copy)]
pub enum NotifySecurityPolicy {
    Production,
    #[cfg(any(test, feature = "test-support"))]
    AllowLoopbackForTests,
}

impl NotifySecurityPolicy {
    fn enforce_public_ip(self) -> bool {
        match self {
            Self::Production => true,
            #[cfg(any(test, feature = "test-support"))]
            Self::AllowLoopbackForTests => false,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_run_with_notify_policy(
    state: AppState,
    workflow: &Workflow,
    run: &mut WorkflowRun,
    tokens_config: &TokensConfig,
    agents_config: &AgentsConfig,
    events_tx: Option<EventSender>,
    shared_budget: Option<SharedBudget>,
    inherited_workspace: Option<String>,
    notify_security_policy: NotifySecurityPolicy,
) -> Result<()> {
    // KT-1096 — every execution of a run, resumes included, runs the
    // revision pinned at its first one.
    // KT-920 — the repository profiles the tree reads are resolved first,
    // outside any connection; the pin stores and hashes those snapshots.
    let profiles = resolve_pin_profiles(&state, workflow, run).await?;
    let pinned = {
        let (definition, snapshot) = (workflow.clone(), run.clone());
        state
            .db
            .with_conn(move |conn| {
                super::run_pins::pin_or_load_with_profiles(conn, &definition, &snapshot, &profiles)
            })
            .await?
    };
    let pinned = match pinned {
        Ok(pinned) => pinned,
        Err(reason) => {
            if inherited_workspace.is_none() {
                // The refused definition's own hooks never run.
                let mut unhooked = workflow_in_run_project(workflow, run).into_owned();
                unhooked.workspace_config = None;
                if let Err(error) = reclaim_run_worktree(&state, &unhooked, run).await {
                    tracing::warn!(run_id = %run.id, "worktree not removed: {error}");
                }
            }
            anyhow::bail!(reason);
        }
    };
    let workflow = &pinned;
    let is_inherited_workspace = inherited_workspace.is_some();
    // Boxed: the run body's state machine is too large for a caller's stack.
    let result = Box::pin(execute_run_body(
        state.clone(),
        workflow,
        run,
        tokens_config,
        agents_config,
        events_tx,
        shared_budget,
        inherited_workspace,
        notify_security_policy,
    ))
    .await;
    // KT-910 — the artifacts directory goes with the run once it can no
    // longer execute; a paused or interrupted run keeps it.
    if !matches!(
        run.status,
        RunStatus::Pending
            | RunStatus::Running
            | RunStatus::WaitingApproval
            | RunStatus::WaitingQuota
            | RunStatus::Interrupted
    ) || (result.is_err()
        && !matches!(
            run.status,
            RunStatus::WaitingApproval | RunStatus::WaitingQuota
        ))
    {
        super::run_artifacts::remove(&run.id);
    }
    // An error settles the run as failed: its worktree goes now, not at the
    // next boot. An inherited worktree belongs to the parent run.
    if result.is_err()
        && !is_inherited_workspace
        && !matches!(
            run.status,
            RunStatus::WaitingApproval | RunStatus::WaitingQuota
        )
    {
        let workflow = workflow_in_run_project(workflow, run);
        if let Err(error) = reclaim_run_worktree(&state, &workflow, run).await {
            tracing::warn!(
                run_id = %run.id,
                error = %error,
                "worktree of an errored run not removed — the next boot cleanup will retry"
            );
        }
    }
    if result.is_ok() {
        super::run_effect::record(&state, run).await;
    }
    result
}

/// What a run tells its reader when its end kept a checkout holding work.
pub fn kept_checkout_note(kept: &super::workspace::KeptCheckout) -> String {
    format!(
        "Worktree `{}` kept with its branch: {}. Commit, push or discard what it holds (unlock it if git says it is locked), or remove it yourself (`git worktree remove --force`); Kronn removes it at a later start once it is clean.",
        kept.path.display(),
        kept.reason
    )
}

/// Finished runs whose checkout is gone no longer own a worktree. Clearing
/// the durable path lets run retention reach them without the boot janitor; a
/// checkout still on disk, or a run not yet terminal in the database, keeps it.
pub async fn forget_removed_workspace(state: &AppState, run: &WorkflowRun) {
    let Some(path) = run.workspace_path.clone() else {
        return;
    };
    if std::path::Path::new(&path).exists() {
        return;
    }
    if let Err(error) = state
        .db
        .with_conn(move |conn| crate::db::workflows::forget_removed_workspace(conn, &path))
        .await
    {
        tracing::warn!(run_id = %run.id, "could not clear the removed worktree path: {error}");
    }
}

/// Removes the worktree a run owns once it will not execute again, keeping its
/// branch when it holds commits no known base has (recorded on the run).
/// The repository profile of every project the run tree will read and has
/// not pinned yet, read from git now.
async fn resolve_pin_profiles(
    state: &AppState,
    workflow: &Workflow,
    run: &WorkflowRun,
) -> Result<crate::core::project_profile::ProfileSnapshots> {
    let (definition, snapshot) = (workflow.clone(), run.clone());
    let keys = state
        .db
        .with_read_conn(move |conn| {
            super::run_pins::profile_projects_to_resolve(conn, &definition, &snapshot)
        })
        .await?;
    crate::core::project_profile::snapshots_for_keys(&state.db, keys).await
}

async fn reclaim_run_worktree(
    state: &AppState,
    workflow: &Workflow,
    run: &mut WorkflowRun,
) -> Result<()> {
    let Some(ws_path) = run.workspace_path.as_ref().map(std::path::PathBuf::from) else {
        return Ok(());
    };
    if !ws_path.exists() {
        return Ok(());
    }
    let owner_run_id = run.id.clone();
    let child_dispatches = state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::active_child_dispatch_count(conn, &owner_run_id)
        })
        .await?;
    if child_dispatches > 0 {
        anyhow::bail!("{child_dispatches} child dispatch(es) still read this worktree");
    }
    // `before_remove` is a hook like the others: the project's recipe
    // applies here too.
    let Some(pid) = workflow.project_id.clone() else {
        return Ok(());
    };
    let Some(project) = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
        .await?
    else {
        return Ok(());
    };
    let repo_path = crate::core::scanner::resolve_host_path(&project.path);
    let project_hooks = project.workspace.map(|workspace| workspace.hooks);
    let ws = Workspace::attach(
        ws_path,
        repo_path,
        &workflow.name,
        &run.id,
        resolved_workspace_hooks(project_hooks.as_ref(), workflow.workspace_config.as_ref()),
    );
    // KT-1096 — this run did not succeed: a checkout holding work stays.
    let outcome = ws.cleanup_keeping_work().await?;
    if let Some(kept) = &outcome.kept {
        tracing::warn!(run_id = %run.id, "{}", kept_checkout_note(kept));
    }
    if let Some(preserved) = outcome.preserved {
        record_produced_branch(run, preserved);
        let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
        if let Err(e) = state
            .db
            .with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
            .await
        {
            tracing::error!(
                run_id = %run.id,
                error = %e,
                "failed to persist produced_branches — the preserved branch pointer is the only record of this run's work"
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn execute_run_body(
    state: AppState,
    workflow: &Workflow,
    run: &mut WorkflowRun,
    tokens_config: &TokensConfig,
    agents_config: &AgentsConfig,
    events_tx: Option<EventSender>,
    shared_budget: Option<SharedBudget>,
    inherited_workspace: Option<String>,
    notify_security_policy: NotifySecurityPolicy,
) -> Result<()> {
    let timeout_clock = WorkflowTimeoutClock::resume(run);
    let approval_workflow = workflow.clone();
    state
        .db
        .with_read_conn(move |conn| {
            crate::core::repository_resources::ensure_workflow_execution_approved(
                conn,
                &approval_workflow,
            )
            .map_err(anyhow::Error::msg)
        })
        .await?;
    let mut workflow_in_project = workflow_in_run_project(workflow, run);
    // KT-1025 — the agents chosen at launch live on the run, so a Gate approval
    // or a resume, which reload the definition, still run on them.
    if super::step_agents::from_run(run).is_some() {
        let mut chosen = workflow_in_project.into_owned();
        super::step_agents::apply_from_run(&mut chosen, run);
        workflow_in_project = std::borrow::Cow::Owned(chosen);
    }
    // KT-851 — a multi-project workflow keeps Quick Prompt/API references
    // symbolic; they resolve in this run's project.
    if crate::core::resource_refs::has_structured_references(&workflow_in_project) {
        let mut resolved = workflow_in_project.into_owned();
        let project_id = resolved.project_id.clone();
        let pinned_run = run.id.clone();
        let resolved_steps = state
            .db
            .with_read_conn(move |conn| {
                // KT-1096 — a pinned run keeps the resolutions it recorded.
                if !super::run_pins::apply_pinned_structured(conn, &pinned_run, &mut resolved)? {
                    crate::core::resource_refs::resolve_run_structured_references(
                        conn,
                        &mut resolved,
                        project_id.as_deref(),
                    )?;
                }
                Ok(resolved)
            })
            .await?;
        workflow_in_project = std::borrow::Cow::Owned(resolved_steps);
    }
    let workflow: &Workflow = &workflow_in_project;
    // Captured once: drives the attach-vs-create and the skip-cleanup paths.
    let is_inherited_workspace = inherited_workspace.is_some();
    // SSE is an optional live projection, never part of the execution
    // contract. A slow connected browser must not suspend the workflow.
    let emit = |event: RunEvent| try_emit_run_event(events_tx.as_ref(), event);

    // 0.8.2 — Broadcast the run state to ALL connected WS clients so
    // WorkflowDetail pages opened in another tab pick up step transitions
    // (especially the Running → WaitingApproval flip) without a refresh.
    // The SSE channel (`emit` above) only feeds the tab that triggered
    // the run; cross-tab updates need WS. Best-effort: ignore send errors
    // when no client is connected.
    let workflow_id_for_ws = workflow.id.clone();
    let run_id_for_ws = run.id.clone();
    let total_steps_for_ws = workflow.steps.len() as u32;
    let broadcast_run_state =
        |status: &crate::models::RunStatus, step_index: i32, current_step: Option<String>| {
            let _ = state
                .ws_broadcast
                .send(crate::models::WsMessage::WorkflowRunUpdated {
                    run_id: run_id_for_ws.clone(),
                    workflow_id: workflow_id_for_ws.clone(),
                    status: format!("{:?}", status),
                    step_index,
                    total_steps: total_steps_for_ws,
                    current_step,
                });
            let _ = state
                .ws_broadcast
                .send(crate::models::WsMessage::SharedRunUpdated {
                    run_id: run_id_for_ws.clone(),
                });
        };

    let db = state.db.clone();
    // Snapshot the persisted per-model Ollama ceilings once for the whole
    // run. Every agent spawn in this run (main path, repair, debate and
    // rollback) must resolve the same operator-visible configuration.
    let ollama_context_overrides = state
        .config
        .read()
        .await
        .server
        .ollama_context_overrides
        .clone();

    // Register a cancellation token keyed by the run id. The "⏹ Arrêter" UI
    // triggers this token via POST /api/workflows/.../runs/:run_id/cancel.
    // We check it between steps and during any long await — short-circuiting
    // to status = Cancelled. The CancelGuard auto-cleans on scope exit.
    let cancel_guard = crate::CancelGuard::insert(&state.cancel_registry, run.id.clone());
    let cancel_token = cancel_guard.token.clone();

    // ADR-005 slice 1 (KT-847) — skills/directives/profiles this run
    // resolves stay pinned to what it first loaded for the whole call.
    // Released when this call returns (completion, Gate pause, error or cancel).
    let _resource_snapshot_guard =
        crate::core::resource_snapshot::RunSnapshotGuard::new(run.id.clone());
    let seeded_run_id = run.id.clone();
    db.with_read_conn(move |conn| super::run_pins::seed_resource_snapshots(conn, &seeded_run_id))
        .await?;

    // Update run status to Running. `false` = the Cancelled-stickiness guard
    // blocked the write: the user cancelled in the window between our caller
    // claiming the run (insert / gate-resume claim) and this line. Without
    // this check the write resurrected a Cancelled row to Running (fresh
    // token, never cancelled) and the run executed to completion.
    run.status = RunStatus::Running;
    let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
    let db2 = db.clone();
    let claimed = db2
        .with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
        .await?;
    if !claimed {
        tracing::info!(
            "Run {} was cancelled (or deleted) before execution started — aborting",
            run.id
        );
        run.status = RunStatus::Cancelled;
        return Ok(());
    }

    let safety_refusal =
        super::safety::sandbox_refusal(&workflow.safety, super::safety::in_container())
            .or_else(|| super::safety::child_approval_refusal(&workflow.safety, run));
    if let Some(msg) = safety_refusal {
        return refuse_run_for_safety(&state, run, &msg, &emit).await;
    }
    // Before any worktree or hook: a refused run must have run nothing.
    if super::safety::approval_pending(&workflow.safety, run) {
        let approval = super::safety::approval_result();
        emit(RunEvent::StepDone {
            step_result: Box::new(approval.clone()),
        });
        run.step_results.push(approval);
        run.status = RunStatus::WaitingApproval;
        let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
        db.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
            .await?;
        emit(RunEvent::RunDone {
            status: run.status.clone(),
        });
        broadcast_run_state(&run.status, run.step_results.len() as i32 - 1, None);
        return Ok(());
    }

    // E part 2 (TD-20260717-run-power-assertion-sleep) — hold the shared,
    // refcounted power assertion for the whole active run so a laptop that
    // sleeps mid-run no longer freezes it or trips a `__guard_timeout__`.
    // RAII: this named binding lives until the function returns, so it is
    // released on EVERY terminal/pause exit below (Success/Failed/Cancelled/
    // StoppedByGuard/WaitingApproval). Acquired only AFTER the run is claimed
    // Running — a run cancelled before it starts keeps nothing awake.
    let _power_lease = crate::core::power_guard::acquire();

    // KT-1252 — the room this run's Agent step will enter shows the workflow
    // and the step it executes meanwhile, from the first moment a launch
    // variable or a literal names it. Released with the run, whichever way it
    // ends. The loop below announces again as steps complete.
    let room_targets = super::step_room::RoomTargetWatch::new(&state.workflow_step_rooms, &run.id);
    {
        let mut launch_ctx = TemplateContext::with_time_anchor(run.started_at);
        if let Some(ref trigger_ctx) = run.trigger_context {
            inject_trigger_context(&mut launch_ctx, trigger_ctx);
        }
        room_targets
            .announce(
                &db,
                run.project_id.as_deref().or(workflow.project_id.as_deref()),
                workflow,
                next_step_index_for_resume(&workflow.steps, &run.step_results),
                &launch_ctx,
            )
            .await;
    }

    // Resolve the project once: its path, and how it prepares a
    // worktree. A worktree is invisible to containers mounted on the main
    // checkout, so a validation run in one reads the wrong code and passes;
    // the recipe that fixes that belongs to the project, not to each of its
    // workflows.
    // KT-926 — an Agent step's prompt carries nothing about the user's other
    // Kronn projects: it goes to the model provider, so the only repos a prompt
    // may name are the ones the user linked to the project themselves.
    let (project_path, project_hooks) = if let Some(ref pid) = workflow.project_id {
        let pid_clone = pid.clone();
        let db3 = db.clone();
        let project_opt = db3
            .with_conn(move |conn| crate::db::projects::get_project(conn, &pid_clone))
            .await?;
        match project_opt {
            Some(project) => (
                project.path,
                project.workspace.map(|workspace| workspace.hooks),
            ),
            None => (String::new(), None),
        }
    } else {
        (String::new(), None)
    };
    // KT-918 — declared scripts are read from the carrying repository's
    // checkout, never from the run's worktree.
    let declares_scripts = workflow
        .steps
        .iter()
        .chain(workflow.on_failure.iter())
        .any(|step| !step.exec_script_files.is_empty());
    // The launch may already carry the run's project in `project_id`; the
    // stored definition keeps the home project, which carries the scripts.
    let carrying_project_id = if declares_scripts {
        let workflow_id = workflow.id.clone();
        db.with_read_conn(move |conn| crate::db::workflows::get_workflow(conn, &workflow_id))
            .await?
            .map(|stored| stored.project_id)
            .unwrap_or_else(|| workflow.project_id.clone())
    } else {
        None
    };
    let carrying_repository: Option<String> = match carrying_project_id {
        Some(pid) if declares_scripts => {
            let path = if workflow.project_id.as_deref() == Some(pid.as_str()) {
                Some(project_path.clone())
            } else {
                db.with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
                    .await?
                    .map(|project| project.path)
            };
            path.filter(|path| !path.is_empty()).map(|path| {
                let resolved = crate::core::scanner::resolve_host_path(&path);
                if resolved.exists() {
                    resolved.to_string_lossy().to_string()
                } else {
                    path
                }
            })
        }
        _ => None,
    };

    // 0.7.0 Phase 4 — detect resume: a non-empty step_results means
    // this is a continuation from a Gate pause (or a future restart-
    // recovery). On resume we reattach to the existing worktree
    // instead of creating a new one — both because it already holds
    // the artifacts the operator just inspected, and because creating
    // a second worktree on the same branch would fail.
    // `workspace_path` is only ever set by a PRIOR execute_run (fresh runs
    // arrive with None), so its presence also marks a resume — covers a run
    // interrupted during its FIRST step (no result persisted yet) and a gate
    // RequestChanges that truncates back to step 0: both must re-attach the
    // preserved worktree, not create a second one.
    let is_resume = run
        .step_results
        .iter()
        .any(|result| result.step_name != super::safety::APPROVAL_STEP)
        || run.workspace_path.is_some();

    // Isolation is opt-in. Before KT-343 every project-linked workflow tried
    // to create a worktree even when `require_isolation` was false, which made
    // read-only fire-and-forget fan-outs trip the isolated-parent safety guard.
    // Hooks are the one compatibility exception: legacy hook-only configs
    // still request a worktree because silently dropping after_create / etc.
    // would be a different behavioral regression.
    let workspace_requested = workflow_requests_workspace(workflow.workspace_config.as_ref());

    // Pre-flight: validate every Agent step's agent is actually installed
    // (or runtime-available) before we start. Without this the run fails
    // mid-execution at the spawn site with a confusing subprocess error
    // ("vibe: command not found" or worse), wasting work that's been done
    // by earlier steps. Mirrors the same guard added to the multi-agent
    // debate orchestrator. We skip this on resume — the workflow is
    // already partway through and the user may have intentionally
    // uninstalled an agent that's only used in skipped branches.
    //
    // KT-2055: it reads each step as it will run (Quick Prompt hydrated) and
    // covers every agent the run will certainly start whose failure fails
    // it: the `multi_agent_review` reviewers and the sub-workflows' steps.
    // What it cannot see — a resumed run, shadows, fallback links,
    // `on_failure` — is refused by the same rule at dispatch (`execute_step`).
    // KT-2057: it runs before the worktree and its `after_create` hook (a
    // refused run has run nothing), and covers the agent a batch's items run.
    if !is_resume {
        let planned = super::run_preflight::planned_agent_steps(&state, workflow, run).await;
        if !planned.is_empty() {
            let detections = crate::agents::detect_all_cached(false).await;
            let usable: Vec<crate::models::AgentType> = detections
                .iter()
                .filter(|d| (d.installed || d.runtime_available) && d.enabled)
                .map(|d| d.agent_type.clone())
                .collect();
            // A test ACP route stands in for the agent binary; without this the
            // preflight would depend on what the host happens to have installed.
            // Routes are keyed by the directory the agents will run in, known
            // here unless the run creates a fresh worktree.
            #[cfg(any(test, feature = "test-support"))]
            let routed = {
                let repo_path = crate::core::scanner::resolve_host_path(&project_path);
                let main_dir = if repo_path.exists() {
                    repo_path.to_string_lossy().to_string()
                } else {
                    project_path.clone()
                };
                let dir = if project_path.is_empty() || !repo_path.exists() {
                    Some(main_dir)
                } else if let Some(inherited) = inherited_workspace
                    .clone()
                    .filter(|path| std::path::Path::new(path).exists())
                {
                    Some(inherited)
                } else {
                    (!workspace_requested).then_some(main_dir)
                };
                dir.is_some_and(|dir| {
                    crate::agents::runner::resolve_agent_work_dir(Some(&dir), &dir)
                        .is_ok_and(|dir| crate::agents::runner::test_acp_routes::is_routed(&dir))
                })
            };
            #[cfg(not(any(test, feature = "test-support")))]
            let routed = false;
            let mut missing: Vec<String> = Vec::new();
            // KT-2030: `Custom` is a named HTTP connection, never an installed
            // agent; its step can start when its connection can be called.
            // KT-2045: any step naming a connection, and a native HTTP
            // provider step on its own key, is held to the same rule.
            // KT-2055: a LiteLlm/Nvidia step naming a connection runs on it,
            // with no local binary or legacy base_url.
            let mut unusable_connections: Vec<String> = Vec::new();
            let mut unloaded: Vec<String> = Vec::new();
            for planned_step in &planned {
                if routed {
                    continue;
                }
                let step = &planned_step.step;
                // A step whose Quick Prompt does not load fails at dispatch in
                // these words: refused now, before any step runs, and judged on
                // nothing else (it is not read as it would run). A child's is
                // read with this run's pins, which its run inherits.
                if let Some(error) = &planned_step.unloaded {
                    unloaded.push(planned_step.describe(format!("'{}': {error}", step.name)));
                    continue;
                }
                if let Some(needs) = planned_step.missing_agent(&usable, agents_config) {
                    missing.push(needs);
                    continue;
                }
                if let Some(refusal) =
                    super::steps::step_connection_refusal(step, Some(&state.db), tokens_config)
                        .await
                {
                    unusable_connections.push(planned_step.describe(refusal));
                }
                if let Some(cfg) = step.multi_agent_review.as_ref() {
                    let reviewer = super::steps::reviewer_step_for(step, cfg);
                    if let Some(refusal) = super::steps::step_connection_refusal(
                        &reviewer,
                        Some(&state.db),
                        tokens_config,
                    )
                    .await
                    {
                        unusable_connections.push(
                            planned_step.describe(super::steps::reviewer_refusal(cfg, &refusal)),
                        );
                    }
                }
            }
            if !missing.is_empty() || !unusable_connections.is_empty() || !unloaded.is_empty() {
                let mut reasons = Vec::new();
                if !missing.is_empty() {
                    reasons.push(format!(
                        "agent(s) not installed/enabled: {}. Install or enable them in Config before running this workflow.",
                        missing.join(", ")
                    ));
                }
                if !unusable_connections.is_empty() {
                    reasons.push(format!(
                        "HTTP connection(s) or provider key(s) not usable: {}.",
                        unusable_connections.join("; ")
                    ));
                }
                if !unloaded.is_empty() {
                    reasons.push(format!(
                        "Quick Prompt(s) that cannot load: {}.",
                        unloaded.join("; ")
                    ));
                }
                let msg = format!("Workflow refuses to start — {}", reasons.join(" "));
                refuse_at_preflight(&state, run, msg, "Preflight", &emit).await?;
                broadcast_run_state(&run.status, run.step_results.len() as i32 - 1, None);
                return Ok(());
            }

            // Resolve every statically reachable Agent step before the first
            // one starts. Dynamic branches are checked again immediately
            // before dispatch by `execute_step`. A native agent without full
            // access has no catalogue to read: its step fails on that reason.
            // Each step of this run as it will run; a sub-workflow resolves
            // its own steps when it starts, a batch's items when they start.
            let mut catalog_failures = Vec::new();
            for step in planned
                .iter()
                .filter(|planned_step| planned_step.within.is_none() && !planned_step.batch)
                .map(|planned_step| &planned_step.step)
                .filter(|step| super::steps::native_full_access_refusal_for(&step.agent).is_none())
            {
                let tier = step
                    .agent_settings
                    .as_ref()
                    .and_then(|settings| settings.tier)
                    .unwrap_or_default();
                let connection = match resolve_step_connection(step, Some(&state.db)).await {
                    Ok(connection) => connection,
                    Err(error) => {
                        catalog_failures.push((
                            step.name.clone(),
                            serde_json::json!({
                                "error": "connection_resolution_failed",
                                "detail": error.to_string(),
                            }),
                        ));
                        continue;
                    }
                };
                let runtime_target_id = connection.as_ref().map(|connection| {
                    crate::db::model_catalog::http_runtime_target_id(&connection.id)
                });
                let model = step_model_override(step, connection.as_ref());
                let resolution = crate::core::model_catalog::preflight_resolve(
                    &state.db,
                    runtime_target_id.as_deref(),
                    step.agent.clone(),
                    tier,
                    model.as_deref(),
                    Some(&agents_config.model_tiers),
                );
                #[cfg(any(test, feature = "test-support"))]
                let resolution =
                    crate::core::model_catalog::test_routed_discovery(routed, resolution);
                if let Err(failure) = resolution.await {
                    catalog_failures.push((
                        step.name.clone(),
                        serde_json::to_value(failure).unwrap_or_default(),
                    ));
                }
            }
            if !catalog_failures.is_empty() {
                let msg = format!(
                    "preflight_failed:{}",
                    serde_json::to_string(&catalog_failures).unwrap_or_default()
                );
                refuse_at_preflight(&state, run, msg, "preflight_failed", &emit).await?;
                broadcast_run_state(&run.status, run.step_results.len() as i32 - 1, None);
                return Ok(());
            }
        }
    }

    let mut safety_baseline_error: Option<String> = None;

    // Create or attach workspace (if we have a project path). An inherited or
    // resumed workspace remains authoritative even if the workflow definition
    // was edited after the run started.
    let workspace = if !project_path.is_empty() {
        let repo_path = crate::core::scanner::resolve_host_path(&project_path);
        if repo_path.exists() {
            // The workflow's own hooks win field by field; the rest comes
            // from the project.
            let hooks = resolved_workspace_hooks(
                project_hooks.as_ref(),
                workflow.workspace_config.as_ref(),
            );
            if let Some(inh) = inherited_workspace
                .as_ref()
                .map(std::path::PathBuf::from)
                .filter(|p| p.exists())
            {
                // Phase 2 — sub-workflow child shares the parent's worktree.
                // Attach to the parent's branch so commits land there; the
                // child never creates/destroys this tree (parent owns it).
                run.workspace_path = Some(inh.to_string_lossy().to_string());
                Some(Workspace::attach(
                    inh,
                    repo_path,
                    &workflow.name,
                    &run.id,
                    hooks,
                ))
            } else if is_resume {
                match run.workspace_path.as_ref().map(std::path::PathBuf::from) {
                    Some(path) if path.exists() => Some(Workspace::attach(
                        path,
                        repo_path,
                        &workflow.name,
                        &run.id,
                        hooks,
                    )),
                    // The run was isolated: continuing in the main checkout
                    // would let its agents commit and push there.
                    Some(path) => {
                        let msg = format!(
                            "Worktree `{}` no longer exists — refusing to resume in the main checkout. Re-trigger the workflow for a fresh run.",
                            path.display()
                        );
                        run.status = RunStatus::Failed;
                        run.finished_at = Some(Utc::now());
                        run.step_results.push(workspace_failure_result(&msg));
                        let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
                        db.with_conn(move |conn| {
                            crate::db::workflows::update_run_progress(conn, snap)
                        })
                        .await?;
                        emit(RunEvent::RunError { error: msg });
                        broadcast_run_state(&run.status, run.step_results.len() as i32 - 1, None);
                        return Ok(());
                    }
                    None => None, // the run never had a worktree: it ran in the main tree
                }
            } else if !workspace_requested {
                tracing::debug!(
                    run_id = %run.id,
                    workflow_id = %workflow.id,
                    "workflow isolation disabled — running under the main-tree exclusivity guard"
                );
                None
            } else {
                let base_ref = workflow_base_ref(workflow.workspace_config.as_ref());
                let created = match Workspace::create_before_hooks(
                    &repo_path,
                    &workflow.name,
                    &run.id,
                    hooks,
                    base_ref,
                )
                .await
                {
                    Ok(ws) => {
                        // Recorded before the project's hooks: their changes count.
                        if super::safety::limits_set(&workflow.safety) {
                            safety_baseline_error =
                                record_safety_baseline(run, &ws.path).await.err();
                        }
                        ws.after_create().await.map(|()| ws)
                    }
                    Err(error) => Err(error),
                };
                match created {
                    Ok(ws) => {
                        run.workspace_path = Some(ws.path.to_string_lossy().to_string());
                        Some(ws)
                    }
                    Err(e) => {
                        // #8 — worktree fallback is DANGEROUS for code-pushing
                        // workflows: silently running agents that `git push` /
                        // mutate files in the developer's MAIN checkout. When
                        // the workflow declares `require_isolation`, abort the
                        // run instead of falling back (mirror the preflight
                        // failure pattern). Read-only workflows keep the legacy
                        // warn-and-continue behaviour. A requested `base_ref`
                        // never falls back: the main checkout is not at it.
                        let requires_isolation = workflow
                            .workspace_config
                            .as_ref()
                            .map(|c| c.require_isolation)
                            .unwrap_or(false);
                        if requires_isolation || base_ref.is_some() {
                            let msg = match base_ref {
                                Some(base_ref) => format!(
                                    "{e} Refusing to run in the main checkout: it does not start from `{base_ref}`."
                                ),
                                None => format!(
                                    "Workflow requires an isolated git worktree but it could not be created: {}. \
                                     Refusing to run in the main checkout — this workflow pushes/mutates code. \
                                     Check the repo (is it a clean git repo? disk space?), or clear `require_isolation` \
                                     to allow main-tree runs.",
                                    e
                                ),
                            };
                            run.status = RunStatus::Failed;
                            run.step_results.push(StepResult {
                                step_name: "__workspace__".to_string(),
                                status: RunStatus::Failed,
                                output: msg.clone(),
                                tokens_used: Some(0),
                                duration_ms: 0,
                                started_at: None,
                                condition_result: None,
                                envelope_detected: None,
                                step_kind: Some("Preflight".into()),
                                step_api_plugin_slug: None,
                                step_api_endpoint_path: None,
                                is_rollback: false,
                                child_run_id: None,
                                agent_provenance: None,
                                native_tool_calls: Box::default(),
                                step_agent: None,
                                step_model: None,
                                cached_prompt_tokens: None,
                                cache_write_prompt_tokens: None,
                                last_activity: None,
                                quota_wait: None,
                                terminal_stop: None,
                                output_bytes: None,
                            });
                            let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
                            let db_w = db.clone();
                            db_w.with_conn(move |conn| {
                                crate::db::workflows::update_run_progress(conn, snap)
                            })
                            .await?;
                            emit(RunEvent::RunError { error: msg });
                            return Ok(());
                        }
                        tracing::warn!("Failed to create worktree, running in main tree: {}", e);
                        run.state.remove(super::safety::BASELINE_STATE_KEY);
                        safety_baseline_error = None;
                        None
                    }
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // TD-20260709 (A) — a run about to execute in the MAIN checkout takes a
    // per-project exclusivity guard (isolated worktree runs skip it). Held
    // for this execution's lifetime; the checkpoint-reset preflight covers
    // the deferred gate-pause window. Crons of the same minute fire in one
    // tick, so a busy tree is waited for (bounded, FIFO) before refusing.
    let writes_main_tree = !workflow
        .workspace_config
        .as_ref()
        .is_some_and(|config| config.main_tree_read_only);
    let _main_tree_guard = if workspace.is_none() && !project_path.is_empty() && writes_main_tree {
        let ancestors = ancestor_run_ids(&state, run).await;
        let wait = crate::workflows::workspace::main_tree_wait();
        match crate::workflows::workspace::MainTreeGuard::acquire(
            &project_path,
            &run.id,
            &ancestors,
            wait,
            &cancel_token,
        )
        .await
        {
            Ok(g) => Some(g),
            Err(crate::workflows::workspace::MainTreeRefusal::Cancelled) => {
                tracing::info!(
                    "Workflow run {} cancelled while waiting for the main checkout",
                    run.id
                );
                run.status = RunStatus::Cancelled;
                run.step_results.push(StepResult {
                    step_name: "__cancelled_by_user__".to_string(),
                    status: RunStatus::Cancelled,
                    output: "Workflow run cancelled by user while waiting for the main checkout"
                        .to_string(),
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
                });
                run.finished_at = Some(Utc::now());
                let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
                let db_g = db.clone();
                db_g.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
                    .await?;
                emit(RunEvent::RunDone {
                    status: RunStatus::Cancelled,
                });
                broadcast_run_state(&run.status, run.step_results.len() as i32 - 1, None);
                return Ok(());
            }
            Err(refusal) => {
                let msg = main_tree_refusal_message(&refusal);
                run.status = RunStatus::Failed;
                run.step_results.push(StepResult {
                    step_name: "__workspace__".to_string(),
                    status: RunStatus::Failed,
                    output: msg.clone(),
                    tokens_used: Some(0),
                    duration_ms: 0,
                    started_at: None,
                    condition_result: None,
                    envelope_detected: None,
                    step_kind: Some("Preflight".into()),
                    step_api_plugin_slug: None,
                    step_api_endpoint_path: None,
                    is_rollback: false,
                    child_run_id: None,
                    agent_provenance: None,
                    native_tool_calls: Box::default(),
                    step_agent: None,
                    step_model: None,
                    cached_prompt_tokens: None,
                    cache_write_prompt_tokens: None,
                    last_activity: None,
                    quota_wait: None,
                    terminal_stop: None,
                    output_bytes: None,
                });
                run.finished_at = Some(Utc::now());
                let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
                let db_g = db.clone();
                db_g.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
                    .await?;
                emit(RunEvent::RunError { error: msg });
                broadcast_run_state(&run.status, run.step_results.len() as i32 - 1, None);
                return Ok(());
            }
        }
    } else {
        None
    };

    // Determine working directory
    let work_dir = workspace
        .as_ref()
        .map(|ws| ws.path.to_string_lossy().to_string())
        .unwrap_or_else(|| {
            let resolved = crate::core::scanner::resolve_host_path(&project_path);
            if resolved.exists() {
                resolved.to_string_lossy().to_string()
            } else {
                project_path.clone()
            }
        });

    if super::safety::limits_set(&workflow.safety) {
        let refusal = if let Some(error) = safety_baseline_error.take() {
            Some(error)
        } else if work_dir.is_empty() {
            Some("this run has no project directory".to_string())
        } else {
            match run.state.get(super::safety::BASELINE_STATE_KEY) {
                // Absent: this run never had one (fresh, or limits set since).
                None => record_safety_baseline(run, std::path::Path::new(&work_dir))
                    .await
                    .err(),
                // Never recaptured: a new baseline would forgive what the run already changed.
                Some(raw)
                    if serde_json::from_str::<super::safety::ChangeBaseline>(raw).is_err() =>
                {
                    Some("the run's recorded starting state is unreadable".to_string())
                }
                Some(_) => None,
            }
        };
        if let Some(error) = refusal {
            let msg = format!(
                "The max files / max lines limits (Security settings) need the project to be a git working tree: {error}"
            );
            return refuse_run_for_safety(&state, run, &msg, &emit).await;
        }
    }

    // Run before_run hook — but not on resume, the hook already fired
    // before the pause and re-firing it would re-run setup actions
    // (npm install, env preparation, etc.) the operator didn't ask for.
    if !is_resume && !is_inherited_workspace {
        if let Some(ref ws) = workspace {
            if let Err(e) = ws.before_run().await {
                tracing::warn!(
                    run_id = %run.id,
                    error = %e,
                    "before_run hook could not be executed — workspace may be unprepared"
                );
            }
        }
    }

    // 0.7.1 — scan sensitive files once at run start so the docs/ write
    // filter can flag agent-written content that overlaps with .env/.pem/
    // .ssh/credentials. Cheap (only reads small files), happens once per
    // run, results reused across every Agent step audit. Empty when the
    // run has no project (Notify-only / ApiCall-only workflows).
    let sensitive_substrings = std::sync::Arc::new(if work_dir.is_empty() {
        crate::core::docs_write_filter::SensitiveSubstrings::new()
    } else {
        crate::core::docs_write_filter::scan_sensitive_files(std::path::Path::new(&work_dir))
    });

    // The zone pinned with the run (KT-1103): a resume after a Settings
    // change renders the same dates. Unpinned runs read the live zone.
    let zone_run = run.id.clone();
    let run_timezone = state
        .db
        .with_read_conn(move |conn| super::run_pins::pinned_timezone(conn, &zone_run))
        .await?
        .unwrap_or_else(crate::core::timezone::current);
    // Build template context from trigger context
    let mut ctx =
        TemplateContext::with_time_anchor(run.started_at).with_default_timezone(run_timezone);
    if let Some(ref trigger_ctx) = run.trigger_context {
        inject_trigger_context(&mut ctx, trigger_ctx);
    }
    // Execution variables never transit through trigger_context. Load the
    // run-scoped encrypted snapshot directly into this in-memory context. A
    // technical resume uses the same run id and therefore the same snapshot.
    if !workflow.variables.is_empty() {
        let secret = state
            .config
            .read()
            .await
            .encryption_secret
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Variable snapshot key unavailable"))?;
        let key = crate::core::crypto::parse_secret(&secret).map_err(anyhow::Error::msg)?;
        let run_id = run.id.clone();
        let values = state
            .db
            .with_conn(move |conn| {
                crate::db::execution_variable_snapshots::load_values(
                    conn,
                    "workflow",
                    &run_id,
                    &key,
                    Utc::now(),
                )
            })
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("Workflow execution variable snapshot unavailable or expired")
            })?;
        ctx.set_launch_variables(&workflow.variables, values);
    }
    // After trigger fields and variables, so neither can stand in for the run.
    ctx.set_builtin("run.id", run.id.clone());
    ctx.set_builtin("run.variant", super::shadow::variant(run).to_string());
    // KT-1643 — the instance's own address, as a step it spawns reaches it, so
    // a shared workflow carries no literal `localhost:3140`.
    let listening_port = state.config.read().await.server.listening_port();
    ctx.set_builtin(
        "run.kronn_url",
        crate::core::contribution::kronn_url(
            crate::core::child_env::var("KRONN_BACKEND_URL")
                .ok()
                .as_deref(),
            listening_port,
        ),
    );
    ctx.set_refs(&workflow.refs);
    // KT-910 — Exec steps write here, Agent steps that name it read it.
    match super::run_artifacts::ensure(&run.id) {
        Ok(dir) => ctx.set_builtin("run.artifacts_dir", dir.to_string_lossy().to_string()),
        Err(error) => tracing::warn!(
            run_id = %run.id,
            "run artifacts directory unavailable: {error}"
        ),
    }
    // KT-917 — `{{ref:<kind>:<slug>}}` resolves in this run's project; an
    // unresolved one stays unknown so strict rendering fails its step.
    let refs_workflow = workflow.clone();
    let refs_project = workflow.project_id.clone();
    let refs_run = run.id.clone();
    let references = db
        .with_conn(move |conn| {
            // KT-1096 — values recorded at pin time, from the pinned objects.
            if let Some(pinned) = super::run_pins::pinned_template_references(
                conn,
                &refs_run,
                &refs_workflow.id,
                refs_project.as_deref(),
            )? {
                return Ok(pinned);
            }
            crate::core::resource_refs::resolve_template_references(
                conn,
                &refs_workflow,
                refs_project.as_deref(),
            )
        })
        .await?;
    for (reference, id) in references {
        ctx.set(reference, id);
    }
    // KT-920 — `{{project.<path>}}` comes from the main checkout's default
    // branch, read once and pinned: no worktree, PR or pause changes it.
    let profile_repo = (!project_path.is_empty())
        .then(|| crate::core::scanner::resolve_host_path(&project_path))
        .filter(|path| path.exists());
    let profile_reads = crate::core::project_profile::workflow_placeholders(workflow);
    let (profile_run, profile_project) = (run.id.clone(), workflow.project_id.clone());
    let pinned_profile = {
        let (run_id, project) = (profile_run.clone(), profile_project.clone());
        db.with_read_conn(move |conn| {
            super::run_pins::pinned_project_profile(conn, &run_id, project.as_deref())
        })
        .await?
    };
    let pinned_profile = match pinned_profile {
        Some(pinned) => pinned,
        None => {
            let read = tokio::task::spawn_blocking(move || {
                crate::core::project_profile::snapshot(profile_repo.as_deref())
            })
            .await?;
            db.with_conn(move |conn| {
                super::run_pins::record_project_profile(
                    conn,
                    &profile_run,
                    profile_project.as_deref(),
                    read,
                )
            })
            .await?
        }
    };
    let profile = crate::core::project_profile::values_for(&pinned_profile, &profile_reads);
    match profile {
        Ok(values) => {
            for (key, value) in values {
                ctx.set_builtin(key, value);
            }
        }
        Err(msg) => {
            run.status = RunStatus::Failed;
            run.finished_at = Some(Utc::now());
            let mut result = workspace_failure_result(&msg);
            result.step_name = "__project_profile__".to_string();
            run.step_results.push(result);
            let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
            db.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
                .await?;
            emit(RunEvent::RunError { error: msg });
            broadcast_run_state(&run.status, run.step_results.len() as i32 - 1, None);
            return Ok(());
        }
    }
    // 0.7.0 Phase 3 — pre-seed every declared artifact to "" so a step
    // referencing `{{artifacts.review}}` on round 1 (before any step
    // wrote it) renders cleanly rather than leaving the literal
    // `{{artifacts.review}}` placeholder in the prompt. Steps that
    // produce the artifact later overwrite the seed via `set_step_output`.
    for name in workflow.artifacts.keys() {
        ctx.set(format!("artifacts.{}", name), "");
    }
    // 0.7.0 Phase 4 — Gate resume: replay every prior step result into
    // the template context so downstream steps see `{{steps.X.summary}}`
    // and `{{artifacts.Y}}` exactly as if the run had never paused.
    // Fresh runs have no prior step_results, so this is a no-op for them.
    replay_prior_results(&mut ctx, &run.step_results);
    super::shadow_summary::restore(&state, &mut ctx, workflow, run).await;
    // 0.7.0 Phase 6 — seed durable state from the run row. On a fresh
    // run this is empty (no-op); on resume / restart-recovery it carries
    // counters and verdicts the agent wrote in prior iterations so the
    // first step after the pause sees them through `{{state.<k>}}`.
    ctx.seed_state(&run.state);

    let safety_baseline = run
        .state
        .get(super::safety::BASELINE_STATE_KEY)
        .and_then(|raw| serde_json::from_str::<super::safety::ChangeBaseline>(raw).ok());

    // Execute steps sequentially
    let mut all_success = true;
    let mut cancelled_by_user = false;
    let mut stopped_by_guard = false;
    let mut paused_for_approval = false;
    let mut paused_for_quota = false;
    let mut step_idx = next_step_index_for_resume(&workflow.steps, &run.step_results);
    let mut retried_step = None;
    if run.step_results.last().is_some_and(|result| {
        matches!(
            result.status,
            RunStatus::Running | RunStatus::Pending | RunStatus::WaitingQuota
        )
    }) {
        // StepStart is persisted for live observability. After a daemon crash
        // that trailing row is evidence of an incomplete step, not history to
        // keep alongside the retry. Replace it with the new attempt. A step
        // refused for quota (KT-811) is replaced the same way.
        retried_step = run.step_results.pop().map(|result| result.step_name);
    }
    let total_steps = workflow.steps.len();
    let max_total_iterations = max_iterations_for(total_steps); // safeguard against infinite Goto loops

    // 0.7.0 — execution guards. Resolved once at run start so subsequent
    // edits to the workflow (loosen the timeout, raise max calls) don't
    // affect a running instance — the contract is "what was set when you
    // hit Run". Plain backend defaults apply when `workflow.guards` is None.
    let resolved_guards = WorkflowGuards::resolve_optional(workflow.guards.as_ref());
    // Phase 1b-ii — shared LLM-calls budget. A child inherits the parent
    // tree's (same counter + cap); a top-level run gets a fresh one capped at
    // its own resolved limit. The whole tree is then governed by ONE quota.
    let is_root_budget = shared_budget.is_none();
    let budget = shared_budget
        .unwrap_or_else(|| SharedBudget::root(resolved_guards.max_llm_calls).recorded_on(&run.id))
        .within_deadline(
            timeout_clock.deadline_utc(resolved_guards.timeout_seconds),
            resolved_guards.timeout_seconds,
        );
    let timeout_deadline = timeout_clock.deadline(resolved_guards.timeout_seconds);
    // Visits, Goto-edge fires, iterations and LLM calls continue across resumes
    // (KT-1046): a loop through a pause must trip where it would without one.
    let mut guard_counters = super::guard_counters::GuardCounters::resume(
        &run.state,
        &workflow.steps,
        &run.step_results,
        retried_step.as_deref(),
    );
    if is_root_budget {
        // A stale total would let the tree overspend: no count, no run.
        budget.add_llm_calls(root_llm_calls_spent(&state, run, &guard_counters).await?);
        record_tree_llm_calls(&state, &budget).await?;
    }

    while step_idx < workflow.steps.len() {
        // Cancellation check — fires when the user clicked "⏹ Arrêter" and
        // the /cancel endpoint triggered our token. We break BEFORE executing
        // the next step so a runaway linear run doesn't keep burning tokens.
        // Note: a step already in flight won't stop here — agent steps have
        // their own disc-level token checked inside `make_agent_stream`.
        if cancel_token.is_cancelled() {
            tracing::info!(
                "Workflow run {} cancelled by user before step {}",
                run.id,
                step_idx
            );
            cancelled_by_user = true;
            run.step_results.push(StepResult {
                step_name: "__cancelled_by_user__".to_string(),
                status: RunStatus::Cancelled,
                output: "Workflow run cancelled by user".to_string(),
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
            });
            all_success = false;
            break;
        }
        guard_counters.iterations += 1;
        if guard_counters.iterations > max_total_iterations {
            tracing::error!(
                "Workflow run exceeded {} iterations — aborting to prevent infinite loop",
                max_total_iterations
            );
            all_success = false;
            run.step_results.push(StepResult {
                step_name: "__safeguard_abort__".to_string(),
                status: RunStatus::Failed,
                output: format!("Workflow aborted: exceeded {} total step iterations (possible infinite Goto loop)", max_total_iterations),
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
            });
            break;
        }

        // ── WorkflowGuards check (0.7.0) ────────────────────────────────
        // The timeout guard measures active runner time. Persisted pauses
        // start a fresh monotonic segment and keep the elapsed budget carried
        // in run state, so a quota reset or approval wait cannot spend it.
        let elapsed_secs = timeout_clock.elapsed().as_secs();
        if elapsed_secs >= resolved_guards.timeout_seconds {
            tracing::warn!(target: "kronn::workflow_guard",
                run_id = %run.id, kind = "Timeout",
                threshold_secs = resolved_guards.timeout_seconds, actual_secs = elapsed_secs,
                "Workflow run stopped by Timeout guard");
            emit(RunEvent::GuardTriggered {
                kind: GuardKind::Timeout,
                threshold: resolved_guards.timeout_seconds,
                actual: elapsed_secs,
            });
            run.step_results.push(StepResult {
                step_name: "__guard_timeout__".to_string(),
                status: RunStatus::StoppedByGuard,
                output: format!(
                    "Stopped by Timeout guard: {}s elapsed (limit {}s)",
                    elapsed_secs, resolved_guards.timeout_seconds
                ),
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
            });
            stopped_by_guard = true;
            break;
        }

        // LLM-calls quota: counts Agent steps as 1 each. BatchQuickPrompt
        // counts as N **AFTER** the fan-out resolves (the executor returns
        // the actual child count); this check uses the cumulative count
        // accumulated from previous iterations. ApiCall and Notify count
        // as 0 because they don't spend tokens.
        if budget.has_unrecorded_calls() {
            if let Err(error) = record_tree_llm_calls(&state, &budget).await {
                tracing::warn!(target: "kronn::workflow_guard", run_id = %run.id, %error,
                    "Workflow run stopped: its LLM-call count is not recorded");
                let mut row = budget_unrecorded_row(&error);
                row.started_at = Some(Utc::now());
                run.step_results.push(row);
                all_success = false;
                break;
            }
        }
        if budget.llm_calls() >= budget.max_llm_calls() {
            tracing::warn!(target: "kronn::workflow_guard",
                run_id = %run.id, kind = "MaxLlmCalls",
                threshold = budget.max_llm_calls(), actual = budget.llm_calls(),
                "Workflow run stopped by MaxLlmCalls guard (shared budget — counts the whole sub-workflow tree)");
            emit(RunEvent::GuardTriggered {
                kind: GuardKind::MaxLlmCalls,
                threshold: budget.max_llm_calls() as u64,
                actual: budget.llm_calls() as u64,
            });
            run.step_results.push(StepResult {
                step_name: "__guard_max_llm_calls__".to_string(),
                status: RunStatus::StoppedByGuard,
                output: format!(
                    "Stopped by MaxLlmCalls guard: {} LLM calls (limit {})",
                    budget.llm_calls(),
                    budget.max_llm_calls()
                ),
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
            });
            stopped_by_guard = true;
            break;
        }

        // Loop detection: count visits PER step. A 100-step linear
        // workflow doesn't trigger this (each step visited once); a
        // workflow that Goto-loops on the same step does. Senior Dev
        // explicitly rejected "total iter count" as fragile.
        let visit_count = {
            let name = &workflow.steps[step_idx].name;
            let n = guard_counters.visits.entry(name.clone()).or_insert(0);
            *n += 1;
            *n
        };
        guard_counters.llm_calls = budget.llm_calls();
        guard_counters.store(&mut run.state);
        // 0.7.0 Phase 6 — expose `{{iter.<step_name>}}` in templates so
        // a step can react to its own re-execution (e.g. "first pass:
        // generate; subsequent: refine"). Updated EVERY iteration so
        // looped Goto-back patterns see the right counter.
        ctx.set(
            format!("iter.{}", workflow.steps[step_idx].name),
            visit_count.to_string(),
        );
        if visit_count > resolved_guards.loop_detection_max_revisits {
            let step_name = workflow.steps[step_idx].name.clone();
            tracing::warn!(target: "kronn::workflow_guard",
                run_id = %run.id, kind = "LoopDetection", step = %step_name,
                threshold = resolved_guards.loop_detection_max_revisits, actual = visit_count,
                "Workflow run stopped by LoopDetection guard");
            emit(RunEvent::GuardTriggered {
                kind: GuardKind::LoopDetection {
                    step_name: step_name.clone(),
                },
                threshold: resolved_guards.loop_detection_max_revisits as u64,
                actual: visit_count as u64,
            });
            run.step_results.push(StepResult {
                step_name: "__guard_loop_detection__".to_string(),
                status: RunStatus::StoppedByGuard,
                output: format!(
                    "Stopped by LoopDetection guard: step '{}' visited {} times (limit {})",
                    step_name, visit_count, resolved_guards.loop_detection_max_revisits
                ),
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
            });
            stopped_by_guard = true;
            break;
        }

        let step = &workflow.steps[step_idx];
        tracing::info!(
            "Executing step {}/{}: '{}'",
            step_idx + 1,
            total_steps,
            step.name
        );
        room_targets
            .announce(
                &db,
                run.project_id.as_deref().or(workflow.project_id.as_deref()),
                workflow,
                step_idx,
                &ctx,
            )
            .await;

        // Journal the intent BEFORE invoking a mechanically side-effecting
        // executor. If the process dies after the external effect commits but
        // before the StepResult snapshot, boot reconciliation preserves this
        // marker and manual resume fails closed instead of replaying blindly.
        // A crash just before the effect may therefore ask for an unnecessary
        // decision; that conservative false positive is intentional.
        let side_effect_intent_recorded = record_side_effect_intent(run, step, step_idx)?;
        if side_effect_intent_recorded {
            timeout_clock.store(run);
            let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
            let intent_db = db.clone();
            let persisted = intent_db
                .with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
                .await?;
            if !persisted {
                return Err(anyhow::anyhow!(
                    "Run {} changed state before side-effect intent for step '{}' could be persisted; refusing to execute it",
                    run.id,
                    step.name
                ));
            }
        }

        // Persist an observable in-flight row BEFORE entering the executor.
        // Until now StepStart existed only on SSE/WS; DB/MCP readers saw the
        // exact same shape for "queued" and "Agent running". The placeholder
        // is replaced in place by the terminal result below.
        let step_started_at = Utc::now();
        let in_flight_result_index = run.step_results.len();
        let mut in_flight_result = StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Running,
            output: String::new(),
            tokens_used: None,
            duration_ms: 0,
            started_at: Some(step_started_at),
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
        };
        apply_step_snapshot(
            step,
            &mut in_flight_result,
            Some(&agents_config.model_tiers),
        );
        run.step_results.push(in_flight_result);
        timeout_clock.store(run);
        let start_snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
        let start_db = db.clone();
        let persisted = start_db
            .with_conn(move |conn| crate::db::workflows::update_run_progress(conn, start_snap))
            .await?;
        if !persisted {
            return Err(anyhow::anyhow!(
                "Run {} changed state before StepStart for '{}' could be persisted",
                run.id,
                step.name
            ));
        }

        emit(RunEvent::StepStart {
            step_name: step.name.clone(),
            step_index: step_idx,
            total_steps,
        });
        // 0.8.2 — cross-tab live update
        broadcast_run_state(&run.status, step_idx as i32, Some(step.name.clone()));

        // Build the step's executor as a single future, then race it against
        // operator cancellation AND the workflow's active-time deadline. When
        // either stop branch wins, the executor future is dropped and the
        // kill_on_drop chain takes over:
        //   - Agent  → AgentProcess.child drops → SIGKILL
        //   - Exec   → tokio Command future drops → SIGKILL
        //   - HTTP   → reqwest cancels on drop (ApiCall, BatchApiCall, Notify)
        //   - BatchQuickPrompt → child agents kill_on_drop the same way
        // See runner.rs:201-209 for the *between-step* cancel check;
        // this is the *in-flight* counterpart.
        let step_start = std::time::Instant::now();
        // 2026-06-11 Phase 1b — sub-workflow recursion context, extracted as
        // owned values BEFORE the `async` block so the SubWorkflow arm
        // doesn't borrow `run`/`workflow` across the await. `__subwf_depth__`
        // is 0 for a top-level run, N for a run that is itself a child.
        let sub_parent_run_id = run.id.clone();
        let sub_current_depth = run
            .trigger_context
            .as_ref()
            .and_then(|t| t.get("__subwf_depth__"))
            .and_then(|v| v.as_u64())
            // A depth past `usize` (32-bit targets) is past the cap, not 0.
            .map_or(0, |depth| usize::try_from(depth).unwrap_or(usize::MAX));
        // Phase 1b-ii — clone the shared budget for the SubWorkflow arm so the
        // child inherits the tree-wide quota (same counter + cap).
        let sub_budget = budget.clone();
        // Phase 2 — hand the parent's worktree path to the child so it commits
        // to the parent's branch (a later parent step like `create_pr` then
        // sees the child's work). `None` when the parent runs in the main tree.
        let sub_parent_workspace = run.workspace_path.clone();
        // A Gate waits for a person: a trial run must never wait for one.
        let skip_in_shadow = (step.external_effect || matches!(step.step_type, StepType::Gate))
            && super::shadow::is_shadow(run);
        // KT-1459 — a run of the retired workflow-level shadow starts no
        // shadows. The snapshot is taken here: the step's future holds `run`.
        let shadow_snapshot =
            (!step.shadows.is_empty() && !super::shadow::is_shadow(run)).then(|| run.clone());
        let step_future = async {
            if skip_in_shadow {
                return super::shadow::skipped(step);
            }
            match step.step_type {
                StepType::BatchQuickPrompt => {
                    // Phase 2 batch workflows — fan out a Quick Prompt over items
                    // from a previous step's output, then optionally wait for all
                    // children to finish before moving on.
                    super::batch_step::execute_batch_quick_prompt_step(
                        step,
                        &run.id,
                        state.clone(),
                        &ctx,
                    )
                    .await
                }
                StepType::Notify => {
                    // Direct HTTP webhook — zero agent tokens. Used as a workflow
                    // finalizer or mechanical data step (post to Slack, create
                    // ticket, etc.). The alternate policy variant is cfg(test)
                    // only; production always enforces the public-IP guard.
                    super::notify_step::execute_notify_step_with_policy(
                        step,
                        &ctx,
                        notify_security_policy.enforce_public_ip(),
                    )
                    .await
                }
                StepType::ApiCall => {
                    // Désagentification: direct HTTP call from the engine.
                    // Uses `SecurityPolicy::production()` — localhost URLs fail
                    // here, as intended. Rate-limiting lands in P0.5b. Project
                    // id comes from the parent workflow, not the run row, since
                    // `WorkflowRun` doesn't carry it (only `workflow_id`).
                    //
                    // 0.8.6 (#59) — stamp source=workflow + run_id on the
                    // api_call_logs row so the audit table can filter by
                    // run.
                    let api_caller =
                        crate::core::api_access::workflow_caller(&state, workflow).await;
                    super::api_call_executor::execute_api_call_step_with_db_as(
                        step,
                        workflow.project_id.as_deref(),
                        &state,
                        &ctx,
                        super::api_call_executor::SecurityPolicy::production(),
                        super::api_call_executor::ApiCallLogContext::workflow_step(run.id.clone()),
                        &api_caller,
                    )
                    .await
                }
                // KT-1755 — boxed: inlined, this arm's temporaries sit in the
                // poll frame of the step future, which every nested run pays
                // again whatever its step type (a debug build gives each one
                // its own stack slot). Behind the box only an Agent step pays.
                StepType::Agent => Box::pin(async {
                    // 0.7+ — hydrate optional QuickPrompt reference. Le helper
                    // est no-op si `step.quick_prompt_id` est None. Sinon il
                    // injecte prompt_template / tier / skill_ids depuis le QP
                    // dans une copie locale du step (per-field override : le
                    // step gagne quand non-vide).
                    let mut hydrated = step.clone();
                    let hydration = super::quick_prompt_hydrate::hydrate_step_from_quick_prompt(
                        &mut hydrated,
                        &state.db,
                        Some(&run.id),
                    )
                    .await;
                    // Observe documents before launch, including dirty/indexed
                    // content. A failed snapshot refuses the launch rather than
                    // guessing which writes belong to this step afterwards.
                    let docs_before = if hydration.is_ok() && !work_dir.is_empty() {
                        crate::core::docs_write_filter::snapshot_docs(std::path::Path::new(
                            &work_dir,
                        ))
                        .await
                        .map(Some)
                    } else {
                        Ok(None)
                    };
                    // KT-793 — a room that does not resolve refuses the launch.
                    let step_room = if hydration.is_ok() {
                        super::step_room::activate(
                            &state.workflow_step_rooms,
                            &state.db,
                            &run.id,
                            run.project_id.as_deref().or(workflow.project_id.as_deref()),
                            &hydrated,
                            &ctx,
                        )
                        .await
                    } else {
                        Ok(None)
                    };
                    let preparation_error = hydration
                        .err()
                        .or_else(|| {
                            docs_before.as_ref().err().map(|error| format!(
                                "Document audit preflight failed; no agent launched, files preserved: {error}"
                            ))
                        })
                        .or_else(|| step_room.as_ref().err().cloned());
                    if let Some(e) = preparation_error {
                        StepOutcome {
                            result: StepResult {
                                step_name: step.name.clone(),
                                status: RunStatus::Failed,
                                output: e,
                                tokens_used: Some(0),
                                duration_ms: step_start.elapsed().as_millis() as u64,
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
                            },
                            condition_action: None,
                        }
                    } else {
                        // KT-1459 — the step's shadows start beside it on the very
                        // input it receives: the same context, the run's state
                        // before a Gate's feedback is consumed. None is awaited:
                        // the main step never waits for, or fails with, one.
                        let shadow_input =
                            shadow_snapshot
                                .as_ref()
                                .map(|_| super::shadow::ShadowInput {
                                    project_path: project_path.clone(),
                                    project_id: workflow.project_id.clone(),
                                    work_dir: work_dir.clone(),
                                    tokens: tokens_config.clone(),
                                    agents: agents_config.clone(),
                                    ollama_context_overrides: ollama_context_overrides.clone(),
                                    ctx: ctx.clone(),
                                    run_state: run.state.clone(),
                                    cancel: cancel_token.clone(),
                                });
                        if let (Some(shadow_run), Some(input)) = (&shadow_snapshot, &shadow_input) {
                            super::shadow::start_for_step(
                                &state,
                                workflow,
                                shadow_run,
                                step,
                                in_flight_result_index,
                                input,
                            )
                            .await;
                        }
                        // Gate feedback, runtime injection (global, approach B):
                        // a human Gate "request changes" persisted its comment to
                        // run.state["last_human_feedback"] before the truncate
                        // (see decide_run). Prepend it to THIS step's prompt (the
                        // re-run target) and consume it once — so every preset AND
                        // every hand-built workflow surfaces the feedback with NO
                        // `{{state.last_human_feedback}}` placeholder needed.
                        inject_and_consume_gate_feedback(
                            &mut hydrated.prompt_template,
                            &mut run.state,
                        );
                        let step = &hydrated;
                        let step_room = step_room.ok().flatten();
                        // KT-1518 — an agent whose provider just refused a turn is
                        // on cooldown: the step starts on its fallback instead of
                        // spending a launch on a provider known to refuse.
                        let mut fallback_notices: Vec<String> = Vec::new();
                        let mut current_step = step.clone();
                        if let Some((moved, notice)) =
                            super::agent_fallback_step::start_on_fallback(
                                &state,
                                agents_config,
                                step,
                            )
                            .await
                        {
                            fallback_notices.push(notice);
                            current_step = moved;
                        }
                        // Live-progress wiring — without this the user gets a
                        // "step is running" pulse with no visible content until
                        // the step finishes (typical Agent step = 30-120s of
                        // silence). Spawn a forwarder that pumps each chunk from
                        // the agent's stdout into the SSE channel as a
                        // `StepProgress` event. Both bounded channels are
                        // best-effort: once the SSE buffer is full, progress
                        // chunks are dropped instead of back-pressuring the
                        // agent or the workflow runner.
                        let (progress_tx, mut progress_rx) =
                            tokio::sync::mpsc::channel::<String>(256);
                        let forwarder_tx = events_tx.clone();
                        // KT-1252 — the step's live progress in its room: the
                        // agent's output and tool calls reach it as they come.
                        let room_progress = step_room.as_ref().map(|room| room.progress().clone());
                        if let Some(progress) = &room_progress {
                            progress.phase(AgentRunPhase::StartingCli);
                        }
                        let forwarder_progress = room_progress.clone();
                        let forwarder = tokio::spawn(async move {
                            while let Some(text) = progress_rx.recv().await {
                                if let Some(progress) = &forwarder_progress {
                                    progress.text();
                                }
                                if let Some(ref tx) = forwarder_tx {
                                    try_emit_run_event(Some(tx), RunEvent::StepProgress { text });
                                }
                            }
                        });
                        // The SSE stream reaches only the client that started the
                        // run; the in-flight row is what every other reader sees.
                        let (activity_tx, activity_rx) = tokio::sync::watch::channel(None);
                        let activity_stop = tokio_util::sync::CancellationToken::new();
                        let activity_publisher = spawn_step_activity_publisher(
                            state.db.clone(),
                            run.id.clone(),
                            Some(in_flight_result_index),
                            step.name.clone(),
                            activity_rx,
                            activity_stop.clone(),
                            room_progress,
                        );
                        let activity_stop = activity_stop.drop_guard();
                        // 0.8.2 — Stamp the wall-clock step start so the
                        // frontend's live-elapsed counter reads an authoritative
                        // value instead of estimating via `runStart + sum of
                        // prior durations`. The estimate diverges from reality
                        // after Goto loops (re-runs of the same step), agent
                        // retries, and any scheduling gap between steps —
                        // and the WorkflowDetail live-mini-dashboard then
                        // disagrees with RunDetail's `LiveStepStatus`.
                        let mut fallback_hops = 0;
                        let mut outcome = loop {
                            let full_access = agents_config.full_access_for(&current_step.agent);
                            let native_tools = crate::core::uplink::linked::<crate::core::uplink_state::WorkflowRoomTools>().map(|tools| (tools.0)(
                                state.clone(),
                                workflow.project_id.clone(),
                                run.id.clone(),
                                current_step.name.clone(),
                                step_room.as_ref().and_then(|room| room.member().cloned()),
                            ));
                            let launched = execute_step(
                                &current_step,
                                &project_path,
                                workflow.project_id.as_deref(),
                                &work_dir,
                                tokens_config,
                                full_access,
                                &ctx,
                                Some(progress_tx.clone()),
                                Some(&activity_tx),
                                Some(&agents_config.model_tiers),
                                Some(&crate::models::setup::HttpEndpoints::from_agents(
                                    agents_config,
                                )),
                                Some(&ollama_context_overrides),
                                native_tools,
                                Some(&state.db),
                                step_room.as_ref().map(|room| room.context()),
                                Some(&run.id),
                            )
                            .await;
                            // KT-1518 — the provider's refusal (never the work's
                            // failure) continues on the next link of the chain.
                            match super::agent_fallback_step::continue_after(
                                &state,
                                agents_config,
                                &step.agent,
                                &current_step,
                                &launched,
                            )
                            .await
                            {
                                super::agent_fallback_step::Next::Rerun {
                                    step: moved,
                                    notice,
                                } if fallback_hops
                                    < crate::core::agent_fallback::MAX_CHAIN_DEPTH =>
                                {
                                    fallback_hops += 1;
                                    fallback_notices.push(notice);
                                    current_step = *moved;
                                }
                                super::agent_fallback_step::Next::Stopped { notice } => {
                                    fallback_notices.push(notice);
                                    break launched;
                                }
                                _ => break launched,
                            }
                        };
                        // The forwarder ends when the last sender is gone.
                        drop(progress_tx);
                        if !fallback_notices.is_empty() {
                            outcome.result.output = super::agent_fallback_step::with_notices(
                                std::mem::take(&mut outcome.result.output),
                                &fallback_notices,
                            );
                        }
                        if let Some(room) = step_room {
                            let stop = match outcome.result.status {
                                RunStatus::Success => AgentRunStop::Finished,
                                RunStatus::Cancelled => AgentRunStop::Cancelled,
                                _ => AgentRunStop::Failed,
                            };
                            room.finish_as(stop).await;
                        }
                        // execute_step took ownership of progress_tx and dropped
                        // it on return → the forwarder's recv() now yields None
                        // and the loop exits naturally. AWAIT it (don't abort —
                        // abort would kill the task before it could drain the
                        // tail of the channel buffer, losing the last few chunks
                        // of the step's output to the SSE stream).
                        let _ = forwarder.await;
                        drop(activity_tx);
                        // Awaited so no activity write can land after the terminal result.
                        drop(activity_stop);
                        let _ = activity_publisher.await;
                        // Never restore/delete observed writes: another process
                        // may have authored them. Reject the step instead, while
                        // preserving all content and the original agent outcome.
                        if let Ok(Some(before)) = &docs_before {
                            // Scanned again: a sensitive file can appear
                            // during the run (a hook, the step itself).
                            let root = std::path::PathBuf::from(&work_dir);
                            let mut sensitive = tokio::task::spawn_blocking(move || {
                                crate::core::docs_write_filter::scan_sensitive_files(&root)
                            })
                            .await
                            .unwrap_or_default();
                            sensitive.extend_from(sensitive_substrings.as_ref());
                            let audit = crate::core::docs_write_filter::audit_docs_writes(
                                std::path::Path::new(&work_dir),
                                &sensitive,
                                before,
                            )
                            .await;
                            match audit {
                                Ok(audit) => {
                                    let rejections = &audit.rejections;
                                    if !rejections.is_empty() {
                                        let reasons = rejections
                                            .iter()
                                            .map(|(path, reason)| {
                                                format!("{path}: {}", reason.explain())
                                            })
                                            .collect::<Vec<_>>()
                                            .join("\n");
                                        fail_document_audit(&mut outcome, &reasons);
                                        let count_key =
                                            format!("docs_write_rejections.{}", step.name);
                                        run.state.insert(count_key, rejections.len().to_string());
                                    }
                                    // What could not be read fails the step, once
                                    // the rest is recorded below.
                                    if !audit.incomplete.is_empty() {
                                        fail_document_audit(
                                            &mut outcome,
                                            &audit.incomplete.join("\n"),
                                        );
                                    }
                                    // KT-1796 — never a failure, never silent.
                                    let key = format!(
                                        "{DOCS_POSSIBLE_SECRETS_STATE_PREFIX}{}",
                                        step.name
                                    );
                                    let known = run.state.get(&key).map(String::as_str);
                                    match possible_secrets_report(known, &audit) {
                                        Some(report) => {
                                            tracing::warn!(
                                                target: "kronn::docs_write_filter",
                                                run_id = %run.id,
                                                step = %step.name,
                                                "possible secrets left in changed documents, not blocked: {report}"
                                            );
                                            run.state.insert(key, report);
                                        }
                                        None => {
                                            run.state.remove(&key);
                                        }
                                    }
                                }
                                Err(error) => fail_document_audit(&mut outcome, &error),
                            }
                        }
                        // KT-1544 — a step that merges its shadows hands the
                        // rest of the workflow the merged result: the selected
                        // shadows are waited for (up to the merge's timeout),
                        // then condensed with the main output. Anything that goes
                        // wrong leaves the main output as it is.
                        if step.shadow_merge.as_ref().is_some_and(|merge| merge.enabled)
                            && (shadow_snapshot.is_none() || shadow_input.is_none())
                        {
                            tracing::warn!(
                                run_id = %run.id,
                                step = %step.name,
                                "no merge: the step started no shadow for this run"
                            );
                        }
                        if let (Some(shadow_run), Some(input)) = (&shadow_snapshot, &shadow_input) {
                            super::shadow_merge::merge_step(
                                &state,
                                workflow,
                                shadow_run,
                                step,
                                in_flight_result_index,
                                input,
                                &mut outcome,
                            )
                            .await;
                        }
                        outcome
                    }
                })
                .await,
                StepType::Gate => {
                    // 0.7.0 Phase 4 — human-in-the-loop pause. Zero tokens —
                    // the engine builds a `WaitingApproval` outcome with the
                    // rendered gate message embedded in the StepResult output.
                    // The run loop breaks below when it sees this status, and
                    // the operator's decision (POST /runs/:id/decide) calls
                    // `resume_run` to continue from the next step.
                    //
                    // Checkpoint the run's own worktree only: in shared mode the
                    // commit would sweep the operator's work (KT-1042).
                    let checkpoint_notice = if step.gate_checkpoint_before.unwrap_or(false) {
                        let notice = super::gate_checkpoint::checkpoint_gate(
                            run.workspace_path.as_deref(),
                            &run.id,
                            &mut run.state,
                            &step.name,
                        );
                        match &notice {
                            None => tracing::info!(
                                run_id = %run.id,
                                step = %step.name,
                                "gate_checkpoint_before committed in the run worktree",
                            ),
                            Some(reason) => tracing::warn!(
                                run_id = %run.id,
                                step = %step.name,
                                reason = %reason,
                                "gate_checkpoint_before not taken",
                            ),
                        }
                        notice
                    } else {
                        None
                    };
                    let mut gate = super::gate_step::execute_gate_step(step, &ctx);
                    if let Some(notice) = checkpoint_notice {
                        gate.result.output = format!("{}\n\n> {notice}", gate.result.output);
                    }
                    gate
                }
                StepType::Exec => {
                    // 0.7.0 Phase 5 — direct shell execution. Zero tokens.
                    // Allowlist-gated, never-shell, args-as-literal-argv.
                    // The run-time guard mirrors the save-time validator
                    // for defence in depth (a workflow loaded from a
                    // hand-edited JSON could carry a stale Exec step).
                    super::exec_step::execute_exec_step_for_project(
                        step,
                        &workflow.exec_allowlist,
                        &work_dir,
                        &ctx,
                        workflow.project_id.as_deref(),
                        carrying_repository.as_deref(),
                        super::exec_step::output_limit_for(
                            step,
                            &workflow.steps,
                            &workflow.on_failure,
                        ),
                    )
                    .await
                }
                StepType::BatchApiCall => {
                    // 0.6.0 — mechanical fan-out of an API call over a list of
                    // items. Zero tokens, parallel HTTP, idempotency-by-prompt
                    // moved to idempotency-by-construction. See
                    // batch_apicall_step.rs for the executor.
                    let api_caller =
                        crate::core::api_access::workflow_caller(&state, workflow).await;
                    super::batch_apicall_step::execute_batch_apicall_step(
                        step,
                        workflow.project_id.as_deref(),
                        &state,
                        &ctx,
                        super::api_call_executor::ApiCallLogContext::workflow_step(run.id.clone()),
                        &api_caller,
                    )
                    .await
                }
                StepType::JsonData => {
                    // 0.7+ — déterministe data source. Émet le payload
                    // littéral du step dans une envelope Structured. Zéro
                    // token, zéro réseau. Cf. json_data_step.rs.
                    super::json_data_step::execute_json_data_step(step).await
                }
                StepType::CollectApiData => {
                    let api_caller =
                        crate::core::api_access::workflow_caller(&state, workflow).await;
                    super::collect_api_data_step::execute_collect_api_data_step(
                        step,
                        workflow.project_id.as_deref(),
                        &state,
                        &ctx,
                        super::api_call_executor::ApiCallLogContext::workflow_step(run.id.clone()),
                        &workflow.exec_allowlist,
                        &work_dir,
                        &api_caller,
                    )
                    .await
                }
                StepType::TransformData => {
                    super::transform_data_step::execute_transform_data_step(step, &ctx).await
                }
                StepType::PublishPageData => {
                    super::publish_page_step::execute_publish_page_data_step(
                        step,
                        &workflow.id,
                        &run.id,
                        run.project_id.as_deref().or(workflow.project_id.as_deref()),
                        &state,
                        &ctx,
                    )
                    .await
                }
                StepType::Decide => {
                    let base = crate::api::ollama_health::resolve_base_url_pub(
                        state.ollama_base_url_override.as_deref(),
                    );
                    super::decide_step::execute_decide_step(
                        step,
                        &ctx,
                        &state,
                        &base,
                        agents_config.model_tiers.ollama.decision.as_deref(),
                    )
                    .await
                }
                StepType::TaskBoard => {
                    super::task_board_step::execute_task_board_step(
                        step,
                        &workflow.id,
                        &state,
                        &ctx,
                    )
                    .await
                }
                StepType::SubWorkflow => {
                    // Phase 1b-i — re-entrant: runs the target workflow as a
                    // child run (Box::pin recursion inside the executor),
                    // bounded by depth. Cf. sub_workflow_step.rs.
                    super::sub_workflow_step::execute_sub_workflow_step(
                        &state,
                        &sub_parent_run_id,
                        sub_current_depth,
                        step,
                        tokens_config,
                        agents_config,
                        sub_budget.clone(),
                        sub_parent_workspace.clone(),
                        super::sub_workflow_step::ChildLaunch {
                            ctx: &ctx,
                            parent_variables: &workflow.variables,
                            parent_project_id: workflow.project_id.as_deref(),
                            cancel: Some(&cancel_token),
                        },
                    )
                    .await
                }
                StepType::TriggerWorkflow => {
                    super::trigger_workflow_step::execute_trigger_workflow_step(
                        &state, workflow, &run.id, step, &ctx,
                    )
                    .await
                }
                StepType::DelegateSubtasks => {
                    // Boxed: inlined, its state machine overflows the run's stack.
                    Box::pin(
                        super::delegate_subtasks_step::execute_delegate_subtasks_step(
                            &state,
                            step,
                            &run.id,
                            run.project_id.as_deref().or(workflow.project_id.as_deref()),
                            &project_path,
                            &work_dir,
                            &ctx,
                            tokens_config,
                            agents_config,
                            &ollama_context_overrides,
                            budget.clone(),
                        ),
                    )
                    .await
                }
            }
        };

        let checkpoint_db = db.clone();
        let checkpoint_run_id = run.id.clone();
        let mut outcome: StepOutcome = match await_in_flight_step(
            step_future,
            &cancel_token,
            timeout_deadline,
            || timeout_clock.checkpoint(&checkpoint_db, &checkpoint_run_id),
        )
        .await
        {
            InFlightStepOutcome::Completed(outcome) => *outcome,
            InFlightStepOutcome::Cancelled => {
                tracing::info!(
                    "Workflow run {} cancelled mid-step '{}' — dropping in-flight executor",
                    run.id,
                    step.name
                );
                if matches!(step.step_type, StepType::BatchQuickPrompt) {
                    if let Err(error) = super::cancellation::cancel_run_tree(
                        &state,
                        &run.id,
                        super::cancellation::CancellationScope::DescendantsOnly,
                        "cancelled_by_parent_workflow",
                    )
                    .await
                    {
                        tracing::error!(run_id = %run.id, "Unable to cancel batch descendants: {error}");
                    }
                }
                cancelled_by_user = true;
                StepOutcome {
                    result: StepResult {
                        step_name: step.name.clone(),
                        status: RunStatus::Cancelled,
                        output: format!("Step '{}' cancelled by user mid-flight.", step.name),
                        tokens_used: interrupted_step_tokens(&step.step_type),
                        duration_ms: step_start.elapsed().as_millis() as u64,
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
                    },
                    condition_action: None,
                }
            }
            InFlightStepOutcome::TimedOut => {
                let elapsed_secs = timeout_clock.elapsed().as_secs();
                let actual_secs = elapsed_secs.max(resolved_guards.timeout_seconds);
                tracing::warn!(
                    target: "kronn::workflow_guard",
                    run_id = %run.id,
                    step = %step.name,
                    kind = "Timeout",
                    threshold_secs = resolved_guards.timeout_seconds,
                    actual_secs,
                    "Workflow run stopped by Timeout guard during an in-flight step"
                );
                emit(RunEvent::GuardTriggered {
                    kind: GuardKind::Timeout,
                    threshold: resolved_guards.timeout_seconds,
                    actual: actual_secs,
                });
                stopped_by_guard = true;
                StepOutcome {
                    result: StepResult {
                        step_name: step.name.clone(),
                        status: RunStatus::StoppedByGuard,
                        output: format!(
                            "Step '{}' stopped by workflow Timeout guard after {}s (limit {}s).",
                            step.name, actual_secs, resolved_guards.timeout_seconds
                        ),
                        tokens_used: interrupted_step_tokens(&step.step_type),
                        duration_ms: step_start.elapsed().as_millis() as u64,
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
                    },
                    condition_action: None,
                }
            }
        };

        if outcome.result.started_at.is_none() {
            outcome.result.started_at = Some(step_started_at);
        }
        apply_terminal_stop(&mut outcome);

        // Snapshot "what actually ran" onto the result row FIRST, then seed
        // the template ctx from it — set_step_meta reads step_agent/step_model,
        // so the order is load-bearing (Codex review: seeding before the
        // snapshot exposed None to the next step).
        record_step_completion(
            step,
            &mut outcome.result,
            &mut ctx,
            Some(&agents_config.model_tiers),
        );
        // KT-1694 — a later step reads what the step's shadows and merge did:
        // the merge ran inside the step's future, so the summary is final.
        if shadow_snapshot.is_some() {
            let summary = super::shadow_summary::for_completed_step(
                &state,
                &run.id,
                in_flight_result_index,
                step,
                &outcome.result,
            )
            .await;
            ctx.set_step_shadows(&step.name, &summary.to_string());
        }

        // 0.7.0 Phase 3 — persist declared artifacts to disk so they
        // survive past the run (committable when in a worktree,
        // inspectable in the run-detail UI, reusable across reboots if
        // the workspace is reused). Undeclared artifacts in the agent's
        // output are kept in `ctx` (template visibility) but NOT
        // persisted — declaring is the contract that says "this matters
        // enough to write a file for".
        let (extracted, extracted_state) =
            super::template::extract_step_markers(&outcome.result.output);
        if !extracted.is_empty() {
            persist_declared_artifacts(workflow, &extracted, std::path::Path::new(&work_dir));
        }

        // 0.7.0 Phase 6 — durable state. `set_step_output` already
        // pushed `state.<k>` into the template ctx for the immediate
        // next step; here we mirror those entries onto `run.state`
        // so they're persisted to the DB on the upcoming progress
        // snapshot and survive Gate pauses / daemon restarts.
        for (k, v) in extracted_state {
            run.state.insert(k, v);
        }

        // After the step's declared artifacts are written: they are the run's changes too.
        if let Some(baseline) = safety_baseline.as_ref().filter(|_| {
            matches!(
                outcome.result.status,
                RunStatus::Success | RunStatus::Partial | RunStatus::Failed
            )
        }) {
            if let Some(reason) = safety_breach(workflow, &work_dir, baseline).await {
                fail_safety_limit(&mut outcome, &reason);
            }
        }
        apply_terminal_stop(&mut outcome);

        // Accumulate tokens
        run.tokens_used += outcome.result.tokens_used.unwrap_or(0);

        // KT-811 — a quota refusal waits for the reset instead of failing the
        // run. A child waits only when its parent will resume it (KT-1556).
        if outcome.result.status == RunStatus::Failed && super::quota_wait::can_wait(run) {
            if let Some(classified) = outcome.result.quota_wait.take() {
                let wait = if step.step_type == StepType::SubWorkflow {
                    super::quota_wait::remember_mirrored_child(
                        &mut run.state,
                        &step.name,
                        outcome.result.child_run_id.as_deref(),
                        classified.id.as_deref(),
                    );
                    super::quota_wait::mirror_child_wait(classified)
                } else {
                    super::quota_wait::plan_wait(&mut run.state, &step.name, classified, Utc::now())
                };
                tracing::info!(
                    run_id = %run.id,
                    step = %step.name,
                    wake_at = ?wait.wake_at,
                    parked = ?wait.parked,
                    "provider quota refusal — the step waits instead of failing"
                );
                outcome.result.quota_wait = Some(wait);
                outcome.result.status = RunStatus::WaitingQuota;
            }
        } else if outcome.result.status != RunStatus::Failed {
            run.state
                .remove(super::quota_wait::QUOTA_ATTEMPTS_STATE_KEY);
        }
        let waiting_quota_here = outcome.result.status == RunStatus::WaitingQuota;
        if step.step_type == StepType::SubWorkflow && !waiting_quota_here {
            super::quota_wait::forget_mirrored_child(&mut run.state, &step.name);
        }
        // 0.7.0 — count this step toward the LLM-calls quota. Only step
        // types that spawn an agent are counted: BatchQuickPrompt counts
        // as the number of children actually spawned (read from the
        // outcome's batch metadata when present, else 1 — conservative).
        // ApiCall and Notify cost zero by design. A call refused for quota spent
        // nothing and is replayed, so it does not count (KT-1046).
        let llm_calls_before = budget.llm_calls();
        match step.step_type {
            StepType::Agent | StepType::BatchQuickPrompt if waiting_quota_here => {}
            StepType::Agent => {
                budget.add_llm_calls(1);
            }
            StepType::BatchQuickPrompt => {
                // Conservative count: each batch step counts as 1 LLM call
                // toward the quota (rather than N children). Senior Dev's
                // recommendation was N post-fan-out, but `StepResult`
                // doesn't carry the spawned-children count today, and
                // wiring it through would touch every batch executor —
                // out of scope for the Phase-1 guards. The fan-out cap
                // (`batch_max_items`) already limits the per-step blast
                // radius. Tracked separately as future enhancement.
                budget.add_llm_calls(1);
            }
            StepType::ApiCall
            | StepType::Notify
            | StepType::Gate
            | StepType::Exec
            | StepType::BatchApiCall
            | StepType::JsonData
            | StepType::CollectApiData
            | StepType::TransformData
            | StepType::PublishPageData
            | StepType::TaskBoard
            // A decision model, not an agent: no agent call to count.
            | StepType::Decide
            // A triggered run has its own budget.
            | StepType::TriggerWorkflow
            // Counts each review call itself, as it happens.
            | StepType::DelegateSubtasks => {}
            // SubWorkflow itself spawns no LLM directly; its child run's
            // Agent steps consume LLM calls. Phase 1b aggregates the child's
            // count into the SHARED budget so the parent quota isn't bypassed
            // (spec §4.2). Stub today → zero direct cost.
            StepType::SubWorkflow => {}
        }

        guard_counters.llm_calls = budget.llm_calls();
        guard_counters.store(&mut run.state);
        // Also covers calls a sub-workflow child spent on the shared budget.
        if guard_counters.llm_calls != llm_calls_before || budget.has_unrecorded_calls() {
            if let Err(error) = record_tree_llm_calls(&state, &budget).await {
                // Not durably counted, so not durably done: a resume re-runs it.
                // Counted after the quota verdict, hence its own application.
                outcome.result.terminal_stop =
                    Some(format!("The LLM-call count could not be recorded: {error}"));
                apply_terminal_stop(&mut outcome);
            }
        }

        let step_failed = outcome.result.status == RunStatus::Failed;
        // Captured BEFORE `outcome.result` is moved into `step_results`
        // below — used by the Stop arm to give the run an honest verdict
        // when an agent step exits 0 but declared `[SIGNAL: ERROR]` (audit
        // P1). Exact trailing-line match, not substring (a body excerpt
        // quoting the word ERROR must not count).
        let step_declared_error = outcome
            .result
            .output
            .lines()
            .rev()
            .take(5)
            .any(|l| l.trim().ends_with("[SIGNAL: ERROR]"));
        // 0.7.0 Phase 4 — Gate produced WaitingApproval. Break out of
        // the loop AFTER recording the StepResult so the operator sees
        // the rendered message on the run-detail page. The run is
        // resumed via `resume_run` once a decision arrives.
        let paused_here = outcome.result.status == RunStatus::WaitingApproval;

        // Emit step done event
        emit(RunEvent::StepDone {
            step_result: Box::new(outcome.result.clone()),
        });
        // 0.8.2 — cross-tab live update. status reflects the new state
        // (WaitingApproval if the step was a Gate, else still Running).
        // The current_step is cleared since this step is now finished.
        let post_step_status = if matches!(
            outcome.result.status,
            RunStatus::WaitingApproval | RunStatus::WaitingQuota
        ) {
            outcome.result.status.clone()
        } else {
            run.status.clone()
        };
        broadcast_run_state(&post_step_status, step_idx as i32, None);

        // 0.8.3 — Feasibility-Gated triage ingest. When the step that
        // just finished is a triage step (description marker OR schema
        // shape match) AND it succeeded with a valid envelope, parse
        // the manifest's decided/mocked/blocked lists and upsert one
        // row per entry into `agent_decisions`. Idempotent on
        // (run_id, decision_id) — a Goto retriage just rewrites the
        // same rows. Failures here log but never abort the run; the
        // manifest itself is the source of truth in StepResult.output.
        if outcome.result.status == RunStatus::Success
            && crate::workflows::triage::is_triage_step(
                step.description.as_deref(),
                &step.output_format,
            )
        {
            if let Some(env) =
                crate::workflows::template::extract_step_envelope(&outcome.result.output)
            {
                if let Ok(mut manifest) = serde_json::from_str::<serde_json::Value>(&env.data_json)
                {
                    // 2026-06-12 — incremental re-triage: a round ≥ 2 may emit
                    // only the items the plan review flagged, plus
                    // `unchanged: ["<id>", …]`. Hydrate those ids from the
                    // previous round's engine-written `.kronn/manifest.json`,
                    // then rewrite the step output so EVERY downstream reader
                    // ({{steps.triage.data}}, the Gate, pr_draft) sees the
                    // FULL manifest. Decisions ingest + machine-file derive
                    // below run on the merged manifest.
                    let has_unchanged = manifest
                        .get("unchanged")
                        .and_then(|u| u.as_array())
                        .map(|a| !a.is_empty())
                        .unwrap_or(false);
                    if has_unchanged && !work_dir.is_empty() {
                        let prev_path =
                            std::path::Path::new(&work_dir).join(".kronn/manifest.json");
                        let prev = std::fs::read_to_string(&prev_path)
                            .ok()
                            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
                        match prev {
                            Some(prev) => {
                                let (merged, n) = crate::workflows::triage::merge_unchanged_items(&manifest, &prev);
                                manifest = merged;
                                outcome.result.output = crate::workflows::triage::rewrite_envelope_data(
                                    &outcome.result.output,
                                    &manifest,
                                );
                                tracing::info!(
                                    "Triage incremental merge: {} unchanged item(s) hydrated from previous manifest",
                                    n
                                );
                            }
                            None => tracing::warn!(
                                "Triage emitted `unchanged` ids but no previous .kronn/manifest.json — manifest stays partial"
                            ),
                        }
                    }
                    let ticket_ref = run.trigger_context.as_ref().and_then(|tc| {
                        tc.get("issue")
                            .and_then(|i| i.get("key").or_else(|| i.get("number")))
                            .and_then(|v| {
                                v.as_str()
                                    .map(String::from)
                                    .or_else(|| v.as_i64().map(|n| n.to_string()))
                            })
                    });
                    let project_id = workflow.project_id.clone();
                    let decisions = crate::workflows::triage::manifest_to_decisions(
                        &manifest,
                        &run.id,
                        &workflow.id,
                        &step.name,
                        project_id.as_deref(),
                        ticket_ref.as_deref(),
                    );
                    let dec_count = decisions.len();
                    let db_ingest = db.clone();
                    let ingest_result = db_ingest
                        .with_conn(move |conn| {
                            for d in &decisions {
                                crate::db::agent_decisions::upsert(conn, d)?;
                            }
                            Ok::<_, anyhow::Error>(())
                        })
                        .await;
                    match ingest_result {
                        Ok(()) => tracing::info!(
                            "Triage ingest: {} decision row(s) persisted for run {}",
                            dec_count, run.id
                        ),
                        Err(e) => tracing::warn!(
                            "Triage ingest failed for run {}: {} — manifest stays in StepResult.output",
                            run.id, e
                        ),
                    }

                    // 2026-06-12 (critical fix #1) — derive the machine files
                    // (.kronn/tasks.json + decision_ids.txt + files_touched.txt)
                    // from the VALIDATED envelope instead of trusting the agent
                    // to hand-write consistent copies. The human gates the
                    // envelope; the fan-out + deterministic checks execute
                    // EXACTLY that data. Overwrites whatever the agent may
                    // have written (engine wins). Best-effort: failures log,
                    // never abort the run.
                    if !work_dir.is_empty() {
                        for (rel, content) in
                            crate::workflows::triage::derive_machine_files(&manifest)
                        {
                            let path = std::path::Path::new(&work_dir).join(&rel);
                            if let Some(parent) = path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            match std::fs::write(&path, &content) {
                                Ok(()) => tracing::info!(
                                    "Triage machine file derived: {} ({} bytes)",
                                    rel,
                                    content.len()
                                ),
                                Err(e) => tracing::warn!(
                                    "Failed to write derived triage file {}: {}",
                                    rel,
                                    e
                                ),
                            }
                        }
                    }
                } else {
                    tracing::warn!(
                        "Triage step '{}' produced an envelope but data_json is not JSON; skipping ingest",
                        step.name
                    );
                }
            }
        }

        // Clear the intent in the SAME durable snapshot that records the
        // StepResult. A crash before this write leaves the marker intact; a
        // successful write proves the outcome and cursor together.
        if side_effect_intent_recorded {
            run.state.remove(UNCERTAIN_SIDE_EFFECT_STATE_KEY);
        }
        // Replace the durable Running placeholder atomically with the final
        // result. Keeping one row per execution avoids duplicate timeline
        // entries while still exposing the live step to MCP readers.
        if let Some(slot) = run.step_results.get_mut(in_flight_result_index) {
            *slot = outcome.result;
        } else {
            return Err(anyhow::anyhow!(
                "Run {} lost its StepStart placeholder for '{}'",
                run.id,
                step.name
            ));
        }

        // KT-1796 — whatever the child runs of this step reported is said
        // on this run too.
        if step.step_type == StepType::SubWorkflow {
            adopt_child_possible_secrets(&state, &run.id, &step.name, &mut run.state).await;
        }

        // Persist progress
        timeout_clock.store(run);
        let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
        let db4 = db.clone();
        db4.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
            .await?;

        // Mid-step cancellation winner — `cancelled_by_user` was flipped
        // by the `tokio::select!` cancel branch above. We persist the
        // `Cancelled` step row first (so the UI sees what was killed),
        // then break out of the run loop here. Skipping the rollback
        // chain is intentional: a user-initiated stop shouldn't trigger
        // rollback semantics meant for failed-then-recover paths.
        if cancelled_by_user {
            all_success = false;
            break;
        }
        if stopped_by_guard {
            all_success = false;
            break;
        }
        if waiting_quota_here {
            paused_for_quota = true;
            break;
        }

        if step_failed {
            all_success = false;
            // 0.7.0+ — let `on_result` Goto/Skip override the short-circuit.
            // Without this, a `cargo test` exit≠0 would always tip into the
            // rollback chain even when the user explicitly declared a
            // recovery rule (`contains "ERROR"` → Goto implement). Stop is
            // the safe synonym for current behaviour, no-match keeps the
            // legacy break + rollback path. The condition_action was already
            // computed by the executor (Exec, ApiCall) — runner just honours
            // it. Agent steps don't reach here because they never set Failed
            // status when emitting a SIGNAL: Failed for them = a real crash.
            match outcome.condition_action {
                Some(ConditionAction::Goto {
                    ref step_name,
                    max_iterations,
                }) => {
                    if let Some(target) = workflow.steps.iter().position(|s| s.name == *step_name) {
                        let edge = (step.name.clone(), step_name.clone());
                        let count = guard_counters.goto_fires.entry(edge.clone()).or_insert(0);
                        if let Some(cap) = max_iterations {
                            if *count >= cap {
                                tracing::info!(
                                    "Step '{}' (Failed) Goto '{}' reached max_iterations ({}) — falling through to rollback",
                                    step.name, step_name, cap
                                );
                                break;
                            }
                        }
                        *count += 1;
                        tracing::info!(
                            "Step '{}' (Failed) honoured on_result Goto '{}' (fire #{}) — skipping rollback",
                            step.name, step_name, *count
                        );
                        // Failed status stays in the StepResult (already emitted
                        // above), but we treat it as recoverable: jump to the
                        // target and clear the abort flag so the run keeps going.
                        all_success = true;
                        step_idx = target;
                        continue;
                    } else {
                        tracing::warn!(
                            "Step '{}' (Failed) Goto target '{}' not found — falling through to rollback",
                            step.name, step_name
                        );
                    }
                }
                Some(ConditionAction::Skip) => {
                    tracing::info!(
                        "Step '{}' (Failed) honoured on_result Skip — skipping next step, no rollback",
                        step.name
                    );
                    all_success = true;
                    step_idx += 2;
                    continue;
                }
                Some(ConditionAction::Stop) | None => {
                    // Stop is identical to no-match here: end the linear run.
                    // Whether on_failure rollback fires depends on `all_success`,
                    // which stays false → rollback chain runs. Same as before.
                }
            }
            break;
        }
        if paused_here {
            paused_for_approval = true;
            // 0.7.0 P1-1 — fire optional webhook to ping ops when the
            // run enters WaitingApproval. Best-effort: spawned in the
            // background, errors logged only, never blocks the run.
            // The URL is templated so `{{state.slack_url}}` etc work.
            if let Some(raw_url) = step.gate_notify_url.as_deref() {
                if let Ok(rendered_url) = ctx.render_strict(raw_url) {
                    if !rendered_url.trim().is_empty() {
                        let payload = serde_json::json!({
                            "run_id": run.id,
                            "workflow_id": workflow.id,
                            "workflow_name": workflow.name,
                            "step_name": step.name,
                            "message": run.step_results
                                .last()
                                .map(|sr| sr.output.clone())
                                .unwrap_or_default(),
                        });
                        let url_clone = rendered_url.clone();
                        let run_id_for_log = run.id.clone();
                        tokio::spawn(async move {
                            match fire_gate_webhook(
                                &url_clone,
                                &payload,
                                crate::core::safe_http::SafeHttpPolicy::Public,
                            )
                            .await
                            {
                                Ok(status) => tracing::info!(
                                    target: "kronn::gate_webhook",
                                    run_id = %run_id_for_log, status,
                                    "Gate webhook fired"
                                ),
                                Err(e) => tracing::warn!(
                                    target: "kronn::gate_webhook",
                                    run_id = %run_id_for_log,
                                    "Gate webhook failed: {}", e
                                ),
                            }
                        });
                    }
                }
            }
            // 0.8.6 (#26) — opt-in auto-approve timer. If the gate
            // step has `gate_auto_approve_after_secs: Some(n)`,
            // spawn a background task that POSTs an Approve decision
            // to our own /decide endpoint after `n` seconds. Failures
            // are logged but don't block the run — the user can
            // still manually decide. Race-safe : the /decide
            // handler refuses to decide a run that's no longer in
            // WaitingApproval, so a human Approve before the timer
            // fires simply wins. Cancellation across backend
            // restart is out of scope for the MVP (the timer
            // resets ; user re-decides).
            if let Some(delay_secs) = step.gate_auto_approve_after_secs {
                if delay_secs > 0 {
                    let run_id_for_timer = run.id.clone();
                    let workflow_id_for_timer = workflow.id.clone();
                    let gate_name_for_timer = step.name.clone();
                    let port = crate::core::child_env::var("KRONN_BACKEND_PORT")
                        .ok()
                        .and_then(|s| s.parse::<u16>().ok())
                        .unwrap_or(3140);
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(delay_secs as u64)).await;
                        let client = match reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(15))
                            .build()
                        {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(
                                    target: "kronn::gate_auto_approve",
                                    run_id = %run_id_for_timer,
                                    "auto-approve client build failed: {}", e,
                                );
                                return;
                            }
                        };
                        // 2026-06-10 audit P0 — this self-POST was DOUBLY dead:
                        // the URL omitted the `{workflow_id}` segment (no such
                        // route → 404) AND the decision was sent as "Approve"
                        // while the handler matches lowercase "approve". Both
                        // failures were logged as "auto-approve POST sent"
                        // (HTTP status recorded but never checked) — every run
                        // with auto-approve stayed WaitingApproval forever.
                        // `gate_step` pins the decision to THIS gate so a timer
                        // armed on gate A can never approve a later gate B.
                        let url = format!(
                            "http://127.0.0.1:{}/api/workflows/{}/runs/{}/decide",
                            port, workflow_id_for_timer, run_id_for_timer,
                        );
                        let body = serde_json::json!({
                            "decision": "approve",
                            "comment": format!("[auto-approved after {delay_secs}s — no human action]"),
                            "gate_step": gate_name_for_timer,
                        });
                        match client.post(&url).json(&body).send().await {
                            Ok(resp) => tracing::info!(
                                target: "kronn::gate_auto_approve",
                                run_id = %run_id_for_timer, status = resp.status().as_u16(),
                                "auto-approve POST sent",
                            ),
                            Err(e) => tracing::warn!(
                                target: "kronn::gate_auto_approve",
                                run_id = %run_id_for_timer,
                                "auto-approve POST failed: {}", e,
                            ),
                        }
                    });
                }
            }
            break;
        }

        // Delay after step (if configured)
        if let Some(delay_secs) = step.delay_after_secs {
            if delay_secs > 0 {
                tracing::info!(
                    "Step '{}' — waiting {}s before next step",
                    step.name,
                    delay_secs
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(delay_secs)).await;
            }
        }

        // Handle condition actions
        match outcome.condition_action {
            Some(ConditionAction::Stop) => {
                // 2026-06-10 audit P1 — honest run verdict on agent-declared
                // errors. Agent steps exit 0 → Success even when they emit
                // `[SIGNAL: ERROR]`; pre-fix an `ERROR → Stop` rule broke the
                // loop with `all_success` still true → the FINAL run read
                // **Success** while the agent had declared a failure (the
                // create-pr preset relies on exactly this signal). We check
                // the trailing signal LINES exactly (not substring — body
                // excerpts quoting the word ERROR don't count): Stop + a
                // declared ERROR signal = the run did not succeed. A Stop on
                // a benign marker (e.g. "DONE_ALL") keeps Success semantics.
                if step_declared_error {
                    all_success = false;
                    tracing::info!(
                        "Step '{}' triggered Stop with a declared [SIGNAL: ERROR] — run marked Failed",
                        step.name
                    );
                } else {
                    tracing::info!("Step '{}' triggered Stop condition", step.name);
                }
                break;
            }
            Some(ConditionAction::Skip) => {
                tracing::info!("Step '{}' triggered Skip — skipping next step", step.name);
                step_idx += 2; // skip next step
                continue;
            }
            Some(ConditionAction::Goto {
                ref step_name,
                max_iterations,
            }) => {
                if let Some(target) = workflow.steps.iter().position(|s| s.name == *step_name) {
                    // 0.7.0 Phase 6 — per-edge cap. Counter keyed by
                    // (source, target). When `max_iterations = Some(N)`,
                    // the Goto fires at most N times before the runner
                    // falls through. `None` keeps legacy behaviour
                    // (capped only by the workflow-level loop_detection
                    // guard).
                    let edge = (step.name.clone(), step_name.clone());
                    let count = guard_counters.goto_fires.entry(edge.clone()).or_insert(0);
                    if let Some(cap) = max_iterations {
                        if *count >= cap {
                            tracing::info!(
                                "Step '{}' Goto '{}' reached max_iterations ({}) — falling through",
                                step.name,
                                step_name,
                                cap
                            );
                            // No `continue` — fall through to step_idx += 1.
                            step_idx += 1;
                            continue;
                        }
                    }
                    *count += 1;
                    tracing::info!(
                        "Step '{}' triggered Goto '{}' (index {}, fire #{})",
                        step.name,
                        step_name,
                        target,
                        *count
                    );
                    step_idx = target;
                    continue;
                } else {
                    tracing::warn!("Goto target '{}' not found, continuing normally", step_name);
                }
            }
            None => {}
        }

        step_idx += 1;
    }

    // Run after_run hook (skip when paused — the run isn't done yet).
    let paused = paused_for_approval || paused_for_quota;
    if !paused {
        if let Some(ref ws) = workspace {
            if let Err(e) = ws.after_run().await {
                tracing::warn!(
                    run_id = %run.id,
                    error = %e,
                    "after_run hook could not be executed"
                );
            }
        }
    }
    // The last writes (final artifacts, after_run) are measured before any verdict.
    if !paused && !cancelled_by_user && super::safety::run_terminal_stop(run).is_none() {
        if let Some(baseline) = safety_baseline.as_ref() {
            if let Some(reason) = safety_breach(workflow, &work_dir, baseline).await {
                tracing::warn!(target: "kronn::workflow_safety", "{reason}");
                let mut stop = super::safety::refusal_result(&reason);
                stop.terminal_stop = Some(reason);
                emit(RunEvent::StepDone {
                    step_result: Box::new(stop.clone()),
                });
                run.step_results.push(stop);
                all_success = false;
            }
        }
    }

    // Post-step actions (CreatePr, CommentIssue, etc.) are handled by MCP tools
    // injected into agent prompts — no separate actions phase needed.

    // Final status priority: user-cancellation > pause > guard-stop > failure > success.
    // Each one is a deliberate stop with its own UI treatment, so we
    // preserve them as distinct terminal states. WaitingApproval is NOT
    // terminal — `resume_run` flips it back to Running once a decision
    // arrives.
    run.status = if cancelled_by_user {
        RunStatus::Cancelled
    } else if paused_for_approval {
        RunStatus::WaitingApproval
    } else if paused_for_quota {
        RunStatus::WaitingQuota
    } else if stopped_by_guard {
        RunStatus::StoppedByGuard
    } else if all_success {
        RunStatus::Success
    } else {
        RunStatus::Failed
    };

    // 0.7.0 Phase 7 — rollback / compensation. Fires only on pure
    // `Failed` (not Cancelled, not StoppedByGuard, not WaitingApproval).
    // Compensation steps see the regular template context PLUS
    // `{{failed_step.*}}` so they can react to what specifically broke.
    // If a rollback step itself fails, subsequent rollback steps are
    // skipped — the run stays `Failed` regardless of rollback outcome.
    let terminal_stop = super::safety::run_terminal_stop(run).is_some();
    if run.status == RunStatus::Failed && !terminal_stop && !workflow.on_failure.is_empty() {
        let failed = run
            .step_results
            .iter()
            .rev()
            .find(|r| r.status == RunStatus::Failed)
            .cloned();
        let (failed_name, failed_output) = match failed {
            Some(r) => (r.step_name, r.output),
            None => ("(unknown)".to_string(), String::new()),
        };
        ctx.set("failed_step.name", failed_name.clone());
        ctx.set("failed_step.output", failed_output);

        tracing::info!(
            target: "kronn::workflow_rollback",
            run_id = %run.id, failed_step = %failed_name, rollback_count = workflow.on_failure.len(),
            "Workflow run failed — running rollback chain"
        );

        for rb_step in &workflow.on_failure {
            // The ⏹ cancel was only raced against the PRIMARY step loop —
            // a cancel during a long rollback chain was acknowledged in the
            // UI while the remaining compensation agents kept executing (and
            // the terminal write then flipped the row back to Failed; now
            // blocked anyway by the Cancelled-stickiness guard).
            if cancel_token.is_cancelled() {
                tracing::info!(
                    target: "kronn::workflow_rollback",
                    run_id = %run.id,
                    "Cancel requested — abandoning remaining rollback steps"
                );
                run.status = RunStatus::Cancelled;
                break;
            }
            emit(RunEvent::StepStart {
                step_name: rb_step.name.clone(),
                step_index: run.step_results.len(),
                total_steps: run.step_results.len() + 1,
            });

            let mut rb_outcome: StepOutcome = match rb_step.step_type {
                StepType::BatchQuickPrompt => {
                    super::batch_step::execute_batch_quick_prompt_step(
                        rb_step,
                        &run.id,
                        state.clone(),
                        &ctx,
                    )
                    .await
                }
                StepType::Notify => super::notify_step::execute_notify_step(rb_step, &ctx).await,
                StepType::ApiCall => {
                    // 0.8.6 (#59) — same audit stamping as the primary
                    // step dispatch above. This is the rollback / branch
                    // step replay path.
                    let api_caller =
                        crate::core::api_access::workflow_caller(&state, workflow).await;
                    super::api_call_executor::execute_api_call_step_with_db_as(
                        rb_step,
                        workflow.project_id.as_deref(),
                        &state,
                        &ctx,
                        super::api_call_executor::SecurityPolicy::production(),
                        super::api_call_executor::ApiCallLogContext::workflow_step(run.id.clone()),
                        &api_caller,
                    )
                    .await
                }
                StepType::Agent => {
                    let room_started = std::time::Instant::now();
                    // KT-1096 — a rollback Agent step loads its pinned Quick
                    // Prompt, as the main path does.
                    let mut hydrated = rb_step.clone();
                    let hydration = super::quick_prompt_hydrate::hydrate_step_from_quick_prompt(
                        &mut hydrated,
                        &state.db,
                        Some(&run.id),
                    )
                    .await;
                    let rb_step = &hydrated;
                    if let Err(error) = hydration {
                        super::step_room::refused_outcome(
                            rb_step,
                            error,
                            room_started.elapsed().as_millis() as u64,
                        )
                    } else {
                        match super::step_room::activate(
                            &state.workflow_step_rooms,
                            &state.db,
                            &run.id,
                            run.project_id.as_deref().or(workflow.project_id.as_deref()),
                            rb_step,
                            &ctx,
                        )
                        .await
                        {
                            Err(error) => super::step_room::refused_outcome(
                                rb_step,
                                error,
                                room_started.elapsed().as_millis() as u64,
                            ),
                            Ok(step_room) => {
                                let full_access = agents_config.full_access_for(&rb_step.agent);
                                let native_tools = crate::core::uplink::linked::<
                                    crate::core::uplink_state::WorkflowRoomTools,
                                >()
                                .map(|tools| {
                                    (tools.0)(
                                        state.clone(),
                                        workflow.project_id.clone(),
                                        run.id.clone(),
                                        rb_step.name.clone(),
                                        step_room.as_ref().and_then(|room| room.member().cloned()),
                                    )
                                });
                                // KT-1252 — the room's bubble follows a rollback
                                // step as it does a step of the main path. No
                                // result is in flight for it, so the publisher
                                // only feeds the room's progress.
                                let room_progress =
                                    step_room.as_ref().map(|room| room.progress().clone());
                                if let Some(progress) = &room_progress {
                                    progress.phase(AgentRunPhase::StartingCli);
                                }
                                let (activity_tx, activity_rx) = tokio::sync::watch::channel(None);
                                let activity_stop = tokio_util::sync::CancellationToken::new();
                                let activity_publisher = spawn_step_activity_publisher(
                                    state.db.clone(),
                                    run.id.clone(),
                                    None,
                                    rb_step.name.clone(),
                                    activity_rx,
                                    activity_stop.clone(),
                                    room_progress,
                                );
                                let activity_stop = activity_stop.drop_guard();
                                let outcome = execute_step(
                                    rb_step,
                                    &project_path,
                                    workflow.project_id.as_deref(),
                                    &work_dir,
                                    tokens_config,
                                    full_access,
                                    &ctx,
                                    None,
                                    Some(&activity_tx),
                                    Some(&agents_config.model_tiers),
                                    Some(&crate::models::setup::HttpEndpoints::from_agents(
                                        agents_config,
                                    )),
                                    Some(&ollama_context_overrides),
                                    native_tools,
                                    Some(&state.db),
                                    step_room.as_ref().map(|room| room.context()),
                                    Some(&run.id),
                                )
                                .await;
                                drop(activity_tx);
                                drop(activity_stop);
                                let _ = activity_publisher.await;
                                if let Some(room) = step_room {
                                    let stop = match outcome.result.status {
                                        RunStatus::Success => AgentRunStop::Finished,
                                        RunStatus::Cancelled => AgentRunStop::Cancelled,
                                        _ => AgentRunStop::Failed,
                                    };
                                    room.finish_as(stop).await;
                                }
                                outcome
                            }
                        }
                    }
                }
                StepType::Gate => {
                    // Gate in rollback would deadlock the run on a Failed
                    // status that no resume path serves — explicitly
                    // unsupported. The wizard rejects this at save time
                    // (see validate_on_failure_steps).
                    super::gate_step::execute_gate_step(rb_step, &ctx)
                }
                StepType::Exec => {
                    // Exec in rollback is allowed (e.g. `make revert`
                    // as a compensation step). Same allowlist enforced.
                    super::exec_step::execute_exec_step_for_project(
                        rb_step,
                        &workflow.exec_allowlist,
                        &work_dir,
                        &ctx,
                        workflow.project_id.as_deref(),
                        carrying_repository.as_deref(),
                        super::exec_step::output_limit_for(
                            rb_step,
                            &workflow.steps,
                            &workflow.on_failure,
                        ),
                    )
                    .await
                }
                StepType::BatchApiCall => {
                    // BatchApiCall in rollback is meaningful for compensation
                    // (e.g. POST /issue/{key}/transitions = "Cancelled" over
                    // every ticket the failed run had created). Same plugin
                    // wiring as the linear path.
                    let api_caller =
                        crate::core::api_access::workflow_caller(&state, workflow).await;
                    super::batch_apicall_step::execute_batch_apicall_step(
                        rb_step,
                        workflow.project_id.as_deref(),
                        &state,
                        &ctx,
                        super::api_call_executor::ApiCallLogContext::workflow_step(run.id.clone()),
                        &api_caller,
                    )
                    .await
                }
                StepType::JsonData => {
                    // Marginal en rollback (peu d'intérêt à émettre du
                    // JSON littéral comme step de compensation), mais on
                    // l'accepte pour rester cohérent avec le linear path.
                    super::json_data_step::execute_json_data_step(rb_step).await
                }
                StepType::CollectApiData => {
                    let api_caller =
                        crate::core::api_access::workflow_caller(&state, workflow).await;
                    super::collect_api_data_step::execute_collect_api_data_step(
                        rb_step,
                        workflow.project_id.as_deref(),
                        &state,
                        &ctx,
                        super::api_call_executor::ApiCallLogContext::workflow_step(run.id.clone()),
                        &workflow.exec_allowlist,
                        &work_dir,
                        &api_caller,
                    )
                    .await
                }
                StepType::TransformData => {
                    super::transform_data_step::execute_transform_data_step(rb_step, &ctx).await
                }
                StepType::PublishPageData => {
                    super::publish_page_step::execute_publish_page_data_step(
                        rb_step,
                        &workflow.id,
                        &run.id,
                        run.project_id.as_deref().or(workflow.project_id.as_deref()),
                        &state,
                        &ctx,
                    )
                    .await
                }
                StepType::TaskBoard => {
                    super::task_board_step::execute_task_board_step(
                        rb_step,
                        &workflow.id,
                        &state,
                        &ctx,
                    )
                    .await
                }
                StepType::Decide => {
                    let base = crate::api::ollama_health::resolve_base_url_pub(
                        state.ollama_base_url_override.as_deref(),
                    );
                    super::decide_step::execute_decide_step(
                        rb_step,
                        &ctx,
                        &state,
                        &base,
                        agents_config.model_tiers.ollama.decision.as_deref(),
                    )
                    .await
                }
                StepType::SubWorkflow => {
                    // FORBIDDEN in on_failure (save-validated). Defence-in-depth
                    // for a hand-edited JSON: fail loudly, never recurse from
                    // a rollback step.
                    super::sub_workflow_step::forbidden_in_rollback(rb_step)
                }
                // An independent run, e.g. a cleanup workflow: nothing recurses.
                StepType::TriggerWorkflow => {
                    super::trigger_workflow_step::execute_trigger_workflow_step(
                        &state, workflow, &run.id, rb_step, &ctx,
                    )
                    .await
                }
                // Forbidden in on_failure (save-validated); a hand-edited JSON fails here.
                StepType::DelegateSubtasks => {
                    super::delegate_subtasks_step::forbidden_in_rollback(rb_step)
                }
            };

            ctx.set_step_output(&rb_step.name, &rb_outcome.result.output);
            for (k, v) in super::template::extract_step_markers(&rb_outcome.result.output).1 {
                run.state.insert(k, v);
            }
            run.tokens_used += rb_outcome.result.tokens_used.unwrap_or(0);
            apply_step_snapshot(
                rb_step,
                &mut rb_outcome.result,
                Some(&agents_config.model_tiers),
            );
            ctx.set_step_kind(&rb_step.name, rb_outcome.result.step_kind.as_deref());
            // 2026-06-10 (audit P1) — mark this result as COMPENSATION so the
            // UI renders it under a dedicated rollback section. Pre-fix a
            // green rollback step right after the failed step read as "the
            // run continued past the failure".
            rb_outcome.result.is_rollback = true;

            let rb_failed = rb_outcome.result.status == RunStatus::Failed;
            emit(RunEvent::StepDone {
                step_result: Box::new(rb_outcome.result.clone()),
            });
            run.step_results.push(rb_outcome.result);

            if rb_failed {
                tracing::warn!(
                    target: "kronn::workflow_rollback",
                    run_id = %run.id, step = %rb_step.name,
                    "Rollback step failed — skipping remaining compensation steps"
                );
                break;
            }
        }
    }

    if !paused {
        run.finished_at = Some(Utc::now());
    }

    timeout_clock.store(run);
    let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
    let terminal_snapshot_run_id = (!paused).then(|| run.id.clone());
    let db5 = db.clone();
    db5.with_conn(move |conn| {
        let updated = crate::db::workflows::update_run_progress(conn, snap)?;
        if let Some(run_id) = terminal_snapshot_run_id.as_deref() {
            crate::db::execution_variable_snapshots::purge_run_lifetime_snapshot(
                conn,
                "workflow",
                run_id,
                Utc::now(),
            )?;
            super::run_pins::purge(conn, run_id)?;
        }
        Ok(updated)
    })
    .await?;

    // Every guard exit (in-flight or between-step timeout, LLM quota, loop
    // detection) ends here. Settled after the terminal write: an admission
    // that runs later sees the stopped parent and refuses, one that ran
    // earlier left a child this sweep cancels, with its whole subtree.
    if run.status == RunStatus::StoppedByGuard {
        if let Err(error) = super::cancellation::cancel_run_tree(
            &state,
            &run.id,
            super::cancellation::CancellationScope::DescendantsOnly,
            "workflow_guard_stop",
        )
        .await
        {
            tracing::error!(run_id = %run.id, "Unable to settle descendants after a guard stop: {error}");
        }
    }

    // Emit run done
    emit(RunEvent::RunDone {
        status: run.status.clone(),
    });
    // 0.8.2 — cross-tab final state flip so any open WorkflowDetail can
    // clear its live indicator without polling.
    broadcast_run_state(&run.status, run.step_results.len() as i32 - 1, None);

    // Cleanup workspace — but NOT if we just paused: the worktree
    // (with its uncommitted artifacts and hooks) must persist until
    // the operator decides to resume or reject. And NOT when this run
    // INHERITED the parent's worktree (Phase 2 sub-workflow child): the
    // parent owns that tree's lifecycle — cleaning it here would delete
    // the child's implementation before the parent's `create_pr` runs.
    let active_child_dispatches = if run.workspace_path.is_some() {
        let owner_run_id = run.id.clone();
        state
            .db
            .with_conn(move |conn| {
                crate::db::workflows::active_child_dispatch_count(conn, &owner_run_id)
            })
            .await
            .unwrap_or_else(|error| {
                tracing::error!(run_id = %run.id, "failed to verify child workspace ownership: {error}");
                // Fail closed: an unavailable refcount must never authorize
                // deletion of a checkout children may still be reading.
                1
            })
    } else {
        0
    };
    if !paused && !is_inherited_workspace && active_child_dispatches == 0 {
        if let Some(ws) = workspace {
            // KT-1806 — a success keeps uncommitted work too: a step's output
            // can be files that no later step committed.
            let cleaned = ws.cleanup_keeping_work().await;
            match cleaned {
                Ok(outcome) => {
                    if let Some(kept) = &outcome.kept {
                        let note = kept_checkout_note(kept);
                        tracing::warn!(run_id = %run.id, "{note}");
                        if let Some(last) = run.step_results.last_mut() {
                            last.output = format!("{}\n\n{note}", last.output);
                        }
                        let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
                        if let Err(error) = state
                            .db
                            .with_conn(move |conn| {
                                crate::db::workflows::update_run_progress(conn, snap)
                            })
                            .await
                        {
                            tracing::error!(run_id = %run.id, "failed to record the kept worktree: {error}");
                        }
                    }
                    if let Some(preserved) = outcome.preserved {
                        // Persist the preserved branch on the run row so the
                        // UI can surface it — without this, the operator only
                        // sees a tracing log line they'll never read.
                        record_produced_branch(run, preserved);
                        let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
                        let db = state.db.clone();
                        if let Err(e) = db
                            .with_conn(move |conn| {
                                crate::db::workflows::update_run_progress(conn, snap)
                            })
                            .await
                        {
                            tracing::error!(
                                run_id = %run.id,
                                error = %e,
                                "failed to persist produced_branches — the preserved branch pointer is the only record of this run's work"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Workspace cleanup failed: {}", e);
                }
            }
        }
        forget_removed_workspace(&state, run).await;
    } else if active_child_dispatches > 0 {
        tracing::warn!(
            run_id = %run.id,
            active_child_dispatches,
            "workspace cleanup deferred: child dispatches still own read access"
        );
    }

    tracing::info!("Workflow run {} finished: {:?}", run.id, run.status);
    Ok(())
}

/// Build the maximum iteration safeguard for a given step count.
/// Formula: total_steps * 10 + 50.
pub fn max_iterations_for(total_steps: usize) -> usize {
    total_steps * 10 + 50
}

/// 0.7.0 Phase 4 — operator's decision on a paused (Gate) run.
///
/// Three outcomes:
///   - `Approve`: continue from the next step. The Gate's StepResult
///     status flips from `WaitingApproval` to `Success`; the optional
///     comment is appended to its output as a `> Décision:` footer.
///   - `RequestChanges`: jump back to a target step and re-run from
///     there. Default target: the step preceding the gate
///     (Auto-Dev `pause_pre_merge → goto: implement` pattern).
///     StepResults from the target onward are discarded so the run
///     replays cleanly.
///   - `Reject`: terminate the run with status `Failed`. No further
///     steps execute.
///
/// The `comment` field is operator-supplied free text — required by
/// the UX for `RequestChanges` (the agent needs to know what to fix)
/// but optional for the others.
#[derive(Debug, Clone)]
pub enum GateDecision {
    Approve { comment: Option<String> },
    RequestChanges { comment: String },
    Reject { comment: Option<String> },
}

/// Resume a paused workflow run after the operator has decided.
///
/// Mutates `run` in place — applies the decision to the trailing
/// `WaitingApproval` step result, sets up the runner state for
/// continuation (or terminal failure), and dispatches accordingly:
///   - Approve / RequestChanges → re-enters [`execute_run`] which now
///     starts from `step_results.len()` and skips workspace creation
///     thanks to the `is_resume` detection.
///   - Reject → marks the run as `Failed` and persists, no re-entry.
///
/// Caller (the API endpoint) is responsible for:
///   - loading the run + workflow from DB before calling
///   - persisting the final state via the runner's own progress writes
///   - holding the cancel-registry / events_tx if streaming is desired
///     (currently None — Phase 4 keeps the resume non-streamed; the
///     Phase 4b polish pass will reuse the SSE channel)
pub async fn resume_run(
    state: AppState,
    workflow: &Workflow,
    run: &mut WorkflowRun,
    decision: GateDecision,
    tokens_config: &TokensConfig,
    agents_config: &AgentsConfig,
    events_tx: Option<EventSender>,
) -> Result<()> {
    use anyhow::anyhow;
    // The gate's targets come from the revision the run executes (KT-1096).
    let pinned_run_id = run.id.clone();
    let pinned = state
        .db
        .with_read_conn(move |conn| super::run_pins::pinned_workflow(conn, &pinned_run_id))
        .await?;
    let workflow = pinned.as_ref().unwrap_or(workflow);
    let workflow_in_project = workflow_in_run_project(workflow, run);
    let workflow: &Workflow = &workflow_in_project;

    let gate_step_idx = run
        .step_results
        .len()
        .checked_sub(1)
        .ok_or_else(|| anyhow!("Cannot resume run {}: no step results", run.id))?;

    {
        let last = &mut run.step_results[gate_step_idx];
        if last.status != RunStatus::WaitingApproval {
            return Err(anyhow!(
                "Cannot resume run {}: trailing step status is {:?}, expected WaitingApproval",
                run.id,
                last.status
            ));
        }
        // 0.8.2 — Rewrite the gate's `duration_ms` to reflect the actual
        // pause time (now - started_at) instead of the ~0ms executor
        // render time it held until approval. Without this, the front-end
        // live-elapsed counter on the next step inflated to include the
        // whole pause (`runStart + sum of completed durations` estimate
        // ignored the invisible gate pause). Falls back to the original
        // value when `started_at` is missing — old gate rows pre-dating
        // this field stay readable.
        if let Some(started_at) = last.started_at {
            let elapsed_ms = (Utc::now() - started_at).num_milliseconds().max(0) as u64;
            last.duration_ms = elapsed_ms;
        }
        match &decision {
            GateDecision::Approve { comment } => {
                last.status = RunStatus::Success;
                append_decision_footer(last, "Approuvé", comment.as_deref());
            }
            GateDecision::RequestChanges { comment } => {
                last.status = RunStatus::Success;
                append_decision_footer(last, "Changements demandés", Some(comment));
            }
            GateDecision::Reject { comment } => {
                last.status = RunStatus::Failed;
                append_decision_footer(last, "Rejeté", comment.as_deref());
            }
        }
    }

    // Persist the human's gate comment into `run.state` BEFORE the (later)
    // `truncate` drops the gate step result. The comment was only ever
    // appended to the gate result's footer via `append_decision_footer`; on a
    // RequestChanges the run truncates step_results down to the target step
    // (e.g. `analyze`/`implement`), which removes the gate result entirely —
    // so the re-run never saw the feedback. `run.state` survives truncate and
    // is seeded into the template ctx, so the target step can read
    // `{{state.last_human_feedback}}`. (Same mechanism as `state.last_review`
    // from the review→implement loop, but for the HUMAN gate path.)
    match &decision {
        GateDecision::RequestChanges { comment } if !comment.trim().is_empty() => {
            run.state
                .insert("last_human_feedback".into(), comment.clone());
        }
        GateDecision::Approve { comment: Some(c) } if !c.trim().is_empty() => {
            run.state.insert("last_human_feedback".into(), c.clone());
        }
        _ => {}
    }

    let gate_step_name = run.step_results[gate_step_idx].step_name.clone();

    // Reject is terminal — no need to re-spawn execute_run.
    if matches!(decision, GateDecision::Reject { .. }) {
        run.status = RunStatus::Failed;
        run.finished_at = Some(Utc::now());
        let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
        let db = state.db.clone();
        db.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
            .await?;
        // 0.8.2 — cross-tab WS broadcast: a Reject is terminal, so
        // execute_run won't re-emit. Without this, other tabs viewing
        // the same run stay stuck on "WaitingApproval" until refresh.
        let _ = state
            .ws_broadcast
            .send(crate::models::WsMessage::WorkflowRunUpdated {
                run_id: run.id.clone(),
                workflow_id: workflow.id.clone(),
                status: format!("{:?}", run.status),
                step_index: gate_step_idx as i32,
                total_steps: workflow.steps.len() as u32,
                current_step: None,
            });
        let _ = state
            .ws_broadcast
            .send(crate::models::WsMessage::SharedRunUpdated {
                run_id: run.id.clone(),
            });
        // Reject still preserves anything the agent committed — the
        // operator's "no" is on the gate, not on the work already on disk.
        if let Err(e) = reclaim_run_worktree(&state, workflow, run).await {
            tracing::warn!(
                run_id = %run.id,
                error = %e,
                "workspace cleanup failed on Reject — worktree may be left behind"
            );
        }
        return Ok(());
    }

    // RequestChanges → jump back to the target step. Truncate
    // step_results so the engine replays from the target onward.
    if let GateDecision::RequestChanges { .. } = &decision {
        // 2026-06-10 audit P1 — both lookups below were keyed off
        // `gate_step_idx`, which is a `step_results` index. After a Goto
        // loop, `step_results.len()` > `workflow.steps.len()` and the two
        // diverge: `workflow.steps.get(gate_step_idx)` read the WRONG step
        // (or None → silently lost the configured target), and the
        // positional `truncate(target_idx)` cut step_results at the wrong
        // place. Resolve the gate step in `workflow.steps` BY NAME instead
        // (the same model `next_step_index_for_resume` uses on resume).
        let gate_step_def = workflow.steps.iter().find(|s| s.name == gate_step_name);
        let target_name = gate_step_def.and_then(|s| s.gate_request_changes_target.clone());

        // Workflow-steps index of the target (where to replay FROM).
        // Default: the step linearly before the gate. Named-but-missing
        // target → warn + same fallback.
        let gate_pos = workflow.steps.iter().position(|s| s.name == gate_step_name);
        let target_idx = if let Some(name) = target_name {
            workflow
                .steps
                .iter()
                .position(|s| s.name == name)
                .unwrap_or_else(|| {
                    tracing::warn!(
                        "Gate '{}' request_changes_target '{}' not found, falling back to previous step",
                        gate_step_name, name
                    );
                    gate_pos.map(|p| p.saturating_sub(1)).unwrap_or(0)
                })
        } else {
            gate_pos.map(|p| p.saturating_sub(1)).unwrap_or(0)
        };

        // Truncate step_results so the run REPLAYS from the target step.
        // Cut at the FIRST step_result that is the target (by name): the
        // rows before it are the clean linear prefix, so the resume cursor
        // lands ON the target (see request_changes_cut for the run-10
        // post-Goto bug the most-recent variant caused). If the target
        // never ran, fall back to the bounded positional index.
        let target_step_name = workflow.steps.get(target_idx).map(|s| s.name.clone());
        let cut = request_changes_cut(&run.step_results, target_step_name.as_deref(), target_idx);
        run.step_results.truncate(cut);

        // The target re-runs on top of the checkpoint; verify the run's own
        // worktree is still exactly that commit (KT-1042).
        let checkpoint_key = format!(
            "{}{}",
            super::gate_checkpoint::CHECKPOINT_STATE_PREFIX,
            gate_step_name,
        );
        if let Some(sha) = run.state.get(&checkpoint_key).cloned() {
            match run.workspace_path.as_deref() {
                None => tracing::warn!(
                    run_id = %run.id,
                    gate = %gate_step_name,
                    sha = %sha,
                    "checkpoint check skipped — the run has no worktree of its own",
                ),
                Some(worktree) => {
                    match super::gate_checkpoint::verify_checkpoint(
                        std::path::Path::new(worktree),
                        &sha,
                    ) {
                        Ok(()) => tracing::info!(
                            run_id = %run.id,
                            gate = %gate_step_name,
                            sha = %sha,
                            "worktree still at the checkpoint before Goto",
                        ),
                        Err(e) => tracing::warn!(
                            run_id = %run.id,
                            gate = %gate_step_name,
                            sha = %sha,
                            error = %e,
                            "worktree moved since the checkpoint — continuing Goto on its current state",
                        ),
                    }
                }
            }
        }
    }

    // Approve and RequestChanges both flow into execute_run.
    run.status = RunStatus::Running;
    execute_run(
        state,
        workflow,
        run,
        tokens_config,
        agents_config,
        events_tx,
        None,
        None,
    )
    .await
}

/// The LLM calls a root run's tree already spent, for a (re)started root: the
/// durable tree count, its own stored counters, or for a run from before both,
/// its history and its sub-workflow children's.
pub async fn root_llm_calls_spent(
    state: &AppState,
    run: &WorkflowRun,
    counters: &super::guard_counters::GuardCounters,
) -> Result<u32> {
    let root = run.id.clone();
    let legacy = !super::guard_counters::GuardCounters::is_stored(&run.state);
    let durable = state
        .db
        .with_conn(move |conn| {
            let tree = crate::db::workflows::tree_llm_calls(conn, &root)?;
            let children = if legacy {
                crate::db::workflows::descendant_llm_calls_from_history(conn, &root)?
            } else {
                0
            };
            Ok((tree, children))
        })
        .await;
    let (tree, children) = durable
        .map_err(|error| anyhow::anyhow!("the run's LLM-call count is unreadable: {error}"))?;
    Ok(tree.max(counters.llm_calls + children))
}

/// Raises the root's durable count to the shared total: a child's calls must
/// survive a crash before its parent's step returns (KT-1046).
pub async fn record_tree_llm_calls(state: &AppState, budget: &SharedBudget) -> Result<()> {
    let Some(root) = budget.root_run_id().map(str::to_string) else {
        return Ok(());
    };
    let total = budget.llm_calls();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::raise_tree_llm_calls(conn, &root, total))
        .await?;
    budget
        .recorded
        .fetch_max(total, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// Takes back one call counted before launch that the provider refused for
/// quota: a refused call spent nothing (KT-1046). Interrupted calls stay counted.
pub async fn release_quota_refused_llm_call(state: &AppState, budget: &SharedBudget) -> Result<()> {
    if let Some(root) = budget.root_run_id().map(str::to_string) {
        state
            .db
            .with_conn(move |conn| crate::db::workflows::release_tree_llm_call(conn, &root))
            .await?;
    }
    decrement_saturating(&budget.llm_calls);
    decrement_saturating(&budget.recorded);
    Ok(())
}

/// Lowers a counter by one, never below zero. A plain CAS loop: `fetch_update`
/// is deprecated on newer toolchains and `try_update` is missing on older ones.
fn decrement_saturating(counter: &std::sync::atomic::AtomicU32) {
    use std::sync::atomic::Ordering::SeqCst;
    let mut current = counter.load(SeqCst);
    while current > 0 {
        match counter.compare_exchange_weak(current, current - 1, SeqCst, SeqCst) {
            Ok(_) => return,
            Err(seen) => current = seen,
        }
    }
}

/// The row that stops a run whose tree spent calls the database did not take.
fn budget_unrecorded_row(error: &anyhow::Error) -> StepResult {
    let reason = format!(
        "Stopped before the next step: the LLM calls already spent could not be \
         recorded ({error}), so the budget is not known."
    );
    StepResult {
        step_name: "__guard_llm_calls_unrecorded__".to_string(),
        status: RunStatus::Failed,
        output: reason.clone(),
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
        terminal_stop: Some(reason),
        output_bytes: None,
    }
}

pub const RUN_RESUME_HISTORY_KEY: &str = "__kronn.resume_history";

fn append_resume_transition(run: &mut WorkflowRun) {
    let now = Utc::now();
    let mut history = run
        .state
        .get(RUN_RESUME_HISTORY_KEY)
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .unwrap_or_else(|| serde_json::json!({ "v": 1, "events": [] }));
    if history
        .get("events")
        .and_then(serde_json::Value::as_array)
        .is_none()
    {
        history = serde_json::json!({ "v": 1, "events": [] });
    }
    let events = history["events"]
        .as_array_mut()
        .expect("resume history events normalized to an array");
    let paused_status = format!("{:?}", run.status);
    let last_status = events
        .last()
        .and_then(|event| event.get("status"))
        .and_then(serde_json::Value::as_str);
    if last_status != Some(paused_status.as_str()) {
        events.push(serde_json::json!({
            "status": paused_status,
            "at": run.finished_at.unwrap_or(now).to_rfc3339(),
        }));
    }
    events.push(serde_json::json!({
        "status": "Running",
        "at": now.to_rfc3339(),
    }));
    run.state
        .insert(RUN_RESUME_HISTORY_KEY.to_string(), history.to_string());
}

pub async fn claim_interrupted_run_row(state: &AppState, run: &mut WorkflowRun) -> Result<()> {
    let id = run.id.clone();
    try_claim_interrupted_run_row(state, run, None)
        .await?
        .map_err(|reason| anyhow::anyhow!("Run {id} cannot resume: {reason}"))
}

/// Claims an `Interrupted` or `WaitingQuota` run, its concurrency key and
/// resume trail included,
/// in the same closure as the admission check. `Ok(Err(reason))`: the
/// workflow's concurrency limit refused it; `run` is then left unchanged, so
/// the caller may wait and try again. `deadline`: the earliest timeout the
/// runners above a sub-workflow child enforce; past it the claim is refused.
pub async fn try_claim_interrupted_run_row(
    state: &AppState,
    run: &mut WorkflowRun,
    deadline: Option<chrono::DateTime<Utc>>,
) -> Result<std::result::Result<(), String>> {
    use anyhow::anyhow;
    if !matches!(run.status, RunStatus::Interrupted | RunStatus::WaitingQuota) {
        return Err(anyhow!(
            "Run {} is {:?} — only Interrupted or quota-waiting runs can be resumed",
            run.id,
            run.status
        ));
    }
    let from_status = run.status.clone();
    // The wait this copy read; owned so the closure below can carry it.
    let quota_wait = (run.status == RunStatus::WaitingQuota).then(|| {
        (
            run.step_results.len().saturating_sub(1),
            super::quota_wait::pending_wait(run).and_then(|wait| wait.id.clone()),
        )
    });
    let mut claim_run = run.clone();
    append_resume_transition(&mut claim_run);
    let candidate = claim_run.clone();
    let claimed = state
        .db
        .with_conn(move |conn| {
            if candidate.run_type == "subworkflow" {
                if let Some(reason) = super::concurrency::past_deadline(deadline) {
                    anyhow::bail!("Run {} cannot resume: {reason}", candidate.id);
                }
                if let Some(reason) = super::concurrency::parent_refuses_children(
                    conn,
                    candidate.parent_run_id.as_deref(),
                )? {
                    anyhow::bail!("Run {} cannot resume: {reason}", candidate.id);
                }
            }
            if let Some(workflow) =
                crate::db::workflows::get_workflow(conn, &candidate.workflow_id)?
            {
                if let Err(reason) =
                    super::concurrency::resume_within_limit(conn, &workflow, &candidate)?
                {
                    return Ok(Err(reason));
                }
            }
            crate::db::workflows::claim_paused_run_status(
                conn,
                &candidate.id,
                &from_status,
                quota_wait
                    .as_ref()
                    .map(|(index, id)| (*index, id.as_deref())),
                &candidate.state,
                candidate.concurrency_key.as_deref(),
            )
            .map(Ok)
        })
        .await?;
    let claimed = match claimed {
        Ok(claimed) => claimed,
        Err(reason) => return Ok(Err(reason)),
    };
    if !claimed {
        return Err(anyhow!(
            "Run {} was just claimed by another caller — resume ignored (no double-resume)",
            run.id
        ));
    }
    *run = claim_run;
    run.status = RunStatus::Running;
    run.finished_at = None;
    Ok(Ok(()))
}

/// A2 — validate + atomically claim an `Interrupted` top-level run for manual
/// resume.
///
/// Fast (no execution) so the API endpoint can await it BEFORE answering:
/// exactly one of two racing "Reprendre" clicks wins the
/// `Interrupted → Running` claim, the loser gets an error. Refusals:
///   - not `Interrupted` (Cancelled/Failed stay sticky — restart those fresh)
///   - a child (`subworkflow`) or `batch` run — resume the parent instead;
///     batch fan-out has its own machinery
///   - the preserved worktree is gone: resuming would silently fall back to
///     the MAIN checkout (execute_run's legacy resume fallback), which is
///     exactly the require_isolation hazard — refuse instead
pub async fn claim_interrupted_run(
    state: &AppState,
    run: &mut WorkflowRun,
    retry_uncertain_effect: bool,
) -> Result<()> {
    check_resume_preconditions(run, retry_uncertain_effect)?;
    claim_interrupted_run_row(state, run).await
}

/// The refusals every top-level resume applies, manual or after a quota reset
/// (KT-811): children and batches, a vanished worktree, and an external effect
/// with no durable result (fail-closed, KT-150).
pub fn check_resume_preconditions(
    run: &mut WorkflowRun,
    retry_uncertain_effect: bool,
) -> Result<()> {
    use anyhow::anyhow;
    if run.run_type == "subworkflow" || run.parent_run_id.is_some() {
        return Err(anyhow!(
            "Run {} is a sub-workflow child — resume its parent run instead (the foreach re-runs only the missing items)",
            run.id
        ));
    }
    if run.run_type == "batch" {
        return Err(anyhow!(
            "Run {} is a batch fan-out — batch runs cannot be resumed, re-trigger the workflow",
            run.id
        ));
    }
    if let Some(ws) = run.workspace_path.as_deref() {
        if !std::path::Path::new(ws).exists() {
            let preserved = run
                .produced_branches
                .iter()
                .map(|branch| format!("`{}`", branch.branch_name))
                .collect::<Vec<_>>();
            let kept = if preserved.is_empty() {
                String::new()
            } else {
                format!(" Its commits are kept on {}.", preserved.join(", "))
            };
            return Err(anyhow!(
                "Worktree `{}` no longer exists — refusing to resume in the main checkout. Re-trigger the workflow for a fresh run.{}",
                ws,
                kept
            ));
        }
    }
    if let Some(intent) = uncertain_side_effect_intent(run) {
        if !retry_uncertain_effect {
            return Err(anyhow!(
                "Step '{}' ({}) started before the crash, but its external effect has no durable result. Refusing an automatic replay; the operator must explicitly confirm retry_uncertain_effect.",
                intent.step_name,
                intent.step_type
            ));
        }
        run.state.remove(UNCERTAIN_SIDE_EFFECT_STATE_KEY);
    }
    Ok(())
}

/// Continues a run its caller already claimed, in the background, with the
/// same settle and unattended-failure notification as every resume.
pub fn spawn_claimed_resume(state: AppState, workflow: Workflow, mut run: WorkflowRun) {
    tokio::spawn(async move {
        let cfg = state.config.read().await;
        let tokens = cfg.tokens.clone();
        let agents = cfg.agents.clone();
        drop(cfg);
        if let Err(e) =
            resume_interrupted_run(state.clone(), &workflow, &mut run, &tokens, &agents, None).await
        {
            settle_errored_run(&state, &workflow, &mut run, &e).await;
        }
        crate::core::run_notify::notify_if_failed(&state, &workflow, &run).await;
    });
}

/// A2 — continue an `Interrupted` run the caller just claimed via
/// [`claim_interrupted_run`]. Everything resume-specific already lives in
/// [`execute_run`]: `is_resume` detection re-attaches the preserved worktree,
/// the cursor restarts at the step AFTER the last completed result (the
/// interrupted step re-runs from its start — its foreach items dedupe via the
/// done-set/git-ledger reconciliation), and prior step outputs + `run.state`
/// are replayed into the template ctx.
pub async fn resume_interrupted_run(
    state: AppState,
    workflow: &Workflow,
    run: &mut WorkflowRun,
    tokens_config: &TokensConfig,
    agents_config: &AgentsConfig,
    events_tx: Option<EventSender>,
) -> Result<()> {
    run.status = RunStatus::Running;
    run.finished_at = None;
    execute_run(
        state,
        workflow,
        run,
        tokens_config,
        agents_config,
        events_tx,
        None,
        None,
    )
    .await
}

/// Append a `> Décision: <verdict>` (and optional comment) footer to a
/// gate StepResult's output. Keeps the rendered gate message intact so
/// the operator's decision is visible alongside the original prompt
/// when reviewing the run history.
fn append_decision_footer(result: &mut StepResult, verdict: &str, comment: Option<&str>) {
    let separator = if result.output.is_empty() { "" } else { "\n\n" };
    match comment.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => {
            result.output.push_str(separator);
            result.output.push_str("> **Décision : ");
            result.output.push_str(verdict);
            result.output.push_str("**\n> ");
            result.output.push_str(&c.replace('\n', "\n> "));
        }
        None => {
            result.output.push_str(separator);
            result.output.push_str("> **Décision : ");
            result.output.push_str(verdict);
            result.output.push_str("**");
        }
    }
}

/// 0.7.0 Phase 3 — write extracted artifacts to disk for the artifacts
/// declared in `workflow.artifacts`. Undeclared artifacts are silently
/// skipped: `extract_artifacts` always populates the template context,
/// but persistence is opt-in via the workflow's artifact map.
///
/// Path resolution: `spec.path` is interpreted relative to `work_dir`
/// (the run's workspace root). Parent directories are created on demand.
/// Failures are logged but never propagate — a workflow run shouldn't
/// fail because the disk is full or the path is unwritable; the agent's
/// output is still in `StepResult.output` and the template context.
pub fn persist_declared_artifacts(
    workflow: &Workflow,
    extracted: &::std::collections::HashMap<String, String>,
    work_dir: &std::path::Path,
) {
    for (name, content) in extracted {
        let spec = match workflow.artifacts.get(name) {
            Some(s) => s,
            None => {
                tracing::debug!(
                    target: "kronn::workflow_artifact",
                    artifact = %name,
                    "agent emitted undeclared artifact — keeping in template context but skipping disk write"
                );
                continue;
            }
        };
        let target = work_dir.join(&spec.path);
        if let Some(parent) = target.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(
                    target: "kronn::workflow_artifact",
                    artifact = %name, path = %target.display(),
                    "failed to create artifact parent dir: {}", e
                );
                continue;
            }
        }
        match std::fs::write(&target, content) {
            Ok(()) => tracing::info!(
                target: "kronn::workflow_artifact",
                artifact = %name, path = %target.display(), bytes = content.len(),
                "persisted artifact"
            ),
            Err(e) => tracing::warn!(
                target: "kronn::workflow_artifact",
                artifact = %name, path = %target.display(),
                "failed to write artifact: {}", e
            ),
        }
    }
}

/// Replays a run's recorded step results into the context, in run order, as
/// if the run had never paused.
pub fn replay_prior_results(ctx: &mut TemplateContext, results: &[StepResult]) {
    for prior in results {
        ctx.set_step_output(&prior.step_name, &prior.output);
        ctx.set_step_kind(&prior.step_name, prior.step_kind.as_deref());
        let prior_agent = prior.step_agent.as_ref().map(|a| format!("{a:?}"));
        ctx.set_step_meta(
            &prior.step_name,
            prior_agent.as_deref(),
            prior.step_model.as_deref(),
        );
        ctx.set_step_provenance(&prior.step_name, prior.agent_provenance.as_deref());
        ctx.set_step_shadows(
            &prior.step_name,
            &super::shadow_summary::none(&prior.step_name).to_string(),
        );
    }
}

/// Stamp the step's "what was actually used here" metadata onto a
/// freshly-produced [`StepResult`] so editing the workflow afterwards
/// (swapping the agent, retargeting the plugin, changing the endpoint)
/// can't corrupt the run's history.
///
/// The frontend reads these fields back to render per-step badges in
/// [`RunDetail`] — `step_kind` drives the badge type, `step_agent` /
/// `step_api_plugin_slug` / `step_api_endpoint_path` populate the
/// subtitle.
///
/// Pulled out of `execute_run`'s loop so the snapshot logic is testable
/// in isolation (the loop itself needs a full workspace + agents to
/// drive end-to-end).
/// Post-execution bookkeeping for one completed step: snapshot the
/// "what actually ran" metadata onto the result row (the user can edit the
/// workflow between runs — history must describe THIS run), THEN seed the
/// template ctx (`steps.X.output/agent/model`) from the snapshotted result.
/// One function so the order can't silently regress.
pub fn record_step_completion(
    step: &WorkflowStep,
    result: &mut StepResult,
    ctx: &mut TemplateContext,
    model_tiers: Option<&crate::models::setup::ModelTiersConfig>,
) {
    apply_step_snapshot(step, result, model_tiers);
    ctx.set_step_output(&step.name, &result.output);
    ctx.set_step_kind(&step.name, result.step_kind.as_deref());
    let agent_name = result.step_agent.as_ref().map(|a| format!("{a:?}"));
    ctx.set_step_meta(
        &step.name,
        agent_name.as_deref(),
        result.step_model.as_deref(),
    );
    ctx.set_step_provenance(&step.name, result.agent_provenance.as_deref());
    // A step with shadows replaces this once its executions are read back.
    ctx.set_step_shadows(
        &step.name,
        &super::shadow_summary::none(&step.name).to_string(),
    );
}

pub fn apply_step_snapshot(
    step: &WorkflowStep,
    result: &mut StepResult,
    model_tiers: Option<&crate::models::setup::ModelTiersConfig>,
) {
    let kind: &'static str = match step.step_type {
        StepType::ApiCall => "ApiCall",
        StepType::Notify => "Notify",
        StepType::BatchQuickPrompt => "BatchQuickPrompt",
        StepType::Agent => "Agent",
        StepType::Gate => "Gate",
        StepType::Exec => "Exec",
        StepType::BatchApiCall => "BatchApiCall",
        StepType::JsonData => "JsonData",
        StepType::CollectApiData => "CollectApiData",
        StepType::TransformData => "TransformData",
        StepType::PublishPageData => "PublishPageData",
        StepType::SubWorkflow => "SubWorkflow",
        StepType::TriggerWorkflow => "TriggerWorkflow",
        StepType::DelegateSubtasks => "DelegateSubtasks",
        StepType::TaskBoard => "TaskBoard",
        StepType::Decide => "Decide",
    };
    result.step_kind = Some(kind.into());
    // KT-1525 — what the step left in the run: the run view grades it.
    result.output_bytes = Some(result.output.len() as u64);
    if matches!(step.step_type, StepType::Agent) {
        if let Some(provenance) = &result.agent_provenance {
            let selected = provenance
                .selected_attempt
                .and_then(|id| provenance.attempts.iter().find(|attempt| attempt.id == id))
                // Failed launches have no retained output, but their last
                // attempted provider/model must remain diagnosable in badges.
                .or_else(|| provenance.attempts.last());
            result.step_agent = selected.map(|attempt| attempt.agent.clone());
            result.step_model = selected.and_then(|attempt| {
                let model = if !attempt.observed_models.is_empty() {
                    Some(attempt.observed_models.join(" / "))
                } else if attempt.model_applied == Some(false) {
                    None
                } else {
                    attempt.resolved_model.clone()
                };
                model.map(|model| match attempt.tier {
                    crate::models::ModelTier::Default => model,
                    tier => format!("{model} · {}", format!("{tier:?}").to_lowercase()),
                })
            });
            return;
        }
    }
    result.step_agent = matches!(step.step_type, StepType::Agent).then(|| step.agent.clone());
    // 2026-06-13 — stamp the model/tier actually resolved for this Agent step
    // so the UI shows the real model on EVERY agent step (incl. per-item
    // fan-out routing), not only steps with an explicit reasoning tier.
    if matches!(step.step_type, StepType::Agent) {
        let settings = step.agent_settings.as_ref();
        let tier = settings.and_then(|s| s.tier).unwrap_or_default();
        // explicit model override wins; else resolve the tier → concrete model
        let model = settings
            .and_then(|s| s.model.clone())
            .or_else(|| crate::agents::runner::resolve_model_flag(&step.agent, tier, model_tiers));
        // label e.g. "opus", "sonnet · reasoning", "haiku · economy"
        result.step_model = match (model, tier) {
            (Some(m), crate::models::ModelTier::Default) => Some(m),
            (Some(m), t) => Some(format!("{m} · {}", format!("{t:?}").to_lowercase())),
            (None, crate::models::ModelTier::Default) => None,
            (None, t) => Some(format!("{t:?}").to_lowercase()),
        };
    }
    if matches!(step.step_type, StepType::ApiCall) {
        result.step_api_plugin_slug = step.api_plugin_slug.clone();
        result.step_api_endpoint_path = step.api_endpoint_path.clone();
    }
}

/// Inject trigger context JSON into template variables.
fn inject_trigger_context(ctx: &mut TemplateContext, trigger_json: &serde_json::Value) {
    if let Some(obj) = trigger_json.as_object() {
        // Issue fields from tracker trigger
        if let Some(title) = obj.get("issue_title").and_then(|v| v.as_str()) {
            ctx.set("issue.title", title);
        }
        if let Some(body) = obj.get("issue_body").and_then(|v| v.as_str()) {
            ctx.set("issue.body", body);
        }
        if let Some(number) = obj.get("issue_number").and_then(|v| v.as_str()) {
            ctx.set("issue.number", number);
        } else if let Some(number) = obj.get("issue_number").and_then(|v| v.as_u64()) {
            ctx.set("issue.number", number.to_string());
        }
        if let Some(url) = obj.get("issue_url").and_then(|v| v.as_str()) {
            ctx.set("issue.url", url);
        }
        if let Some(labels) = obj.get("issue_labels").and_then(|v| v.as_array()) {
            let label_strs: Vec<String> = labels
                .iter()
                .filter_map(|l| l.as_str().map(String::from))
                .collect();
            ctx.set("issue.labels", label_strs.join(", "));
        }

        // Generic fields — expose any string value from trigger context
        for (key, value) in obj {
            if let Some(s) = value.as_str() {
                ctx.set(key.clone(), s);
            }
        }
    }
}

/// A step dropped mid-flight may already have spent model tokens it never
/// reported; only step kinds that run no model can claim a measured zero.
fn interrupted_step_tokens(step_type: &StepType) -> Option<u64> {
    match step_type {
        StepType::Agent
        | StepType::BatchQuickPrompt
        | StepType::SubWorkflow
        | StepType::DelegateSubtasks => None,
        _ => Some(0),
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_gate_webhook_does_not_follow_a_redirect_to_an_internal_address() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(path("/hook"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", "http://169.254.169.254/latest/meta-data/"),
            )
            .mount(&server)
            .await;
        let err = fire_gate_webhook(
            &format!("{}/hook", server.uri()),
            &serde_json::json!({"run_id": "r"}),
            crate::core::safe_http::SafeHttpPolicy::PublicOrLoopbackForTests,
        )
        .await
        .unwrap_err();
        assert!(err.contains("Security"), "{err}");
        let literal = fire_gate_webhook(
            "http://[::ffff:127.0.0.1]:3140/api/config",
            &serde_json::json!({}),
            crate::core::safe_http::SafeHttpPolicy::Public,
        )
        .await
        .unwrap_err();
        assert!(literal.contains("Security"), "{literal}");
    }

    // ─── inject_and_consume_gate_feedback — gate feedback, approach B ───
    #[test]
    fn gate_feedback_is_injected_and_consumed_once() {
        let mut state = std::collections::HashMap::new();
        state.insert(
            "last_human_feedback".to_string(),
            "Use the existing CircuitBreaker, don't add a new one".to_string(),
        );
        let mut prompt = "Implémente le plan validé.".to_string();

        // First step after the gate: injected + feedback present in prompt.
        let injected = inject_and_consume_gate_feedback(&mut prompt, &mut state);
        assert!(injected);
        assert!(
            prompt.contains("Use the existing CircuitBreaker"),
            "feedback must be prepended"
        );
        assert!(
            prompt.contains("Implémente le plan validé."),
            "original prompt preserved"
        );
        // Consumed — no longer in state.
        assert!(!state.contains_key("last_human_feedback"));

        // Subsequent step in the same run: nothing left → no-op, prompt untouched.
        let mut prompt2 = "Run the tests.".to_string();
        let injected2 = inject_and_consume_gate_feedback(&mut prompt2, &mut state);
        assert!(!injected2);
        assert_eq!(prompt2, "Run the tests.");
    }

    #[test]
    fn gate_feedback_noop_when_absent_or_blank() {
        let mut state = std::collections::HashMap::new();
        let mut prompt = "do work".to_string();
        assert!(!inject_and_consume_gate_feedback(&mut prompt, &mut state));
        assert_eq!(prompt, "do work");

        // Blank comment is treated as no feedback (still consumed, not injected).
        state.insert("last_human_feedback".to_string(), "   ".to_string());
        assert!(!inject_and_consume_gate_feedback(&mut prompt, &mut state));
        assert_eq!(prompt, "do work");
    }

    // ─── a project prepares its own worktrees ──────────────────

    fn hooks(
        after_create: Option<&str>,
        before_run: Option<&str>,
    ) -> crate::models::WorkspaceHooks {
        crate::models::WorkspaceHooks {
            after_create: after_create.map(str::to_string),
            before_run: before_run.map(str::to_string),
            after_run: None,
            before_remove: None,
        }
    }

    #[test]
    fn a_workflow_inherits_the_project_recipe_and_overrides_it_field_by_field() {
        let project = hooks(Some("make worktree-up"), Some("make deps"));

        // An unspecified workflow hook inherits the project hook.
        let inherited = resolved_workspace_hooks(
            Some(&project),
            Some(&WorkspaceConfig {
                hooks: crate::models::WorkspaceHooks::default(),
                require_isolation: true,
                main_tree_read_only: false,
                base_ref: None,
            }),
        )
        .expect("hooks");
        assert_eq!(inherited.after_create.as_deref(), Some("make worktree-up"));
        assert_eq!(inherited.before_run.as_deref(), Some("make deps"));

        // Knows better for one field, keeps the rest.
        let overridden = resolved_workspace_hooks(
            Some(&project),
            Some(&WorkspaceConfig {
                hooks: hooks(Some("./scripts/one-off.sh"), None),
                require_isolation: true,
                main_tree_read_only: false,
                base_ref: None,
            }),
        )
        .expect("hooks");
        assert_eq!(
            overridden.after_create.as_deref(),
            Some("./scripts/one-off.sh")
        );
        assert_eq!(
            overridden.before_run.as_deref(),
            Some("make deps"),
            "the rest is inherited"
        );
    }

    #[test]
    fn a_project_without_a_recipe_changes_nothing() {
        // No project section, no workflow hooks: exactly what the runner
        // passed before this existed.
        assert!(resolved_workspace_hooks(None, None).is_none());
        assert!(resolved_workspace_hooks(
            None,
            Some(&WorkspaceConfig {
                hooks: crate::models::WorkspaceHooks::default(),
                require_isolation: true,
                main_tree_read_only: false,
                base_ref: None,
            })
        )
        .is_none());
        // And a workflow that declares its own keeps them untouched.
        let own = resolved_workspace_hooks(
            None,
            Some(&WorkspaceConfig {
                hooks: hooks(Some("./prepare.sh"), None),
                require_isolation: false,
                main_tree_read_only: false,
                base_ref: None,
            }),
        )
        .expect("hooks");
        assert_eq!(own.after_create.as_deref(), Some("./prepare.sh"));
    }

    #[test]
    fn a_project_recipe_does_not_force_a_worktree_on_a_workflow_that_wants_none() {
        // Isolation stays the workflow's decision: inheriting a recipe must
        // not turn a read-only fan-out into an isolated run.
        let project = hooks(Some("make worktree-up"), None);
        assert!(!workflow_requests_workspace(Some(&WorkspaceConfig {
            hooks: crate::models::WorkspaceHooks::default(),
            require_isolation: false,
            main_tree_read_only: false,
            base_ref: None,
        })));
        assert!(resolved_workspace_hooks(Some(&project), None).is_some());
    }

    // ─── SharedBudget (Phase 1b-ii) ─────────────────────────────────────

    #[test]
    fn shared_budget_clone_shares_one_counter_and_cap() {
        // A child's budget is a CLONE of the parent's — adding on either
        // side moves the SAME tree-wide counter, and the cap is shared.
        let root = super::SharedBudget::root(3);
        let child = root.clone();
        assert_eq!(root.llm_calls(), 0);
        root.add_llm_calls(1); // parent spends 1
        child.add_llm_calls(1); // child spends 1 — same counter
        assert_eq!(root.llm_calls(), 2, "both views observe the shared count");
        assert_eq!(child.llm_calls(), 2);
        assert_eq!(child.max_llm_calls(), 3, "child inherits the root cap");
        // The whole tree trips the cap together.
        child.add_llm_calls(1);
        assert!(
            root.llm_calls() >= root.max_llm_calls(),
            "tree-wide quota reached"
        );
    }

    #[test]
    fn workspace_is_opt_in_but_legacy_hooks_keep_requesting_it() {
        assert!(!workflow_requests_workspace(None));
        assert!(!workflow_requests_workspace(Some(&WorkspaceConfig {
            hooks: WorkspaceHooks::default(),
            require_isolation: false,
            main_tree_read_only: false,
            base_ref: None,
        })));
        assert!(workflow_requests_workspace(Some(&WorkspaceConfig {
            hooks: WorkspaceHooks::default(),
            require_isolation: true,
            main_tree_read_only: false,
            base_ref: None,
        })));
        assert!(workflow_requests_workspace(Some(&WorkspaceConfig {
            hooks: WorkspaceHooks {
                before_run: Some("echo prepare".into()),
                ..WorkspaceHooks::default()
            },
            require_isolation: false,
            main_tree_read_only: false,
            base_ref: None,
        })));
        // A starting point only a worktree can honour requests one; a blank one does not.
        let based = |base_ref: &str| WorkspaceConfig {
            hooks: WorkspaceHooks::default(),
            require_isolation: false,
            main_tree_read_only: true,
            base_ref: Some(base_ref.into()),
        };
        assert!(workflow_requests_workspace(Some(&based("origin/main"))));
        assert!(!workflow_requests_workspace(Some(&based("   "))));
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

    #[test]
    fn fresh_run_starts_at_zero() {
        let steps = vec![fake_step("a"), fake_step("b")];
        let results: Vec<crate::models::StepResult> = vec![];
        assert_eq!(next_step_index_for_resume(&steps, &results), 0);
    }

    #[test]
    fn linear_resume_continues_after_last_result() {
        let steps = vec![fake_step("a"), fake_step("b"), fake_step("c")];
        let results = vec![fake_result("a"), fake_result("b")];
        assert_eq!(next_step_index_for_resume(&steps, &results), 2);
    }

    #[test]
    fn interrupted_resume_retries_trailing_running_placeholder() {
        let steps = vec![fake_step("a"), fake_step("b"), fake_step("c")];
        let mut running = fake_result("b");
        running.status = RunStatus::Running;
        running.started_at = Some(Utc::now());
        let results = vec![fake_result("a"), running];
        assert_eq!(next_step_index_for_resume(&steps, &results), 1);
    }

    #[test]
    fn resume_after_goto_loop_uses_step_name_not_results_len() {
        // Ticket Autopilot regression: review→implement Goto fired twice,
        // so step_results has duplicate entries. The next index after the
        // gate (last result) must come from the workflow definition, NOT
        // from `step_results.len()` which would overshoot the workflow.
        let steps = vec![
            fake_step("fetch"),       // 0
            fake_step("analyze"),     // 1
            fake_step("plan_gate"),   // 2
            fake_step("implement"),   // 3
            fake_step("run_tests"),   // 4
            fake_step("review"),      // 5
            fake_step("create_pr"),   // 6
            fake_step("ready_gate"),  // 7
            fake_step("notify_done"), // 8
        ];
        // Two extra results from the Goto loop firing twice.
        let results = vec![
            fake_result("fetch"),
            fake_result("analyze"),
            fake_result("plan_gate"),
            fake_result("implement"),
            fake_result("run_tests"),
            fake_result("review"),
            // Goto loop iteration:
            fake_result("implement"),
            fake_result("run_tests"),
            fake_result("review"),
            fake_result("create_pr"),
            fake_result("ready_gate"),
        ];
        assert_eq!(results.len(), 11, "sanity: 11 results, 9 steps");
        // Buggy old logic would return 11 → loop never runs → notify_done skipped.
        // Correct: ready_gate is at index 7, next is 8 (notify_done).
        assert_eq!(next_step_index_for_resume(&steps, &results), 8);
    }

    #[test]
    fn last_result_name_not_in_workflow_returns_steps_len() {
        // Workflow was edited mid-run (a step renamed/removed). Don't
        // re-run earlier steps blindly — wrap up gracefully.
        let steps = vec![fake_step("a"), fake_step("b")];
        let results = vec![fake_result("ghost_step")];
        assert_eq!(next_step_index_for_resume(&steps, &results), 2);
    }

    #[test]
    fn request_changes_cut_common_case_equals_positional_index() {
        // No Goto: results align with steps. RequestChanges → re-run from
        // `implement` (idx 3). Cut keeps [fetch, analyze, plan_gate] = 3,
        // exactly the old positional truncate — common case unchanged.
        let results = vec![
            fake_result("fetch"),
            fake_result("analyze"),
            fake_result("plan_gate"),
            fake_result("implement"),
            fake_result("review_gate"),
        ];
        assert_eq!(request_changes_cut(&results, Some("implement"), 3), 3);
    }

    #[test]
    fn request_changes_cut_after_goto_replays_from_first_occurrence() {
        // Post-Goto: `implement` ran twice. The cut MUST be the FIRST
        // occurrence (idx 3): keeping rows 0..3 leaves `plan_gate` as the
        // last row, so the resume cursor lands on `implement`. The old
        // most-recent cut (idx 5) kept the first round's `review` in the
        // prefix and the run resumed at the GATE instead of the target
        // (run-10 live bug, 2026-06-13).
        let results = vec![
            fake_result("fetch"),
            fake_result("analyze"), // 0,1
            fake_result("plan_gate"),
            fake_result("implement"), // 2,3
            fake_result("review"),    // 4
            fake_result("implement"), // 5  ← Goto re-run
            fake_result("review"),
            fake_result("ready_gate"), // 6,7
        ];
        assert_eq!(request_changes_cut(&results, Some("implement"), 3), 3);
        // resume-cursor contract: last kept row (`plan_gate`) + 1 = target
        let steps = vec![
            fake_step("fetch"),
            fake_step("analyze"),
            fake_step("plan_gate"),
            fake_step("implement"),
            fake_step("review"),
            fake_step("ready_gate"),
        ];
        let kept = &results[..3];
        assert_eq!(
            next_step_index_for_resume(&steps, kept),
            3,
            "cursor lands ON implement"
        );
    }

    #[test]
    fn request_changes_cut_target_never_ran_falls_back_bounded() {
        // Target step exists in the workflow but never executed (e.g. a
        // Skip jumped over it). Fall back to the bounded positional index.
        let results = vec![fake_result("fetch"), fake_result("gate")];
        assert_eq!(request_changes_cut(&results, Some("never_ran"), 1), 1);
        // Fallback index past the end is clamped.
        assert_eq!(request_changes_cut(&results, Some("never_ran"), 99), 2);
    }

    // ─── inject_trigger_context ──────────────────────────────────────────

    #[test]
    fn inject_trigger_context_issue_fields() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "issue_title": "Fix the bug",
            "issue_body": "It crashes on startup",
            "issue_number": "42",
            "issue_url": "https://github.com/owner/repo/issues/42",
            "issue_labels": ["bug", "priority-high"],
        });
        inject_trigger_context(&mut ctx, &json);

        assert_eq!(ctx.render("{{issue.title}}").unwrap(), "Fix the bug");
        assert_eq!(
            ctx.render("{{issue.body}}").unwrap(),
            "It crashes on startup"
        );
        assert_eq!(ctx.render("{{issue.number}}").unwrap(), "42");
        assert_eq!(
            ctx.render("{{issue.url}}").unwrap(),
            "https://github.com/owner/repo/issues/42"
        );
        assert_eq!(
            ctx.render("{{issue.labels}}").unwrap(),
            "bug, priority-high"
        );
    }

    #[test]
    fn inject_trigger_context_issue_number_as_u64() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "issue_number": 99,
        });
        inject_trigger_context(&mut ctx, &json);
        assert_eq!(ctx.render("{{issue.number}}").unwrap(), "99");
    }

    #[test]
    fn inject_trigger_context_generic_string_fields() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "type": "tracker",
            "custom_field": "hello",
        });
        inject_trigger_context(&mut ctx, &json);
        assert_eq!(ctx.render("{{type}}").unwrap(), "tracker");
        assert_eq!(ctx.render("{{custom_field}}").unwrap(), "hello");
    }

    #[test]
    fn inject_trigger_context_non_string_values_ignored() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "count": 42,
            "nested": {"key": "val"},
        });
        inject_trigger_context(&mut ctx, &json);
        // Non-string generic fields are NOT injected
        assert_eq!(ctx.render("{{count}}").unwrap(), "{{count}}");
        assert_eq!(ctx.render("{{nested}}").unwrap(), "{{nested}}");
    }

    #[test]
    fn inject_trigger_context_empty_object() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({});
        inject_trigger_context(&mut ctx, &json);
        // No variables set — template placeholders remain
        assert_eq!(ctx.render("{{issue.title}}").unwrap(), "{{issue.title}}");
    }

    #[test]
    fn inject_trigger_context_not_an_object() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!("just a string");
        inject_trigger_context(&mut ctx, &json);
        // Should silently do nothing
        assert_eq!(ctx.render("{{anything}}").unwrap(), "{{anything}}");
    }

    #[test]
    fn inject_trigger_context_null_value() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::Value::Null;
        inject_trigger_context(&mut ctx, &json);
        assert_eq!(ctx.render("{{anything}}").unwrap(), "{{anything}}");
    }

    #[test]
    fn inject_trigger_context_empty_labels() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "issue_labels": [],
        });
        inject_trigger_context(&mut ctx, &json);
        assert_eq!(ctx.render("{{issue.labels}}").unwrap(), "");
    }

    #[test]
    fn inject_trigger_context_labels_with_non_string_items() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "issue_labels": ["bug", 42, "feature"],
        });
        inject_trigger_context(&mut ctx, &json);
        // Non-string items in labels array are filtered out
        assert_eq!(ctx.render("{{issue.labels}}").unwrap(), "bug, feature");
    }

    // ─── apply_step_snapshot ────────────────────────────────────────────
    //
    // The whole point of these snapshots is run-history honesty: editing
    // the workflow definition after a run completes must NOT corrupt the
    // run record. These tests lock the per-step-kind shape so a future
    // refactor can't silently start setting step_agent on an ApiCall row
    // (or skip step_api_endpoint_path on the ApiCall path).

    fn mk_step_for_snapshot(kind: StepType) -> WorkflowStep {
        WorkflowStep {
            exec_approved_by_trust: None,
            id: None,
            name: "s".into(),
            step_type: kind,
            description: None,
            agent: AgentType::Codex,
            prompt_template: String::new(),
            mode: StepMode::Normal,
            output_format: StepOutputFormat::FreeText,
            mcp_config_ids: vec![],
            agent_settings: None,
            on_result: vec![],
            on_timeout: None,
            stall_timeout_secs: None,
            retry: None,
            delay_after_secs: None,
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
            api_plugin_slug: Some("mcp-github".into()),
            api_config_id: Some("cfg-1".into()),
            api_endpoint_path: Some("/repos/anthropics/cookbook/issues".into()),
            api_method: None,
            api_path_params: None,
            api_query: None,
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
            quick_prompt_version: None,
        }
    }

    fn empty_result() -> StepResult {
        StepResult {
            step_name: "s".into(),
            status: RunStatus::Success,
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

    /// KT-1525 — the size of what the step left in the run is stamped on its
    /// result, in bytes (not characters), whatever the step kind.
    #[test]
    fn snapshot_stamps_the_output_size_in_bytes() {
        for kind in [StepType::Agent, StepType::ApiCall, StepType::CollectApiData] {
            let step = mk_step_for_snapshot(kind);
            let mut r = empty_result();
            r.output = "é".repeat(1000);
            apply_step_snapshot(&step, &mut r, None);
            assert_eq!(r.output_bytes, Some(2000));
        }
        let mut empty = empty_result();
        apply_step_snapshot(&mk_step_for_snapshot(StepType::Agent), &mut empty, None);
        assert_eq!(empty.output_bytes, Some(0));
    }

    #[test]
    fn snapshot_agent_step_records_agent_only() {
        let step = mk_step_for_snapshot(StepType::Agent);
        let mut r = empty_result();
        apply_step_snapshot(&step, &mut r, None);
        assert_eq!(r.step_kind.as_deref(), Some("Agent"));
        assert_eq!(r.step_agent, Some(AgentType::Codex));
        // ApiCall fields stay None for Agent steps even when the step
        // happens to carry stale api_* values (legacy edits).
        assert!(r.step_api_plugin_slug.is_none());
        assert!(r.step_api_endpoint_path.is_none());
    }

    #[test]
    fn snapshot_stamps_resolved_model_on_agent_step() {
        // 2026-06-13 — step_model must be stamped so the UI shows the real
        // model on EVERY agent step. With no explicit tier (Default), Codex
        // resolves to None (no --model) → step_model None; with a reasoning
        // tier it resolves only an explicitly configured model + tier label.
        use crate::models::{AgentSettings, ModelTier};
        let mut step = mk_step_for_snapshot(StepType::Agent);
        step.agent = AgentType::ClaudeCode;
        step.agent_settings = Some(AgentSettings {
            fallback: None,
            tools: None,
            model: None,
            tier: Some(ModelTier::Reasoning),
            reasoning_effort: None,
            max_tokens: None,
            connection_id: None,
        });
        let mut r = empty_result();
        apply_step_snapshot(&step, &mut r, None);
        assert_eq!(r.step_model.as_deref(), Some("reasoning"));
        let mut tiers = crate::models::ModelTiersConfig::default();
        tiers.claude_code.reasoning = Some("operator-reasoning".into());
        apply_step_snapshot(&step, &mut r, Some(&tiers));
        assert_eq!(
            r.step_model.as_deref(),
            Some("operator-reasoning · reasoning")
        );
        // explicit model override wins, default tier → bare model
        step.agent_settings = Some(AgentSettings {
            fallback: None,
            tools: None,
            model: Some("o3".into()),
            tier: None,
            reasoning_effort: None,
            max_tokens: None,
            connection_id: None,
        });
        let mut r2 = empty_result();
        apply_step_snapshot(&step, &mut r2, None);
        assert_eq!(r2.step_model.as_deref(), Some("o3"));
    }

    #[test]
    fn snapshot_apicall_step_records_plugin_and_endpoint_no_agent() {
        let step = mk_step_for_snapshot(StepType::ApiCall);
        let mut r = empty_result();
        apply_step_snapshot(&step, &mut r, None);
        assert_eq!(r.step_kind.as_deref(), Some("ApiCall"));
        assert!(
            r.step_agent.is_none(),
            "ApiCall has no agent — leaving step_agent null is what powers the per-step badge"
        );
        assert_eq!(r.step_api_plugin_slug.as_deref(), Some("mcp-github"));
        assert_eq!(
            r.step_api_endpoint_path.as_deref(),
            Some("/repos/anthropics/cookbook/issues")
        );
    }

    #[test]
    fn snapshot_notify_step_records_notify_kind_no_agent_no_plugin() {
        let step = mk_step_for_snapshot(StepType::Notify);
        let mut r = empty_result();
        apply_step_snapshot(&step, &mut r, None);
        assert_eq!(r.step_kind.as_deref(), Some("Notify"));
        assert!(r.step_agent.is_none());
        assert!(r.step_api_plugin_slug.is_none());
    }

    #[test]
    fn snapshot_batch_step_records_batch_kind_no_agent_no_plugin() {
        let step = mk_step_for_snapshot(StepType::BatchQuickPrompt);
        let mut r = empty_result();
        apply_step_snapshot(&step, &mut r, None);
        assert_eq!(r.step_kind.as_deref(), Some("BatchQuickPrompt"));
        assert!(r.step_agent.is_none());
        assert!(r.step_api_plugin_slug.is_none());
    }

    #[test]
    fn snapshot_gate_step_records_gate_kind_no_agent_no_plugin() {
        let step = mk_step_for_snapshot(StepType::Gate);
        let mut r = empty_result();
        apply_step_snapshot(&step, &mut r, None);
        assert_eq!(r.step_kind.as_deref(), Some("Gate"));
        assert!(
            r.step_agent.is_none(),
            "Gate has no agent — the badge should render as a 'pause' chip with no agent name"
        );
        assert!(r.step_api_plugin_slug.is_none());
    }

    // ─── append_decision_footer (Phase 4 — gate decision) ───────────────

    fn gate_result_with(output: &str) -> StepResult {
        StepResult {
            step_name: "gate".into(),
            status: RunStatus::WaitingApproval,
            output: output.into(),
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

    #[test]
    fn footer_appends_with_separator_and_verdict_no_comment() {
        let mut r = gate_result_with("Validate the PR?");
        append_decision_footer(&mut r, "Approuvé", None);
        assert!(r.output.starts_with("Validate the PR?"));
        assert!(r.output.contains("> **Décision : Approuvé**"));
        assert!(
            r.output.contains("\n\n"),
            "should keep gate body and footer separated by blank line"
        );
    }

    #[test]
    fn footer_appends_with_comment() {
        let mut r = gate_result_with("Validate?");
        append_decision_footer(&mut r, "Changements demandés", Some("Add unit tests"));
        assert!(r.output.contains("> **Décision : Changements demandés**"));
        assert!(r.output.contains("> Add unit tests"));
    }

    #[test]
    fn footer_handles_multiline_comment_with_blockquote_continuation() {
        // Multi-line comments are blockquoted line-by-line so the
        // run-detail markdown renders the comment as a single
        // visually-grouped quote rather than a quote followed by raw
        // text. UX preserves operator's line-breaks.
        let mut r = gate_result_with("Gate body");
        append_decision_footer(&mut r, "Rejeté", Some("Line1\nLine2"));
        assert!(r.output.contains("> Line1\n> Line2"));
    }

    #[test]
    fn footer_skips_separator_when_body_is_empty() {
        // If the gate had an empty rendered message, don't prepend a
        // pair of blank lines (would render as a leading whitespace
        // block in the UI).
        let mut r = gate_result_with("");
        append_decision_footer(&mut r, "Approuvé", None);
        assert!(!r.output.starts_with("\n"), "got: {:?}", r.output);
        assert!(r.output.contains("Approuvé"));
    }

    #[test]
    fn footer_treats_blank_comment_as_no_comment() {
        // Whitespace-only comment from the UI should fall back to
        // verdict-only — preventing "> " stray-prefix lines.
        let mut r = gate_result_with("Body");
        append_decision_footer(&mut r, "Approuvé", Some("   \n\t  "));
        assert!(r.output.contains("> **Décision : Approuvé**"));
        // No bare "> " line beneath the verdict.
        let lines: Vec<&str> = r.output.lines().collect();
        let last = lines.last().copied().unwrap_or("");
        assert!(
            last.contains("Approuvé**"),
            "last line should be the verdict line, got: {:?}",
            last
        );
    }

    // ─── max_iterations_for ──────────────────────────────────────────────

    #[test]
    fn max_iterations_zero_steps() {
        assert_eq!(max_iterations_for(0), 50);
    }

    #[test]
    fn max_iterations_typical_workflow() {
        assert_eq!(max_iterations_for(3), 80); // 3*10 + 50
        assert_eq!(max_iterations_for(5), 100); // 5*10 + 50
    }

    #[test]
    fn max_iterations_single_step() {
        assert_eq!(max_iterations_for(1), 60); // 1*10 + 50
    }

    // ─── RunEvent serialization ──────────────────────────────────────────

    #[test]
    fn run_event_step_start_serializes() {
        let evt = RunEvent::StepStart {
            step_name: "analyze".into(),
            step_index: 0,
            total_steps: 3,
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["event"], "StepStart");
        assert_eq!(json["data"]["step_name"], "analyze");
        assert_eq!(json["data"]["step_index"], 0);
        assert_eq!(json["data"]["total_steps"], 3);
    }

    #[test]
    fn run_event_run_done_serializes() {
        let evt = RunEvent::RunDone {
            status: RunStatus::Success,
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["event"], "RunDone");
    }

    #[test]
    fn run_event_run_error_serializes() {
        let evt = RunEvent::RunError {
            error: "timeout".into(),
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["event"], "RunError");
        assert_eq!(json["data"]["error"], "timeout");
    }

    // ─── WorkflowGuards (Phase 1 — 0.7.0) ────────────────────────────────

    fn successful_step_outcome() -> StepOutcome {
        StepOutcome {
            result: empty_result(),
            condition_action: None,
        }
    }

    #[test]
    fn document_audit_failure_cannot_continue_via_agent_conditions() {
        let mut outcome = successful_step_outcome();
        outcome.result.output = "original agent result".into();
        outcome.condition_action = Some(ConditionAction::Skip);
        outcome.result.condition_result = Some("Skip".into());
        fail_document_audit(&mut outcome, "docs/new report.md: possible credential");
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(outcome.condition_action.is_none());
        assert!(outcome.result.condition_result.is_none());
        let persisted = serde_json::to_value(&outcome.result).unwrap();
        assert!(persisted["output"]
            .as_str()
            .unwrap()
            .contains("docs/new report.md"));
        assert!(persisted["output"]
            .as_str()
            .unwrap()
            .contains("original agent result"));
    }

    #[tokio::test]
    async fn in_flight_step_stops_when_absolute_deadline_is_reached() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = await_in_flight_step(
            std::future::pending(),
            &cancel,
            tokio::time::Instant::now(),
            || async {},
        )
        .await;
        assert!(matches!(result, InFlightStepOutcome::TimedOut));
    }

    #[tokio::test]
    async fn explicit_cancel_wins_when_cancel_and_deadline_are_both_ready() {
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let result = await_in_flight_step(
            std::future::pending(),
            &cancel,
            tokio::time::Instant::now(),
            || async {},
        )
        .await;
        assert!(matches!(result, InFlightStepOutcome::Cancelled));
    }

    #[tokio::test]
    async fn completed_step_wins_over_deadline_in_the_same_scheduler_turn() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = await_in_flight_step(
            async { successful_step_outcome() },
            &cancel,
            tokio::time::Instant::now(),
            || async {},
        )
        .await;
        assert!(matches!(result, InFlightStepOutcome::Completed(_)));
    }

    #[test]
    fn active_timeout_deadline_is_immediate_after_the_budget_is_spent() {
        let mut run = pending_run("run-timeout-clock", "wf-timeout-clock");
        run.state.insert(
            TIMEOUT_ACTIVE_MS_STATE_KEY.to_string(),
            61_000_u64.to_string(),
        );
        let clock = WorkflowTimeoutClock::resume(&run);
        assert!(clock.deadline(60) <= tokio::time::Instant::now());
    }

    #[test]
    fn workflow_guards_default_resolves_to_backend_constants() {
        let g = WorkflowGuards::default().resolved();
        assert_eq!(g.timeout_seconds, DEFAULT_GUARD_TIMEOUT_SECS);
        assert_eq!(g.max_llm_calls, DEFAULT_GUARD_MAX_LLM_CALLS);
        assert_eq!(
            g.loop_detection_max_revisits,
            DEFAULT_GUARD_LOOP_MAX_REVISITS
        );
    }

    #[test]
    fn workflow_guards_partial_override_uses_defaults_for_unset() {
        // User sets only timeout; the other two should fall back to defaults.
        let g = WorkflowGuards {
            timeout_seconds: Some(60),
            max_llm_calls: None,
            loop_detection_max_revisits: None,
        };
        let r = g.resolved();
        assert_eq!(r.timeout_seconds, 60);
        assert_eq!(r.max_llm_calls, DEFAULT_GUARD_MAX_LLM_CALLS);
        assert_eq!(
            r.loop_detection_max_revisits,
            DEFAULT_GUARD_LOOP_MAX_REVISITS
        );
    }

    #[test]
    fn workflow_guards_resolve_optional_none_yields_defaults() {
        let r = WorkflowGuards::resolve_optional(None);
        assert_eq!(r.timeout_seconds, DEFAULT_GUARD_TIMEOUT_SECS);
        assert_eq!(r.max_llm_calls, DEFAULT_GUARD_MAX_LLM_CALLS);
        assert_eq!(
            r.loop_detection_max_revisits,
            DEFAULT_GUARD_LOOP_MAX_REVISITS
        );
    }

    #[test]
    fn workflow_guards_full_override() {
        let g = WorkflowGuards {
            timeout_seconds: Some(120),
            max_llm_calls: Some(5),
            loop_detection_max_revisits: Some(3),
        };
        let r = g.resolved();
        assert_eq!(r.timeout_seconds, 120);
        assert_eq!(r.max_llm_calls, 5);
        assert_eq!(r.loop_detection_max_revisits, 3);
    }

    #[test]
    fn run_status_stopped_by_guard_round_trips_through_db() {
        // Ensure parse + serialize for the new variant doesn't drop data.
        // Mirror test for the matching `parse_run_status` / `run_status_str`
        // pair in `db/workflows.rs`.
        let status = RunStatus::StoppedByGuard;
        let json = serde_json::to_string(&status).unwrap();
        let back: RunStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, RunStatus::StoppedByGuard);
    }

    #[test]
    fn guard_kind_serializes_with_step_name_for_loop_detection() {
        // The `LoopDetection` variant carries which step looped — the
        // frontend uses this to render "step 'self_review' visited 11x"
        // instead of a generic "loop detected".
        let k = GuardKind::LoopDetection {
            step_name: "self_review".into(),
        };
        let json = serde_json::to_value(&k).unwrap();
        assert_eq!(json["type"], "LoopDetection");
        assert_eq!(json["step_name"], "self_review");
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

    #[test]
    fn persist_writes_declared_artifacts_to_workspace() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut artifacts = ::std::collections::HashMap::new();
        artifacts.insert(
            "plan".to_string(),
            ArtifactSpec {
                path: ".kronn/plan.md".into(),
                format: Some("markdown".into()),
            },
        );
        let wf = make_workflow_with_artifacts(artifacts);

        let mut extracted = ::std::collections::HashMap::new();
        extracted.insert("plan".to_string(), "# Plan body".to_string());

        persist_declared_artifacts(&wf, &extracted, dir.path());

        let written = dir.path().join(".kronn/plan.md");
        assert!(written.exists(), "artifact must be written");
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "# Plan body");
    }

    #[test]
    fn persist_skips_undeclared_artifacts() {
        let dir = tempfile::TempDir::new().unwrap();
        let wf = make_workflow_with_artifacts(::std::collections::HashMap::new());

        let mut extracted = ::std::collections::HashMap::new();
        extracted.insert("rogue".to_string(), "should not land".to_string());

        persist_declared_artifacts(&wf, &extracted, dir.path());

        let walked: Vec<_> = walkdir::WalkDir::new(dir.path())
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .collect();
        assert!(
            walked.is_empty(),
            "no file should be written for undeclared artifacts (got {} files)",
            walked.len()
        );
    }

    #[test]
    fn persist_creates_parent_directories_on_demand() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut artifacts = ::std::collections::HashMap::new();
        artifacts.insert(
            "trace".to_string(),
            ArtifactSpec {
                path: ".kronn/deep/nested/trace.yaml".into(),
                format: None,
            },
        );
        let wf = make_workflow_with_artifacts(artifacts);

        let mut extracted = ::std::collections::HashMap::new();
        extracted.insert("trace".to_string(), "step: ok".to_string());

        persist_declared_artifacts(&wf, &extracted, dir.path());

        let written = dir.path().join(".kronn/deep/nested/trace.yaml");
        assert!(written.exists(), "deep parent dirs must be created");
    }

    #[test]
    fn persist_overwrites_existing_artifact_on_re_emit() {
        // Auto-Dev's review→implement→review loop emits the same
        // `review` artifact on every iteration. The latest write wins.
        let dir = tempfile::TempDir::new().unwrap();
        let mut artifacts = ::std::collections::HashMap::new();
        artifacts.insert(
            "review".to_string(),
            ArtifactSpec {
                path: ".kronn/review.yaml".into(),
                format: None,
            },
        );
        let wf = make_workflow_with_artifacts(artifacts);

        let mut e1 = ::std::collections::HashMap::new();
        e1.insert("review".to_string(), "v1".to_string());
        persist_declared_artifacts(&wf, &e1, dir.path());

        let mut e2 = ::std::collections::HashMap::new();
        e2.insert("review".to_string(), "v2 with more".to_string());
        persist_declared_artifacts(&wf, &e2, dir.path());

        let written = dir.path().join(".kronn/review.yaml");
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "v2 with more");
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

    #[tokio::test]
    async fn execute_run_drives_deterministic_steps_to_success_and_persists() {
        let (state, tokens, agents) = test_state_and_configs();

        // Two JsonData steps — exercises the multi-step loop, not just a
        // single dispatch. No project → no worktree, no agent, no network.
        let mut wf = make_workflow_with_artifacts(::std::collections::HashMap::new());
        wf.id = "wf-det".into();
        wf.steps = vec![
            json_data_step("emit_first", serde_json::json!({ "phase": "one" })),
            json_data_step("emit_second", serde_json::json!({ "phase": "two", "n": 2 })),
        ];

        let mut run = pending_run("run-det", "wf-det");

        // Persist workflow + run so the runner's DB write-backs have rows.
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
        .expect("a deterministic run must not error");

        // In-memory run object reflects success.
        assert_eq!(run.status, RunStatus::Success, "the whole run succeeds");
        assert_eq!(run.step_results.len(), 2, "both steps executed");
        assert!(
            run.step_results
                .iter()
                .all(|r| r.status == RunStatus::Success),
            "every step succeeded",
        );
        assert_eq!(run.step_results[0].step_name, "emit_first");
        assert_eq!(run.step_results[1].step_name, "emit_second");
        // The JsonData payload is surfaced in the step output envelope.
        assert!(
            run.step_results[1].output.contains("\"phase\""),
            "step output carries the literal payload: {}",
            run.step_results[1].output,
        );

        // And the orchestrator persisted the terminal state to the DB — the
        // write-back path that the pure helpers never touch.
        let persisted = state
            .db
            .with_conn(move |conn| crate::db::workflows::get_run(conn, "run-det"))
            .await
            .unwrap()
            .expect("run row exists");
        assert_eq!(
            persisted.status,
            RunStatus::Success,
            "DB row reflects success"
        );
        assert_eq!(
            persisted.step_results.len(),
            2,
            "DB persisted both step results"
        );
    }

    #[tokio::test]
    async fn execute_run_requires_current_hash_approval_after_imported_alignment_refresh() {
        let (state, tokens, agents) = test_state_and_configs();
        let mut workflow = make_workflow_with_artifacts(::std::collections::HashMap::new());
        workflow.id = "wf-imported".into();
        workflow.steps = vec![json_data_step(
            "emit",
            serde_json::json!({ "approved": true }),
        )];
        let mut run = pending_run("run-imported", &workflow.id);
        let workflow_for_db = workflow.clone();
        let run_for_db = run.clone();

        state
            .db
            .with_conn(move |conn| {
                crate::db::workflows::insert_workflow(conn, &workflow_for_db)?;
                crate::db::workflows::insert_run(conn, &run_for_db)?;
                crate::db::repository_resources::upsert_alignment(
                    conn,
                    "repo",
                    "workflow",
                    "imported",
                    "wf-imported",
                    "repository-imported",
                    "database-imported",
                    "2026-09-28T10:00:00Z",
                    true,
                )?;
                crate::db::repository_resources::upsert_alignment(
                    conn,
                    "repo",
                    "workflow",
                    "imported",
                    "wf-imported",
                    "repository-published",
                    "database-published",
                    "2026-09-28T11:00:00Z",
                    false,
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let blocked = execute_run(
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
        .unwrap_err();
        assert!(blocked.to_string().contains("must be approved"));

        let rendered = crate::core::repository_resources::render_workflow(&workflow, "imported")
            .expect("render imported workflow");
        state
            .db
            .with_conn(move |conn| {
                crate::db::repository_resources::approve(
                    conn,
                    "repo",
                    "workflow",
                    "imported",
                    &crate::core::repository_resources::approval_hash(&rendered.document),
                )
            })
            .await
            .unwrap();

        execute_run(
            state, &workflow, &mut run, &tokens, &agents, None, None, None,
        )
        .await
        .expect("the approved workflow fingerprint must execute");
        assert_eq!(run.status, RunStatus::Success);
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

    fn step_window(run: &WorkflowRun) -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
        let step = &run.step_results[0];
        let start = step.started_at.expect("step start recorded");
        (
            start,
            start + chrono::Duration::milliseconds(step.duration_ms as i64),
        )
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod preflight_validation_tests {
    use crate::models::AgentType;

    /// Mirror the discriminant-comparison logic from execute_run's preflight
    /// block — keeps the contract testable without spinning up `detect_all`
    /// (which hits the host filesystem and depends on which agent binaries
    /// are actually on PATH).
    fn missing_step_agents(
        step_agents: &[(&str, AgentType)],
        usable: &[AgentType],
    ) -> Vec<(String, AgentType)> {
        step_agents
            .iter()
            .filter(|(_, a)| {
                !usable
                    .iter()
                    .any(|u| std::mem::discriminant(u) == std::mem::discriminant(a))
            })
            .map(|(n, a)| (n.to_string(), a.clone()))
            .collect()
    }

    #[test]
    fn empty_workflow_yields_no_missing() {
        assert!(missing_step_agents(&[], &[AgentType::ClaudeCode]).is_empty());
    }

    #[test]
    fn all_agents_usable_yields_no_missing() {
        let steps = vec![("plan", AgentType::ClaudeCode), ("test", AgentType::Codex)];
        let usable = vec![AgentType::ClaudeCode, AgentType::Codex];
        assert!(missing_step_agents(&steps, &usable).is_empty());
    }

    #[test]
    fn cross_agent_workflow_with_one_uninstalled_step_is_flagged() {
        // Real-world case: workflow uses ClaudeCode for planning + Vibe for
        // a cheap summarisation step. User uninstalls Vibe between editing
        // the workflow and clicking Run. Pre-fix the run would fail
        // mid-execution at the spawn site with "vibe: command not found".
        let steps = vec![
            ("plan", AgentType::ClaudeCode),
            ("summarise", AgentType::Vibe),
            ("review", AgentType::ClaudeCode),
        ];
        let usable = vec![AgentType::ClaudeCode]; // Vibe uninstalled
        let missing = missing_step_agents(&steps, &usable);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].0, "summarise");
        assert!(matches!(missing[0].1, AgentType::Vibe));
    }

    #[test]
    fn empty_usable_flags_every_step() {
        let steps = vec![
            ("a", AgentType::ClaudeCode),
            ("b", AgentType::Codex),
            ("c", AgentType::Vibe),
        ];
        let missing = missing_step_agents(&steps, &[]);
        assert_eq!(missing.len(), 3);
    }
}
