//! Executor for `StepType::DelegateSubtasks` (KT-909).
//!
//! The step is a mechanical principal for a campaign: it launches a parent
//! task's subtasks through the task-execution saga, reads their durable state
//! between two pauses, and calls an agent only to review a delivery. Every
//! decision goes through the same review, escalation and integration code as an
//! agent principal's; the step adds no state of its own beyond a technical
//! discussion that owns the campaign and keeps each reviewer verdict.

use std::time::{Duration, Instant};

use anyhow::Context;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use crate::api::orchestration::{
    decide_review_as_delegation_step, provision_campaign_task_execution, run_integration,
    CampaignProvisionInput, IntegrationOutcome, ReviewOutcome,
};
use crate::core::worktree;
use crate::db::Database;
use crate::models::{
    AgentType, CampaignWorkerSelection, ConditionAction, DelegateSubtasksConfig, DelegateWorker,
    Discussion, DiscussionMessage, MessageChannel, MessageRole, MessageTarget,
    OrchestrationControlState, OrchestrationRun, OrchestrationRunKind, PlanningActor,
    PlanningActorKind, PlanningTaskStatus, QuotaWait, RunStatus, StepResult, SummaryStrategy,
    TaskExecution, TaskExecutionStatus, WorkflowStep,
};
use crate::AppState;

use super::runner::SharedBudget;
use super::steps::StepOutcome;
use super::template::TemplateContext;

/// Ids of the technical discussions this step owns. Discussion ids are
/// server-generated UUIDs, so the prefix cannot collide with a human room.
const DISCUSSION_PREFIX: &str = "wf-delegate-";
const DEFAULT_CONCURRENCY: u32 = 1;
pub(crate) const MAX_CONCURRENCY: u32 = 8;
const DEFAULT_REVIEW_ROUNDS: u32 = 3;
pub(crate) const MAX_REVIEW_ROUNDS: u32 = 10;
const DEFAULT_TIMEOUT_SECS: u64 = 6 * 3600;
/// Between two reads of the durable state when nothing moved. An SQL read,
/// never a model call.
const IDLE_POLL: Duration = Duration::from_secs(2);
const DIFF_MAX_CHARS: usize = 40_000;
/// A subtask whose approved delivery conflicts this many times stops the step.
const CONFLICT_LIMIT: i64 = 2;

pub(crate) fn is_delegation_discussion(discussion_id: &str) -> bool {
    discussion_id.starts_with(DISCUSSION_PREFIX)
}

/// Why the step stopped; the name is the `[SIGNAL: …]` `on_result` routes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DelegationSignal {
    Ok,
    Escalated,
    Conflict,
    Blocked,
    Failed,
    Timeout,
}

impl DelegationSignal {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Escalated => "ESCALATED",
            Self::Conflict => "CONFLICT",
            Self::Blocked => "BLOCKED",
            Self::Failed => "FAILED",
            Self::Timeout => "TIMEOUT",
        }
    }
}

/// What the reviewer receives: a fresh session's whole context.
#[derive(Debug, Clone)]
pub(crate) struct ReviewRequest {
    pub execution_id: String,
    pub attempt_no: u32,
    pub task_reference: String,
    pub prompt: String,
    /// The delivered worktree, so the reviewer can read files the diff cuts.
    pub work_dir: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ReviewReply {
    pub text: String,
    pub tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub model: Option<String>,
    /// The reviewer could not run; the step stops rather than guess a verdict.
    pub failure: Option<String>,
    /// KT-811 — the provider refused for a quota; the run waits for the reset.
    pub quota_wait: Option<QuotaWait>,
}

/// The outside world of the delegation loop: the reviewer, and the pause
/// between two reads of the durable state. Tests drive fake workers from
/// `idle`, so every wait there is synchronous with the step.
#[async_trait::async_trait]
pub(crate) trait DelegationWorld: Send + Sync {
    async fn review(&self, request: ReviewRequest) -> ReviewReply;
    async fn idle(&self);
    fn now(&self) -> Instant {
        Instant::now()
    }
    /// Resolves once `deadline` is reached; bounds a review in flight.
    async fn wait_until(&self, deadline: Instant) {
        tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
    }
}

/// One delegation, resolved from the step and the run.
pub(crate) struct Delegation<'a> {
    pub state: &'a AppState,
    pub run_id: &'a str,
    pub step_name: &'a str,
    pub workflow_project_id: Option<&'a str>,
    pub reviewer_agent: AgentType,
    pub config: &'a DelegateSubtasksConfig,
    pub parent_task: String,
    pub target_branch: Option<String>,
    /// The run's working directory; its branch is the default target.
    pub work_dir: &'a str,
    pub reviewer_guidance: &'a str,
    pub budget: Option<SharedBudget>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct SubtaskReport {
    pub task: String,
    pub title: String,
    pub status: String,
    pub execution_id: Option<String>,
    pub integrated_sha: Option<String>,
    pub review_rounds: u32,
    pub attempts: u32,
    pub integration_conflicts: u32,
    pub worker: Option<String>,
    pub cost_usd: Option<f64>,
    pub tokens: u64,
}

#[derive(Debug)]
pub(crate) struct DelegationReport {
    pub signal: DelegationSignal,
    pub summary: String,
    pub discussion_id: Option<String>,
    pub campaign_id: Option<String>,
    pub target_branch: Option<String>,
    pub subtasks: Vec<SubtaskReport>,
    /// Model calls this invocation made: reviewer sessions only.
    pub review_calls: u32,
    pub review_tokens: u64,
    pub quota_wait: Option<QuotaWait>,
    /// Ends the run for good: the LLM-call count could not be recorded.
    pub terminal_stop: Option<String>,
}

struct Subtask {
    id: String,
    reference: String,
    title: String,
    worker: Option<CampaignWorkerSelection>,
}

struct Setup {
    discussion_id: String,
    campaign_id: String,
    target_branch: String,
    subtasks: Vec<Subtask>,
}

struct Stop {
    signal: DelegationSignal,
    reason: String,
}

impl Stop {
    fn new(signal: DelegationSignal, reason: impl Into<String>) -> Self {
        Self {
            signal,
            reason: reason.into(),
        }
    }
}

fn step_actor(run_id: &str, step_name: &str) -> PlanningActor {
    PlanningActor {
        kind: PlanningActorKind::Backend,
        id: Some(format!(
            "workflow-step:{}:{step_name}",
            run_id.chars().take(8).collect::<String>()
        )),
        session_id: None,
        source_message_id: None,
    }
}

fn worker_selection(worker: &DelegateWorker) -> CampaignWorkerSelection {
    let target = if crate::agents::runner::is_http_chat_agent(&worker.agent) {
        MessageTarget::discussion_agent(worker.agent.clone())
    } else {
        MessageTarget::agent(worker.agent.clone())
    };
    CampaignWorkerSelection {
        target: match worker.tier {
            Some(tier) => target.with_tier(tier),
            None => target,
        },
        model: worker.model.clone(),
        profile_id: None,
    }
}

/// The first `worker:<key>` tag the map knows, else the default worker.
fn resolve_worker(config: &DelegateSubtasksConfig, tags: &[String]) -> Option<DelegateWorker> {
    tags.iter()
        .filter_map(|tag| tag.strip_prefix("worker:"))
        .find_map(|key| config.worker_map.get(key.trim()).cloned())
        .or_else(|| config.default_worker.clone())
}

fn review_message_id(execution_id: &str, attempt_no: u32) -> String {
    format!("wf-review:{execution_id}:{attempt_no}")
}

/// Keyed by the reviewed attempt: a successful reassignment advances the
/// attempt, so a marker for the current one means it is still pending.
fn reassign_marker_id(execution_id: &str, attempt_no: u32) -> String {
    format!("wf-reassign:{execution_id}:{attempt_no}")
}

pub(crate) async fn run_delegation(
    delegation: &Delegation<'_>,
    world: &dyn DelegationWorld,
) -> DelegationReport {
    let started = world.now();
    let timeout = Duration::from_secs(
        delegation
            .config
            .timeout_secs
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .max(1),
    );
    let db = delegation.state.db.clone();
    let mut report = DelegationReport {
        signal: DelegationSignal::Failed,
        summary: String::new(),
        discussion_id: None,
        campaign_id: None,
        target_branch: None,
        subtasks: Vec::new(),
        review_calls: 0,
        review_tokens: 0,
        quota_wait: None,
        terminal_stop: None,
    };
    let setup = match prepare(delegation).await {
        Ok(setup) => setup,
        Err(stop) => {
            report.signal = stop.signal;
            report.summary = stop.reason;
            return report;
        }
    };
    report.discussion_id = Some(setup.discussion_id.clone());
    report.campaign_id = Some(setup.campaign_id.clone());
    report.target_branch = Some(setup.target_branch.clone());

    let deadline = Deadline {
        at: started + timeout,
        timeout,
    };
    let stop = loop {
        if let Some(stop) = past_deadline(world, deadline) {
            break stop;
        }
        match advance(delegation, &setup, world, &mut report, deadline).await {
            Ok(Progress::Moved) => continue,
            Ok(Progress::Idle) => world.idle().await,
            Ok(Progress::Stopped(stop)) => break stop,
            Err(error) => break Stop::new(DelegationSignal::Failed, format!("{error:#}")),
        }
    };
    report.signal = stop.signal;
    report.summary = stop.reason;
    report.subtasks = subtask_reports(&db, &setup).await.unwrap_or_default();
    report
}

#[derive(Clone, Copy)]
struct Deadline {
    at: Instant,
    timeout: Duration,
}

/// TIMEOUT once the step's absolute deadline is reached. Checked before every
/// new decision or integration; what is already durable stays resumable.
fn past_deadline(world: &dyn DelegationWorld, deadline: Deadline) -> Option<Stop> {
    let Deadline { at, timeout } = deadline;
    (world.now() >= at).then(|| {
        Stop::new(
            DelegationSignal::Timeout,
            format!(
                "step timeout ({} s) reached; executions keep running and a resumed step picks them up",
                timeout.as_secs()
            ),
        )
    })
}

enum Progress {
    Moved,
    Idle,
    Stopped(Stop),
}

async fn prepare(delegation: &Delegation<'_>) -> Result<Setup, Stop> {
    let config = delegation.config;
    let parent_ref = delegation.parent_task.trim().to_string();
    if parent_ref.is_empty() {
        return Err(Stop::new(
            DelegationSignal::Failed,
            "`delegate_subtasks.parent_task` rendered empty",
        ));
    }
    let db = delegation.state.db.clone();
    let lookup = parent_ref.clone();
    let tasks = db
        .with_read_conn(move |conn| {
            let Some(parent) = crate::db::planning::get_task(conn, &lookup)? else {
                return Ok(None);
            };
            let mut children = Vec::new();
            for child in &parent.subtasks {
                if let Some(detail) = crate::db::planning::get_task(conn, &child.id)? {
                    children.push(detail);
                }
            }
            Ok(Some((parent, children)))
        })
        .await
        .map_err(|error| Stop::new(DelegationSignal::Failed, error.to_string()))?;
    let Some((parent, mut children)) = tasks else {
        return Err(Stop::new(
            DelegationSignal::Failed,
            format!("parent task `{parent_ref}` not found"),
        ));
    };
    if children.is_empty() {
        return Err(Stop::new(
            DelegationSignal::Failed,
            format!("{} has no subtask to delegate", parent.summary.reference),
        ));
    }
    children.sort_by(|a, b| {
        (a.summary.rank, &a.summary.reference).cmp(&(b.summary.rank, &b.summary.reference))
    });

    // Every open subtask needs a worker and one shared project before anything launches.
    let mut unresolved = Vec::new();
    let mut projects = std::collections::BTreeSet::new();
    let mut subtasks = Vec::new();
    for child in &children {
        let open = !matches!(
            child.summary.status,
            PlanningTaskStatus::Done | PlanningTaskStatus::Archived
        );
        let worker = if open {
            match resolve_worker(config, &child.summary.tags) {
                Some(worker) => Some(worker_selection(&worker)),
                None => {
                    unresolved.push(child.summary.reference.clone());
                    None
                }
            }
        } else {
            None
        };
        if open {
            match child.summary.project_ids.as_slice() {
                [project] => {
                    projects.insert(project.clone());
                }
                _ => {
                    return Err(Stop::new(
                        DelegationSignal::Failed,
                        format!(
                            "{} must belong to exactly one project",
                            child.summary.reference
                        ),
                    ))
                }
            }
        }
        subtasks.push(Subtask {
            id: child.summary.id.clone(),
            reference: child.summary.reference.clone(),
            title: child.summary.title.clone(),
            worker,
        });
    }
    if !unresolved.is_empty() {
        return Err(Stop::new(
            DelegationSignal::Failed,
            format!(
                "no worker for {}: tag them `worker:<key>` with a key of `worker_map`, or set `default_worker`",
                unresolved.join(", ")
            ),
        ));
    }
    let project_id = match (projects.len(), delegation.workflow_project_id) {
        (0, _) => parent
            .summary
            .project_ids
            .first()
            .cloned()
            .or_else(|| delegation.workflow_project_id.map(str::to_string)),
        (1, workflow_project) => {
            let project = projects.into_iter().next().expect("one project");
            if workflow_project.is_some_and(|workflow_project| workflow_project != project) {
                return Err(Stop::new(
                    DelegationSignal::Failed,
                    "the subtasks belong to another project than the workflow",
                ));
            }
            Some(project)
        }
        _ => {
            return Err(Stop::new(
                DelegationSignal::Failed,
                "the open subtasks span several projects",
            ))
        }
    };
    let Some(project_id) = project_id else {
        return Err(Stop::new(
            DelegationSignal::Failed,
            "the subtasks have no project",
        ));
    };
    let pid = project_id.clone();
    let project_path = db
        .with_read_conn(move |conn| {
            Ok(crate::db::projects::get_project(conn, &pid)?.map(|p| p.path))
        })
        .await
        .map_err(|error| Stop::new(DelegationSignal::Failed, error.to_string()))?
        .ok_or_else(|| Stop::new(DelegationSignal::Failed, "the subtasks' project vanished"))?;
    let repo_path = crate::core::scanner::resolve_host_path(&project_path);

    let target_branch = match delegation
        .target_branch
        .as_deref()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
    {
        Some(branch) => branch.to_string(),
        None => {
            let work_dir = if delegation.work_dir.trim().is_empty() {
                repo_path.clone()
            } else {
                std::path::PathBuf::from(delegation.work_dir)
            };
            let state = worktree::main_repo_state(&work_dir)
                .map_err(|error| Stop::new(DelegationSignal::Failed, error))?;
            if state.is_detached || state.current_branch.is_empty() {
                return Err(Stop::new(
                    DelegationSignal::Failed,
                    "the run's working directory is on a detached HEAD; set `target_branch`",
                ));
            }
            state.current_branch
        }
    };
    let target_branch = worktree::resolve_local_branch(&repo_path, &target_branch)
        .map_err(|error| Stop::new(DelegationSignal::Failed, error))?;

    let discussion_id = owner_discussion(delegation, &project_id, &parent.summary.reference)
        .await
        .map_err(|error| Stop::new(DelegationSignal::Failed, format!("{error:#}")))?;
    let campaign = owner_campaign(
        delegation,
        &discussion_id,
        &project_id,
        &target_branch,
        &subtasks,
    )
    .await
    .map_err(|error| Stop::new(DelegationSignal::Failed, format!("{error:#}")))?;

    // The saga would hold every approval on a dirty target: refuse before launching.
    let all_done = children.iter().all(|child| {
        matches!(
            child.summary.status,
            PlanningTaskStatus::Done | PlanningTaskStatus::Archived
        )
    });
    if !all_done {
        let checkout = worktree::integration_target_worktree(&repo_path, &target_branch)
            .map_err(|error| Stop::new(DelegationSignal::Blocked, error))?;
        let dirty = worktree::worktree_dirty_files(&checkout)
            .map_err(|error| Stop::new(DelegationSignal::Blocked, error))?;
        if !dirty.is_empty() {
            return Err(Stop::new(
                DelegationSignal::Blocked,
                format!(
                    "`{target_branch}` has {} uncommitted file(s) in {}; commit them before delegating",
                    dirty.len(),
                    checkout.display()
                ),
            ));
        }
    }
    Ok(Setup {
        discussion_id,
        campaign_id: campaign.id,
        target_branch,
        subtasks,
    })
}

/// The run's technical room for this step, created once and found again on resume.
async fn owner_discussion(
    delegation: &Delegation<'_>,
    project_id: &str,
    parent_reference: &str,
) -> anyhow::Result<String> {
    let title = format!(
        "{parent_reference} · délégation « {} » · run {}",
        delegation.step_name,
        delegation.run_id.chars().take(8).collect::<String>()
    );
    let run_id = delegation.run_id.to_string();
    let lookup_title = title.clone();
    let existing = delegation
        .state
        .db
        .with_read_conn(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT id FROM discussions WHERE workflow_run_id = ?1 AND title = ?2 \
                     AND id LIKE 'wf-delegate-%' LIMIT 1",
                    rusqlite::params![run_id, lookup_title],
                    |row| row.get::<_, String>(0),
                )
                .optional()?)
        })
        .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let now = chrono::Utc::now();
    let discussion = Discussion {
        connection_id: None,
        awaiting_agent: false,
        agent_running: false,
        id: format!("{DISCUSSION_PREFIX}{}", uuid::Uuid::new_v4()),
        project_id: Some(project_id.to_string()),
        title,
        agent: delegation.reviewer_agent.clone(),
        language: "fr".into(),
        participants: vec![delegation.reviewer_agent.clone()],
        messages: vec![],
        message_count: 0,
        non_system_message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        tier: Default::default(),
        model: None,
        pin_first_message: false,
        archived: false,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: SummaryStrategy::default(),
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: Some(delegation.run_id.to_string()),
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };
    let id = discussion.id.clone();
    delegation
        .state
        .db
        .with_conn(move |conn| crate::db::discussions::insert_discussion(conn, &discussion))
        .await?;
    Ok(id)
}

/// The campaign the room owns, created once; the subtasks join its plan in order.
async fn owner_campaign(
    delegation: &Delegation<'_>,
    discussion_id: &str,
    project_id: &str,
    target_branch: &str,
    subtasks: &[Subtask],
) -> anyhow::Result<OrchestrationRun> {
    let config = delegation.config;
    let mut input = crate::models::OrchestrationRunInput::single_task(discussion_id);
    input.kind = OrchestrationRunKind::Campaign;
    input.project_id = Some(project_id.to_string());
    input.target_branch = Some(target_branch.to_string());
    input.max_concurrent_executions = config
        .concurrency
        .unwrap_or(DEFAULT_CONCURRENCY)
        .clamp(1, MAX_CONCURRENCY);
    input.max_review_rounds = config
        .max_review_rounds
        .unwrap_or(DEFAULT_REVIEW_ROUNDS)
        .clamp(1, MAX_REVIEW_ROUNDS);
    input.validations = config.validations.clone();
    let task_ids: Vec<String> = subtasks.iter().map(|task| task.id.clone()).collect();
    let discussion_id = discussion_id.to_string();
    let actor = step_actor(delegation.run_id, delegation.step_name);
    delegation
        .state
        .db
        .with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let plan = crate::db::planning::get_discussion_plan(&tx, &discussion_id)?;
            let linked: std::collections::HashSet<String> = plan
                .active
                .iter()
                .chain(plan.later.iter())
                .map(|relation| relation.task.id.clone())
                .collect();
            for task_id in task_ids.iter().filter(|id| !linked.contains(*id)) {
                crate::db::planning::link_discussion(
                    &tx,
                    task_id,
                    &crate::models::LinkPlanningDiscussionRequest {
                        discussion_id: discussion_id.clone(),
                        placement: Default::default(),
                        is_primary: false,
                        position: None,
                        actor: actor.clone(),
                    },
                )?;
            }
            let existing: Option<String> = tx
                .query_row(
                    "SELECT id FROM orchestration_runs WHERE discussion_id = ?1 \
                     AND kind = 'campaign' ORDER BY created_at DESC LIMIT 1",
                    [&discussion_id],
                    |row| row.get(0),
                )
                .optional()?;
            let run = match existing {
                Some(id) => crate::db::orchestration::get_orchestration_run(&tx, &id)?
                    .context("campaign vanished")?,
                None => crate::db::orchestration::create_orchestration_run(&tx, &input)?,
            };
            tx.commit()?;
            Ok(run)
        })
        .await
}

/// The latest execution of each subtask in the campaign.
async fn latest_executions(
    db: &Database,
    campaign_id: &str,
    task_ids: Vec<String>,
) -> anyhow::Result<Vec<Option<TaskExecution>>> {
    let campaign_id = campaign_id.to_string();
    db.with_read_conn(move |conn| {
        task_ids
            .iter()
            .map(|task_id| {
                let id: Option<String> = conn
                    .query_row(
                        "SELECT id FROM task_executions WHERE orchestration_run_id = ?1 \
                         AND task_id = ?2 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                        rusqlite::params![campaign_id, task_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                match id {
                    Some(id) => crate::db::orchestration::get_task_execution(conn, &id),
                    None => Ok(None),
                }
            })
            .collect()
    })
    .await
}

/// The reason the latest escalation of an execution recorded.
async fn escalation_reason(db: &Database, execution_id: &str) -> anyhow::Result<Option<String>> {
    let id = execution_id.to_string();
    db.with_read_conn(move |conn| {
        Ok(conn
            .query_row(
                "SELECT json_extract(changes_json, '$.reason') FROM task_execution_events \
                 WHERE task_execution_id = ?1 AND (to_status = 'Escalated' OR action = 'escalated') \
                 AND json_extract(changes_json, '$.reason') IS NOT NULL \
                 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                [&id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    })
    .await
}

async fn integration_conflicts(db: &Database, execution_id: &str) -> anyhow::Result<i64> {
    let id = execution_id.to_string();
    db.with_read_conn(move |conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM task_execution_events WHERE task_execution_id = ?1 \
             AND action = ?2 AND json_extract(changes_json, '$.reason') LIKE 'merge conflict%'",
            rusqlite::params![id, crate::db::orchestration::INTEGRATION_REWORKED],
            |row| row.get(0),
        )?)
    })
    .await
}

/// One look at the durable state, and at most one round of actions on it.
async fn advance(
    delegation: &Delegation<'_>,
    setup: &Setup,
    world: &dyn DelegationWorld,
    report: &mut DelegationReport,
    deadline: Deadline,
) -> anyhow::Result<Progress> {
    let db = &delegation.state.db;
    let task_ids: Vec<String> = setup.subtasks.iter().map(|task| task.id.clone()).collect();
    let executions = latest_executions(db, &setup.campaign_id, task_ids.clone()).await?;
    let ids = task_ids.clone();
    let statuses: Vec<PlanningTaskStatus> = db
        .with_read_conn(move |conn| {
            ids.iter()
                .map(|id| {
                    Ok(crate::db::planning::get_task(conn, id)?
                        .context("subtask vanished")?
                        .summary
                        .status)
                })
                .collect()
        })
        .await?;

    // Durable stops first: what needs a human never waits on a model.
    for (subtask, execution) in setup.subtasks.iter().zip(&executions) {
        let Some(execution) = execution else { continue };
        let reference = &subtask.reference;
        match execution.status {
            TaskExecutionStatus::Escalated => {
                let reason = escalation_reason(db, &execution.id).await?;
                return Ok(Progress::Stopped(Stop::new(
                    DelegationSignal::Escalated,
                    format!(
                        "{reference} escalated: {}",
                        reason.as_deref().unwrap_or("no reason recorded")
                    ),
                )));
            }
            TaskExecutionStatus::Failed | TaskExecutionStatus::Cancelled => {
                return Ok(Progress::Stopped(Stop::new(
                    DelegationSignal::Failed,
                    format!(
                        "{reference} execution is {}: {}",
                        execution.status.as_str(),
                        execution.outcome_reason.as_deref().unwrap_or("no reason")
                    ),
                )))
            }
            TaskExecutionStatus::Blocked => {
                return Ok(Progress::Stopped(Stop::new(
                    DelegationSignal::Blocked,
                    format!(
                        "{reference} is blocked: {}",
                        execution.blocked_reason.as_deref().unwrap_or("no reason")
                    ),
                )))
            }
            TaskExecutionStatus::Approved if execution.blocked_reason_code.is_some() => {
                return Ok(Progress::Stopped(Stop::new(
                    DelegationSignal::Blocked,
                    format!(
                        "{reference} is approved but its integration is held: {}",
                        execution.blocked_reason.as_deref().unwrap_or("no reason")
                    ),
                )))
            }
            _ => {}
        }
        if integration_conflicts(db, &execution.id).await? >= CONFLICT_LIMIT {
            return Ok(Progress::Stopped(Stop::new(
                DelegationSignal::Conflict,
                format!(
                    "{reference} conflicted with `{}` again on integration",
                    setup.target_branch
                ),
            )));
        }
    }

    let mut moved = false;
    for (subtask, execution) in setup.subtasks.iter().zip(&executions) {
        let Some(execution) = execution else { continue };
        match execution.status {
            // An approval the step committed before a stop: finish its integration.
            TaskExecutionStatus::Approved => {
                if let Some(stop) = past_deadline(world, deadline) {
                    return Ok(Progress::Stopped(stop));
                }
                if let IntegrationOutcome::Refused { reason } = integrate(db, &execution.id).await?
                {
                    return Ok(Progress::Stopped(Stop::new(
                        DelegationSignal::Blocked,
                        format!("{} integration refused: {reason}", subtask.reference),
                    )));
                }
                moved = true;
            }
            TaskExecutionStatus::AwaitingReview => {
                if let Some(stop) = review(
                    delegation, setup, world, report, subtask, execution, deadline,
                )
                .await?
                {
                    return Ok(Progress::Stopped(stop));
                }
                moved = true;
            }
            _ => {}
        }
    }
    if moved {
        return Ok(Progress::Moved);
    }

    if statuses.iter().all(|status| {
        matches!(
            status,
            PlanningTaskStatus::Done | PlanningTaskStatus::Archived
        )
    }) {
        return Ok(Progress::Stopped(Stop::new(
            DelegationSignal::Ok,
            format!(
                "{} subtask(s) integrated into `{}`",
                setup.subtasks.len(),
                setup.target_branch
            ),
        )));
    }

    let campaign_id = setup.campaign_id.clone();
    let (campaign, candidates) = db
        .with_read_conn(move |conn| {
            let campaign = crate::db::orchestration::get_orchestration_run(conn, &campaign_id)?
                .context("campaign vanished")?;
            let candidates =
                crate::db::orchestration::campaign_task_candidates(conn, &campaign_id, None)?;
            Ok((campaign, candidates))
        })
        .await?;
    if !matches!(
        campaign.control_state,
        OrchestrationControlState::Running | OrchestrationControlState::Completed
    ) {
        return Ok(Progress::Stopped(Stop::new(
            DelegationSignal::Blocked,
            format!(
                "campaign is {}: {}",
                campaign.control_state.as_str(),
                campaign.control_reason.as_deref().unwrap_or("no reason")
            ),
        )));
    }

    // Launch what the campaign admits, in plan order, within its concurrency.
    let mut launched = false;
    if campaign.control_state == OrchestrationControlState::Running {
        for candidate in candidates.iter().filter(|candidate| candidate.launchable) {
            let Some(subtask) = setup
                .subtasks
                .iter()
                .find(|subtask| subtask.id == candidate.task.id)
            else {
                continue;
            };
            let Some(worker) = subtask.worker.clone() else {
                continue;
            };
            let launch = provision_campaign_task_execution(
                db,
                CampaignProvisionInput {
                    orchestration_run_id: setup.campaign_id.clone(),
                    task_reference: subtask.id.clone(),
                    worker_override: Some(worker),
                    idempotency_key: Some(format!(
                        "wf-delegate:{}:{}:{}",
                        delegation.run_id, delegation.step_name, subtask.id
                    )),
                },
            )
            .await;
            if let Err(error) = launch {
                return Ok(Progress::Stopped(Stop::new(
                    DelegationSignal::Blocked,
                    format!("{} could not launch: {error:?}", subtask.reference),
                )));
            }
            launched = true;
        }
    }
    if launched {
        return Ok(Progress::Moved);
    }

    let live = executions.iter().flatten().any(|execution| {
        !execution.status.is_terminal() && execution.status != TaskExecutionStatus::Escalated
    });
    if !live {
        let reasons = candidates
            .iter()
            .filter(|candidate| task_ids.contains(&candidate.task.id) && !candidate.launchable)
            .filter(|candidate| candidate.task.status == PlanningTaskStatus::Todo)
            .map(|candidate| {
                let why = candidate
                    .reasons
                    .iter()
                    .map(|reason| reason.detail.clone())
                    .collect::<Vec<_>>()
                    .join("; ");
                format!("{}: {why}", candidate.task.reference)
            })
            .collect::<Vec<_>>();
        return Ok(Progress::Stopped(Stop::new(
            DelegationSignal::Blocked,
            if reasons.is_empty() {
                "no subtask can launch and none is running".to_string()
            } else {
                format!("no subtask can launch: {}", reasons.join(" | "))
            },
        )));
    }
    Ok(Progress::Idle)
}

async fn integrate(db: &Database, execution_id: &str) -> anyhow::Result<IntegrationOutcome> {
    run_integration(db, execution_id)
        .await
        .map_err(|error| anyhow::anyhow!("integration failed: {error:?}"))
}

/// Typed reviewer output, a superset of `ReviewDecisionV1`'s verdicts.
#[derive(Debug, Clone, serde::Deserialize)]
struct ReviewerVerdict {
    decision: String,
    #[serde(default)]
    comment: Option<String>,
    #[serde(default)]
    findings: Vec<crate::models::ReviewFinding>,
    #[serde(default)]
    dod_verifications: Vec<crate::models::ReviewDodVerification>,
    #[serde(default)]
    worker: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// The last JSON object of the reply that carries a `decision`.
fn parse_verdict(text: &str) -> Option<ReviewerVerdict> {
    let mut found = None;
    for (start, _) in text.match_indices('{') {
        let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
        if let Some(Ok(value)) = stream.next() {
            if value.get("decision").is_some() {
                if let Ok(verdict) = serde_json::from_value::<ReviewerVerdict>(value) {
                    found = Some(verdict);
                }
            }
        }
    }
    found
}

async fn review(
    delegation: &Delegation<'_>,
    setup: &Setup,
    world: &dyn DelegationWorld,
    report: &mut DelegationReport,
    subtask: &Subtask,
    execution: &TaskExecution,
    deadline: Deadline,
) -> anyhow::Result<Option<Stop>> {
    let db = &delegation.state.db;
    let message_id = review_message_id(&execution.id, execution.attempt_no);
    let lookup = message_id.clone();
    let recorded: Option<String> = db
        .with_read_conn(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT content FROM messages WHERE id = ?1",
                    [&lookup],
                    |row| row.get(0),
                )
                .optional()?)
        })
        .await?;
    // A verdict recorded before a restart is applied again, never re-bought.
    let text = match recorded {
        Some(text) => text,
        None => {
            if let Some(budget) = delegation.budget.as_ref() {
                if budget.llm_calls() >= budget.max_llm_calls() {
                    return Ok(Some(Stop::new(
                        DelegationSignal::Blocked,
                        format!(
                            "the run's LLM-call budget ({}) is spent; {} awaits review",
                            budget.max_llm_calls(),
                            subtask.reference
                        ),
                    )));
                }
            }
            if let Some(stop) = past_deadline(world, deadline) {
                return Ok(Some(stop));
            }
            let request = review_request(delegation, setup, subtask, execution).await?;
            // A started session counts, whether it answers or is cut by the
            // deadline, and is durably recorded before it starts (KT-1046).
            if let Some(budget) = delegation.budget.as_ref() {
                budget.add_llm_calls(1);
                if let Err(error) =
                    super::runner::record_tree_llm_calls(delegation.state, budget).await
                {
                    let reason = format!(
                        "the run's LLM-call count could not be recorded ({error}); no review was started"
                    );
                    report.terminal_stop = Some(reason.clone());
                    return Ok(Some(Stop::new(DelegationSignal::Failed, reason)));
                }
            }
            report.review_calls += 1;
            let reply = tokio::select! {
                biased;
                reply = world.review(request) => reply,
                _ = world.wait_until(deadline.at) => {
                    return Ok(past_deadline(world, deadline).or_else(|| {
                        Some(Stop::new(DelegationSignal::Timeout, "step timeout reached during a review"))
                    }));
                }
            };
            report.review_tokens += reply.tokens.unwrap_or(0);
            if let Some(wait) = reply.quota_wait {
                // A quota refusal spent nothing; if the release fails it stays counted.
                if let Some(budget) = delegation.budget.as_ref() {
                    if let Err(error) =
                        super::runner::release_quota_refused_llm_call(delegation.state, budget)
                            .await
                    {
                        tracing::warn!(run_id = %delegation.run_id, %error,
                            "a quota-refused review stays counted: its release was not recorded");
                    }
                }
                report.quota_wait = Some(wait);
                return Ok(Some(Stop::new(
                    DelegationSignal::Failed,
                    format!(
                        "the reviewer's provider refused for a quota; {} awaits review",
                        subtask.reference
                    ),
                )));
            }
            if let Some(failure) = reply.failure {
                return Ok(Some(Stop::new(
                    DelegationSignal::Failed,
                    format!("the reviewer of {} failed: {failure}", subtask.reference),
                )));
            }
            record_verdict(delegation, &setup.discussion_id, &message_id, &reply).await?;
            reply.text
        }
    };
    // A late verdict stays recorded; the resumed step applies it.
    if let Some(stop) = past_deadline(world, deadline) {
        return Ok(Some(stop));
    }

    let Some(verdict) = parse_verdict(&text) else {
        return escalate(
            db,
            delegation,
            execution,
            "the reviewer's verdict is not readable JSON",
        )
        .await;
    };
    let reviewer = format!("{:?} reviewer", delegation.reviewer_agent);
    match verdict.decision.as_str() {
        "approve" => {
            let mut decision = json!({
                "version": "1",
                "task_ref": subtask.reference,
                "decision": "approve",
                "reviewed_head_sha": delivered_head(db, execution).await?,
                "dod_verifications": verdict
                    .dod_verifications
                    .iter()
                    .filter(|item| !item.evidence.trim().is_empty())
                    .collect::<Vec<_>>(),
            });
            if let Some(comment) = verdict.comment.as_deref().filter(|c| !c.trim().is_empty()) {
                decision["comment"] = json!(comment);
            }
            match decide_review_as_delegation_step(
                db,
                &execution.id,
                &decision.to_string(),
                &reviewer,
            )
            .await
            .map_err(|error| anyhow::anyhow!("approve failed: {error:?}"))?
            {
                ReviewOutcome::Reviewed { .. } => {
                    if let Some(stop) = past_deadline(world, deadline) {
                        return Ok(Some(stop));
                    }
                    if let IntegrationOutcome::Refused { reason } =
                        integrate(db, &execution.id).await?
                    {
                        return Ok(Some(Stop::new(
                            DelegationSignal::Blocked,
                            format!("{} integration refused: {reason}", subtask.reference),
                        )));
                    }
                    Ok(None)
                }
                ReviewOutcome::ApproveBlocked { reason } => {
                    escalate(
                        db,
                        delegation,
                        execution,
                        &format!("approval refused by Kronn's guard: {reason:?}"),
                    )
                    .await
                }
                ReviewOutcome::NotReviewable { .. } => Ok(None),
                other => Ok(Some(Stop::new(
                    DelegationSignal::Failed,
                    format!("{} approval was not applied: {other:?}", subtask.reference),
                ))),
            }
        }
        "request_changes" => {
            let comment = verdict
                .comment
                .clone()
                .filter(|comment| !comment.trim().is_empty())
                .unwrap_or_else(|| "Changes requested; see the findings.".to_string());
            let decision = json!({
                "version": "1",
                "task_ref": subtask.reference,
                "decision": "request_changes",
                "comment": comment,
                "findings": verdict.findings,
            });
            match decide_review_as_delegation_step(
                db,
                &execution.id,
                &decision.to_string(),
                &reviewer,
            )
            .await
            .map_err(|error| anyhow::anyhow!("request_changes failed: {error:?}"))?
            {
                ReviewOutcome::Reviewed { .. } | ReviewOutcome::NotReviewable { .. } => Ok(None),
                ReviewOutcome::Escalated { execution } => Ok(Some(Stop::new(
                    DelegationSignal::Escalated,
                    format!(
                        "{} exhausted its {} review round(s)",
                        subtask.reference, execution.max_review_rounds
                    ),
                ))),
                other => Ok(Some(Stop::new(
                    DelegationSignal::Failed,
                    format!(
                        "{} change request was not applied: {other:?}",
                        subtask.reference
                    ),
                ))),
            }
        }
        "reassign" => reassign(delegation, execution, &verdict).await,
        "escalate" => {
            let reason = verdict
                .reason
                .or(verdict.comment)
                .unwrap_or_else(|| "the reviewer escalated".to_string());
            escalate(db, delegation, execution, &reason).await
        }
        other => {
            escalate(
                db,
                delegation,
                execution,
                &format!("unknown reviewer decision `{other}`"),
            )
            .await
        }
    }
}

/// One reassignment per subtask; a second request escalates to a human.
async fn reassign(
    delegation: &Delegation<'_>,
    execution: &TaskExecution,
    verdict: &ReviewerVerdict,
) -> anyhow::Result<Option<Stop>> {
    let db = &delegation.state.db;
    let Some(worker) = verdict
        .worker
        .as_deref()
        .and_then(|key| delegation.config.worker_map.get(key.trim()))
    else {
        return escalate(
            db,
            delegation,
            execution,
            "the reviewer asked for a reassignment to a worker `worker_map` does not name",
        )
        .await;
    };
    let marker = orchestrator_note(
        reassign_marker_id(&execution.id, execution.attempt_no),
        format!(
            "Réaffectation demandée par la relecture : {:?}",
            worker.agent
        ),
    );
    let discussion_id = execution.parent_discussion_id.clone();
    let family = format!("wf-reassign:{}:%", execution.id);
    let allowed = db
        .with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let pending: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1)",
                [&marker.id],
                |row| row.get(0),
            )?;
            let earlier: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id LIKE ?1 AND id <> ?2)",
                rusqlite::params![family, marker.id],
                |row| row.get(0),
            )?;
            if !pending && !earlier {
                crate::db::discussions::insert_message(&tx, &discussion_id, &marker)?;
            }
            tx.commit()?;
            // A pending marker resumes the same reassignment; an earlier one means a second.
            Ok(pending || !earlier)
        })
        .await?;
    if !allowed {
        return escalate(
            db,
            delegation,
            execution,
            "a second reassignment was requested",
        )
        .await;
    }
    let reason = verdict
        .reason
        .clone()
        .or(verdict.comment.clone())
        .unwrap_or_else(|| "reassigned by the delegation reviewer".to_string());
    crate::api::orchestration::reassign_native_execution(
        delegation.state,
        &execution.id,
        worker_selection(worker),
        &reason,
    )
    .await?;
    Ok(None)
}

async fn escalate(
    db: &Database,
    delegation: &Delegation<'_>,
    execution: &TaskExecution,
    reason: &str,
) -> anyhow::Result<Option<Stop>> {
    let id = execution.id.clone();
    let actor = step_actor(delegation.run_id, delegation.step_name);
    let detail =
        json!({ "reason": reason, "attempt": execution.attempt_no, "source": "delegate_subtasks" });
    db.with_conn(move |conn| {
        crate::db::orchestration::transition_execution(
            conn,
            &id,
            TaskExecutionStatus::Escalated,
            &actor,
            detail,
        )?;
        Ok(())
    })
    .await?;
    Ok(Some(Stop::new(
        DelegationSignal::Escalated,
        reason.to_string(),
    )))
}

fn orchestrator_note(id: String, content: String) -> DiscussionMessage {
    let mut message = crate::api::orchestration::orchestrator_message(id, content);
    message.role = MessageRole::System;
    message
}

/// The reviewer's reply joins the room under an id keyed by (execution, attempt).
async fn record_verdict(
    delegation: &Delegation<'_>,
    discussion_id: &str,
    message_id: &str,
    reply: &ReviewReply,
) -> anyhow::Result<()> {
    let message = DiscussionMessage {
        id: message_id.to_string(),
        role: MessageRole::Agent,
        channel: MessageChannel::Main,
        content: reply.text.clone(),
        agent_type: Some(delegation.reviewer_agent.clone()),
        timestamp: chrono::Utc::now(),
        tokens_used: reply.tokens.unwrap_or(0),
        session_tokens_at_message: None,
        recovered_partial: false,
        auth_mode: None,
        model_tier: None,
        model: reply.model.clone(),
        cost_usd: reply.cost_usd,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        lint_report: None,
        target_agent: None,
        reply_to_message_id: None,
        author_cli_ordinal: None,
    };
    let discussion_id = discussion_id.to_string();
    delegation
        .state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::insert_message(conn, &discussion_id, &message)?;
            Ok(())
        })
        .await
}

async fn delivered_head(db: &Database, execution: &TaskExecution) -> anyhow::Result<String> {
    let (id, attempt) = (execution.id.clone(), execution.attempt_no);
    db.with_read_conn(move |conn| {
        Ok(
            crate::db::worker_deliveries::get_delivery(conn, &id, attempt)?
                .context("no delivery for the attempt under review")?
                .head_sha,
        )
    })
    .await
}

/// The compact context of a fresh review session: DoD, report, diff.
async fn review_request(
    delegation: &Delegation<'_>,
    setup: &Setup,
    subtask: &Subtask,
    execution: &TaskExecution,
) -> anyhow::Result<ReviewRequest> {
    let db = &delegation.state.db;
    let (id, attempt, task_id) = (
        execution.id.clone(),
        execution.attempt_no,
        execution.task_id.clone(),
    );
    let (task, delivery, workspace) = db
        .with_read_conn(move |conn| {
            let task = crate::db::planning::get_task(conn, &task_id)?.context("task vanished")?;
            let delivery = crate::db::worker_deliveries::get_delivery(conn, &id, attempt)?
                .context("no delivery for the attempt under review")?;
            let workspace = crate::db::discussion_workspaces::get_managed_for_execution(conn, &id)?;
            Ok((task, delivery, workspace))
        })
        .await?;
    let work_dir = workspace
        .and_then(|workspace| workspace.canonical_path)
        .context("the delivered worktree is gone")?;
    let base = execution
        .base_sha
        .clone()
        .context("execution has no base")?;
    let diff = git_diff(&work_dir, &base, &delivery.head_sha).await?;
    let manifest: Value = serde_json::from_str(&delivery.manifest_json).unwrap_or(Value::Null);
    let dod = task
        .definition_of_done
        .iter()
        .map(|item| format!("- `{}` — {}", item.id, item.sentence))
        .collect::<Vec<_>>()
        .join("\n");
    let report = json!({
        "summary": manifest.get("summary"),
        "dod_status": manifest.get("dod_status"),
        "tests": manifest.get("tests"),
        "risks": manifest.get("risks"),
        "limitations": manifest.get("limitations"),
    });
    let validations = if delegation.config.validations.is_empty() {
        "none".to_string()
    } else {
        delegation
            .config
            .validations
            .iter()
            .map(|spec| format!("`{}`", spec.command))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let workers = delegation
        .config
        .worker_map
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let rounds = format!(
        "{}/{}",
        execution.review_rounds, execution.max_review_rounds
    );
    let guidance = delegation.reviewer_guidance.trim();
    let prompt = format!(
        "You review one delivery of a delegated subtask. Judge it against its Definition of Done only.\n\n\
         ## Task {reference} — {title}\n{description}\n\n\
         ## Definition of Done\n{dod}\n\n\
         ## Worker report (attempt {attempt}, review rounds used {rounds})\n```json\n{report}\n```\n\n\
         ## Diff `{base_short}..{head_short}` on `{target}`\n```diff\n{diff}\n```\n\n\
         Kronn runs these validations on the approved candidate before integrating it: {validations}.\n\
         {guidance}\n\n\
         Answer with ONE JSON object, last in your reply:\n\
         {{\"decision\": \"approve\" | \"request_changes\" | \"reassign\" | \"escalate\",\n \
         \"comment\": \"…\", \"findings\": [{{\"path\": \"…\", \"line\": 1, \"issue\": \"…\"}}],\n \
         \"dod_verifications\": [{{\"dod_id\": \"…\", \"met\": true, \"evidence\": \"…\"}}],\n \
         \"worker\": \"one of: {workers}\", \"reason\": \"…\"}}\n\
         `approve` needs one `dod_verifications` entry with evidence for EVERY DoD id above; \
         `request_changes` needs a comment; `reassign` names a worker; `escalate` gives a reason for a human.",
        reference = subtask.reference,
        title = subtask.title,
        description = task.description.trim(),
        attempt = execution.attempt_no,
        report = serde_json::to_string_pretty(&report).unwrap_or_default(),
        base_short = base.chars().take(10).collect::<String>(),
        head_short = delivery.head_sha.chars().take(10).collect::<String>(),
        target = setup.target_branch,
    );
    Ok(ReviewRequest {
        execution_id: execution.id.clone(),
        attempt_no: execution.attempt_no,
        task_reference: subtask.reference.clone(),
        prompt,
        work_dir,
    })
}

async fn git_diff(work_dir: &str, base: &str, head: &str) -> anyhow::Result<String> {
    worktree::reject_option_like_rev(base).map_err(anyhow::Error::msg)?;
    worktree::reject_option_like_rev(head).map_err(anyhow::Error::msg)?;
    let output = crate::core::cmd::async_git_cmd()
        .args([
            "diff",
            "--no-color",
            "--no-ext-diff",
            &format!("{base}..{head}"),
            "--",
        ])
        .current_dir(work_dir)
        .output()
        .await
        .context("git diff could not run")?;
    if !output.status.success() {
        anyhow::bail!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let diff = String::from_utf8_lossy(&output.stdout);
    if diff.chars().count() <= DIFF_MAX_CHARS {
        return Ok(diff.into_owned());
    }
    let kept: String = diff.chars().take(DIFF_MAX_CHARS).collect();
    Ok(format!(
        "{kept}\n… diff cut at {DIFF_MAX_CHARS} characters; read the rest in the worktree."
    ))
}

async fn subtask_reports(db: &Database, setup: &Setup) -> anyhow::Result<Vec<SubtaskReport>> {
    let task_ids: Vec<String> = setup.subtasks.iter().map(|task| task.id.clone()).collect();
    let executions = latest_executions(db, &setup.campaign_id, task_ids).await?;
    let mut reports = Vec::new();
    for (subtask, execution) in setup.subtasks.iter().zip(executions) {
        let conflicts = match &execution {
            Some(execution) => integration_conflicts(db, &execution.id).await?,
            None => 0,
        };
        let task_id = subtask.id.clone();
        let execution_for_cost = execution.clone();
        let (status, cost_usd, tokens) = db
            .with_read_conn(move |conn| {
                let status =
                    crate::db::planning::get_task(conn, &task_id)?.map(|task| task.summary.status);
                let Some(execution) = execution_for_cost else {
                    return Ok((status, None, 0u64));
                };
                let (cost, tokens): (Option<f64>, i64) = conn.query_row(
                    "SELECT SUM(cost_usd), COALESCE(SUM(tokens_used), 0) FROM messages \
                     WHERE discussion_id = ?1 OR id LIKE ?2",
                    rusqlite::params![
                        execution.sub_discussion_id.unwrap_or_default(),
                        format!("wf-review:{}:%", execution.id)
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                Ok((status, cost, tokens.max(0) as u64))
            })
            .await?;
        reports.push(SubtaskReport {
            task: subtask.reference.clone(),
            title: subtask.title.clone(),
            status: match (&execution, status) {
                (Some(execution), _) => execution.status.as_str().to_string(),
                (None, Some(status)) => format!("{status:?}"),
                (None, None) => "Missing".to_string(),
            },
            execution_id: execution.as_ref().map(|execution| execution.id.clone()),
            integrated_sha: execution
                .as_ref()
                .and_then(|execution| execution.integrated_sha.clone()),
            review_rounds: execution.as_ref().map_or(0, |e| e.review_rounds),
            attempts: execution.as_ref().map_or(0, |e| e.attempt_no + 1),
            integration_conflicts: conflicts.max(0) as u32,
            worker: execution
                .as_ref()
                .and_then(|execution| execution.worker_agent_type.clone()),
            cost_usd,
            tokens,
        });
    }
    Ok(reports)
}

/// The production world: a fresh one-shot agent session per review, no
/// orchestration tools, and an SQL read every [`IDLE_POLL`].
pub(crate) struct LiveWorld<'a> {
    pub reviewer: WorkflowStep,
    pub project_path: &'a str,
    pub project_id: Option<&'a str>,
    pub tokens_config: &'a crate::models::TokensConfig,
    pub agents_config: &'a crate::models::AgentsConfig,
    pub ollama_context_overrides: &'a std::collections::HashMap<String, u64>,
    pub db: &'a Database,
    pub run_id: &'a str,
}

#[async_trait::async_trait]
impl DelegationWorld for LiveWorld<'_> {
    async fn review(&self, request: ReviewRequest) -> ReviewReply {
        tracing::info!(
            target: "kronn::delegate_subtasks",
            run_id = %self.run_id,
            execution_id = %request.execution_id,
            attempt = request.attempt_no,
            task = %request.task_reference,
            "reviewing a delegated delivery in a fresh session"
        );
        let mut ctx = TemplateContext::new();
        // Inserted as a value, never rendered: diffs may hold `{{`.
        ctx.set("review.context", request.prompt);
        let outcome = super::steps::execute_step(
            &self.reviewer,
            self.project_path,
            self.project_id,
            &request.work_dir,
            self.tokens_config,
            self.agents_config.full_access_for(&self.reviewer.agent),
            &ctx,
            None,
            None,
            Some(&self.agents_config.model_tiers),
            Some(&crate::models::setup::HttpEndpoints::from_agents(
                self.agents_config,
            )),
            Some(self.ollama_context_overrides),
            None,
            Some(self.db),
            None,
            Some(self.run_id),
        )
        .await;
        let result = outcome.result;
        let cost_usd = result.agent_provenance.as_ref().and_then(|provenance| {
            provenance
                .attempts
                .iter()
                .map(|attempt| attempt.cost_usd)
                .sum::<Option<f64>>()
        });
        let model = result.step_model.clone();
        ReviewReply {
            failure: (result.status != RunStatus::Success && result.quota_wait.is_none())
                .then(|| result.output.chars().take(500).collect()),
            text: result.output,
            tokens: result.tokens_used,
            cost_usd,
            model,
            quota_wait: result.quota_wait,
        }
    }

    async fn idle(&self) {
        tokio::time::sleep(IDLE_POLL).await;
    }
}

/// The reviewer is the step's own agent and settings, as a plain one-shot Agent step.
fn reviewer_step(step: &WorkflowStep) -> WorkflowStep {
    let mut reviewer = step.clone();
    reviewer.step_type = crate::models::StepType::Agent;
    reviewer.prompt_template = "{{review.context}}".to_string();
    reviewer.output_format = crate::models::StepOutputFormat::FreeText;
    reviewer.on_result = Vec::new();
    // One review is one session: a retried attempt would be an uncounted call.
    reviewer.retry = None;
    reviewer.quick_prompt_id = None;
    reviewer.multi_agent_review = None;
    reviewer.room_id = None;
    reviewer.delegate_subtasks = None;
    reviewer
}

/// Runner entry point.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_delegate_subtasks_step(
    state: &AppState,
    step: &WorkflowStep,
    run_id: &str,
    workflow_project_id: Option<&str>,
    project_path: &str,
    work_dir: &str,
    ctx: &TemplateContext,
    tokens_config: &crate::models::TokensConfig,
    agents_config: &crate::models::AgentsConfig,
    ollama_context_overrides: &std::collections::HashMap<String, u64>,
    budget: SharedBudget,
) -> StepOutcome {
    let start = Instant::now();
    let Some(config) = step.delegate_subtasks.as_ref() else {
        return finish(
            step,
            start,
            DelegationReport::refused("DelegateSubtasks step missing `delegate_subtasks`"),
        );
    };
    let parent_task = match ctx.render_strict(&config.parent_task) {
        Ok(parent) => parent,
        Err(error) => {
            return finish(
                step,
                start,
                DelegationReport::refused(format!("`parent_task` could not be rendered: {error}")),
            )
        }
    };
    let target_branch = match config
        .target_branch
        .as_deref()
        .map(|t| ctx.render_strict(t))
    {
        None => None,
        Some(Ok(branch)) => Some(branch),
        Some(Err(error)) => {
            return finish(
                step,
                start,
                DelegationReport::refused(format!(
                    "`target_branch` could not be rendered: {error}"
                )),
            )
        }
    };
    let guidance = match ctx.render_strict(&step.prompt_template) {
        Ok(guidance) => guidance,
        Err(error) => {
            return finish(
                step,
                start,
                DelegationReport::refused(format!(
                    "`prompt_template` could not be rendered: {error}"
                )),
            )
        }
    };
    let delegation = Delegation {
        state,
        run_id,
        step_name: &step.name,
        workflow_project_id,
        reviewer_agent: step.agent.clone(),
        config,
        parent_task,
        target_branch,
        work_dir,
        reviewer_guidance: &guidance,
        budget: Some(budget),
    };
    let world = LiveWorld {
        reviewer: reviewer_step(step),
        project_path,
        project_id: workflow_project_id,
        tokens_config,
        agents_config,
        ollama_context_overrides,
        db: &state.db,
        run_id,
    };
    let report = run_delegation(&delegation, &world).await;
    finish(step, start, report)
}

/// `on_failure` cannot delegate: a rollback must not launch new work.
pub(crate) fn forbidden_in_rollback(step: &WorkflowStep) -> StepOutcome {
    finish(
        step,
        Instant::now(),
        DelegationReport::refused("DelegateSubtasks is not allowed in `on_failure`"),
    )
}

impl DelegationReport {
    fn refused(reason: impl Into<String>) -> Self {
        Self {
            signal: DelegationSignal::Failed,
            summary: reason.into(),
            discussion_id: None,
            campaign_id: None,
            target_branch: None,
            subtasks: Vec::new(),
            review_calls: 0,
            review_tokens: 0,
            quota_wait: None,
            terminal_stop: None,
        }
    }
}

pub(crate) fn finish(step: &WorkflowStep, start: Instant, report: DelegationReport) -> StepOutcome {
    let signal = report.signal.as_str();
    let data = json!({
        "discussion_id": report.discussion_id,
        "campaign_id": report.campaign_id,
        "target_branch": report.target_branch,
        "review_calls": report.review_calls,
        "subtasks": report.subtasks,
    });
    let output = super::step_output_format::format_step_output(
        data,
        signal,
        &report.summary,
        None,
        &[signal],
    );
    let condition_action = super::steps::evaluate_conditions(&step.on_result, &output);
    let condition_result = condition_action.as_ref().map(|action| match action {
        ConditionAction::Stop => "Stop".to_string(),
        ConditionAction::Skip => "Skip".to_string(),
        ConditionAction::Goto { step_name, .. } => format!("Goto:{step_name}"),
    });
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: if report.signal == DelegationSignal::Ok {
                RunStatus::Success
            } else {
                RunStatus::Failed
            },
            output,
            tokens_used: Some(report.review_tokens),
            duration_ms: start.elapsed().as_millis() as u64,
            started_at: None,
            condition_result,
            envelope_detected: Some(true),
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
            quota_wait: report.quota_wait,
            terminal_stop: report.terminal_stop,
        },
        condition_action,
    }
}

#[cfg(test)]
#[path = "delegate_subtasks_step_tests.rs"]
mod tests;
