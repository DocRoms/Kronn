//! KT-318 provisioning saga — launch a ready plan task into a fresh sub-discussion
//! and a SHA-pinned sibling worktree, then dispatch a native worker.
//!
//! This coordinates Git (worktree), SQLite (execution / discussion / workspace /
//! dispatch) and planning, which are NOT one transaction. So the saga keeps a
//! DURABLE per-step checkpoint on the execution row (`Provisioning`, then the
//! breadcrumb columns) and compensates each failpoint in ownership-aware,
//! physical-then-intent order (ADR §4bis; DoD-6). Only the FINAL "durably
//! launchable" flip is one atomic SQLite commit
//! ([`crate::db::orchestration::commit_provisioning_checkpoint`]): brief + single
//! native dispatch + execution `Working` + task `Todo → InProgress`, nothing
//! visible to the dispatcher or `wait_for_peer` until it commits (DoD-7/8).
//!
//! A native worker (`DiscussionAgent`/`Agent`) drives to `Working`. A joined-`Cli`
//! worker cannot be woken by a native dispatch inside the child (a session owns one
//! room), so Phase E forks: it opens a durable control offer in the ORIGIN room
//! targeted at the exact session and parks `Blocked(awaiting_worker_acceptance)`
//! until the session accepts (KT-328); the HTTP/UX surface calling this saga is
//! KT-323.

use anyhow::{bail, Context, Result};
use axum::{
    extract::{Path, State},
    Json,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::{scanner, worktree};
use crate::db::orchestration::{CheckpointOutcome, ProvisioningCheckpoint};
use crate::db::planning::StartTaskCheckpoint;
use crate::db::Database;
use crate::models::{
    AgentType, ApiErrorCode, ApiResponse, BlockedReasonCode, DeliveryManifestV1, Discussion,
    DiscussionMessage, ExecutionRecoveryAction, FileChangeKind, ManifestFile, MessageChannel,
    MessageRole, MessageTarget, MessageTargetKind, ModelTier, OrchestrationActor, PlanningActor,
    PlanningActorKind, PlanningDodItem, PlanningTaskStatus, ReviewDecisionV1, ReviewFinding,
    ReviewVerdict, SagaResumeAction, SummaryStrategy, TaskExecution, TaskExecutionStatus,
    TaskWorkerScope, DELIVERY_CONTRACT_VERSION,
};
use crate::AppState;

/// Structured input to the launch saga.
pub struct ProvisionInput {
    /// `KT-###` or the task uuid.
    pub task_reference: String,
    /// The principal discussion the plan lives in (breadcrumb parent).
    pub parent_discussion_id: String,
    /// The chosen worker identity. `DiscussionAgent`/`Agent` launch natively; a
    /// `Cli` kind opens a child-bound control offer and parks awaiting acceptance
    /// (KT-328).
    pub worker: MessageTarget,
    /// Revision to pin the child branch from (branch/tag/sha). Defaults to `main`.
    pub base_rev: Option<String>,
    /// A retry with the same key resumes the same execution rather than duplicating.
    pub idempotency_key: Option<String>,
}

#[derive(Clone)]
struct CampaignLaunchContext {
    run_id: String,
    selection: crate::models::CampaignWorkerSelection,
}

struct BeginProvisioningContext<'a> {
    target_branch: Option<&'a str>,
    idempotency_key: Option<&'a str>,
    validations: &'a [crate::models::ValidationSpec],
    campaign: Option<&'a CampaignLaunchContext>,
    resume_execution_id: Option<&'a str>,
    worker_scope: Option<&'a TaskWorkerScope>,
}

/// Campaign launch request. `worker_override = None` resolves the persisted
/// default, then the principal discussion identity, with an explicit explanation.
pub struct CampaignProvisionInput {
    pub orchestration_run_id: String,
    pub task_reference: String,
    pub worker_override: Option<crate::models::CampaignWorkerSelection>,
    pub idempotency_key: Option<String>,
}

/// Structured refusal / failure of the launch saga. DoD-1 refusals are explicit,
/// and a mid-saga Git failure leaves the execution resumable rather than silent.
#[derive(Debug)]
pub enum ProvisionError {
    /// The task reference did not resolve.
    TaskNotFound,
    /// DoD-1: the task is not launchable, with a human-readable reason.
    NotLaunchable(String),
    /// A Git/workspace step failed. `compensated` = the physical worktree + the
    /// managed intent row were cleaned; otherwise a resumable row remains. The
    /// execution is left `Blocked`/`Provisioning`, never a silent orphan.
    WorkspaceFailed { reason: String, compensated: bool },
    /// The final atomic checkpoint refused (task raced out of Todo, or a blocker
    /// appeared); nothing was dispatched, task stays Todo, execution resumable.
    CheckpointRefused(String),
    /// An unexpected internal error.
    Internal(String),
}

/// A backend/system actor — every provisioning write is attributed to it.
fn backend_actor() -> OrchestrationActor {
    PlanningActor {
        kind: PlanningActorKind::Backend,
        id: Some("orchestrator".to_string()),
        session_id: None,
        source_message_id: None,
    }
}

/// A worker (agent) actor — a worker's own delivery is attributed to IT, not the backend,
/// so the journal shows who delivered (DoD-2). `Agent` is a non-spoofable planning actor
/// kind, distinct from the `Backend` orchestrator transitions.
/// An agent actor (`kind = Agent`) attributed to `alias` — the worker at delivery, the
/// principal at review. The non-spoofable identity planning already uses for its journal.
fn agent_actor(alias: &str, session_id: Option<&str>) -> OrchestrationActor {
    PlanningActor {
        kind: PlanningActorKind::Agent,
        id: Some(alias.to_string()),
        session_id: session_id.map(str::to_string),
        source_message_id: None,
    }
}

/// First 8 chars of the execution id — the collision-free short id in the
/// deterministic worktree path/branch.
fn exec_short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

#[derive(Debug, Default, Serialize)]
pub struct OrchestrationBootReport {
    pub interrupted: usize,
    pub classified: usize,
    pub resumed_or_parked: usize,
    pub orphan_workspaces_removed: usize,
    pub orphan_workspaces_preserved: usize,
    pub errors: Vec<String>,
}

pub(crate) fn available_agent_types(
    detected_agents: Vec<crate::models::AgentDetection>,
) -> Vec<AgentType> {
    detected_agents
        .into_iter()
        .filter(|agent| {
            agent.enabled
                && agent.auth_ready.unwrap_or(true)
                && (agent.installed || agent.runtime_available)
        })
        .map(|agent| agent.agent_type)
        .collect()
}

pub(crate) async fn available_agent_types_for_state(state: &AppState) -> Vec<AgentType> {
    let mut detected_agents = crate::agents::detect_all_cached(false).await;
    {
        let config = state.config.read().await;
        crate::agents::apply_configured_status(&mut detected_agents, &config);
    }
    available_agent_types(detected_agents)
}

/// Reconcile every orchestration boundary before the dispatch engine starts.
/// Each in-flight row first becomes `Interrupted`; only then do we inspect its
/// durable lineage and the real Git refs and persist a bounded recovery action.
/// No status is declared successful from a checkpoint alone.
pub async fn reconcile_at_boot(state: &AppState) -> OrchestrationBootReport {
    let mut report = OrchestrationBootReport::default();
    let moved = match state
        .db
        .with_conn(crate::db::orchestration::reconcile_stale_task_executions)
        .await
    {
        Ok(ids) => ids,
        Err(error) => {
            report
                .errors
                .push(format!("execution interrupt scan: {error}"));
            Vec::new()
        }
    };
    report.interrupted = moved.len();
    let ids = state
        .db
        .with_conn(crate::db::orchestration::list_interrupted_execution_ids)
        .await
        .unwrap_or_else(|error| {
            report.errors.push(format!(
                "interrupted execution classification scan: {error}"
            ));
            moved
        });

    let mut detected_agents = crate::agents::detect_all_cached(false).await;
    {
        let config = state.config.read().await;
        crate::agents::apply_configured_status(&mut detected_agents, &config);
    }
    let available_agents = available_agent_types(detected_agents);

    for id in ids {
        // Recovery decisions describe the real execution/Git state at one
        // instant. A failed apply may leave that decision pending while the
        // execution advances to a newer Interrupted checkpoint. Reclassify on
        // every boot before consuming it so a stale ResumeProvisioning can
        // never be replayed against an Applying-origin interruption.
        if let Err(error) = classify_interrupted_execution(&state.db, &id, &available_agents).await
        {
            report
                .errors
                .push(format!("execution {id} classification: {error}"));
            continue;
        }
        report.classified += 1;
        // Classification is durably journaled first. Applying it through the
        // same guarded surface as an explicit resume then either continues safe
        // work or parks a human-only boundary. A prior boot's pending decision
        // takes this exact path too.
        let response = resume_execution(State(state.clone()), Path(id.clone()))
            .await
            .0;
        if response.success {
            report.resumed_or_parked += 1;
        } else {
            report.errors.push(format!(
                "execution {id} recovery apply: {}",
                response
                    .error
                    .unwrap_or_else(|| "unknown recovery error".into())
            ));
        }
    }

    let orphans = state
        .db
        .with_conn(crate::db::discussion_workspaces::list_orphaned_managed)
        .await
        .unwrap_or_else(|error| {
            report
                .errors
                .push(format!("managed workspace orphan scan: {error}"));
            Vec::new()
        });
    for orphan in orphans {
        let Some(path) = orphan.canonical_path.clone() else {
            report.orphan_workspaces_preserved += 1;
            report.errors.push(format!(
                "workspace {} has no canonical path; preserved",
                orphan.id
            ));
            continue;
        };
        let project_path = {
            let project_id = orphan.project_id.clone();
            state
                .db
                .with_conn(move |conn| {
                    Ok(crate::db::projects::get_project(conn, &project_id)?.map(|p| p.path))
                })
                .await
        };
        let Ok(Some(project_path)) = project_path else {
            report.orphan_workspaces_preserved += 1;
            report.errors.push(format!(
                "workspace {} project is unavailable; preserved",
                orphan.id
            ));
            continue;
        };
        let repo = scanner::resolve_host_path(&project_path);
        let checkout = scanner::resolve_host_path(&path);
        let checkout_path = checkout.to_string_lossy().to_string();
        let removed = if !checkout.exists() {
            Ok(())
        } else {
            worktree::remove_cancelled_task_worktree(&repo, &checkout_path, &orphan.branch)
        };
        match removed {
            Ok(()) => {
                let workspace_id = orphan.id.clone();
                let branch = orphan.branch.clone();
                let cleanup = state
                    .db
                    .with_conn(move |conn| {
                        let tx = conn.unchecked_transaction()?;
                        crate::db::orchestration::record_reconciliation_event(
                            &tx,
                            "workspace",
                            &workspace_id,
                            "orphan_removed",
                            serde_json::json!({ "path": path, "branch": branch }),
                        )?;
                        crate::db::discussion_workspaces::delete_orphaned_managed(
                            &tx,
                            &workspace_id,
                        )?;
                        tx.commit()?;
                        Ok(())
                    })
                    .await;
                match cleanup {
                    Ok(()) => report.orphan_workspaces_removed += 1,
                    Err(error) => report.errors.push(format!(
                        "workspace {} DB orphan cleanup: {error}",
                        orphan.id
                    )),
                }
            }
            Err(error) => {
                report.orphan_workspaces_preserved += 1;
                let workspace_id = orphan.id.clone();
                let reason = error.clone();
                let _ = state
                    .db
                    .with_conn(move |conn| {
                        crate::db::orchestration::record_reconciliation_event(
                            conn,
                            "workspace",
                            &workspace_id,
                            "orphan_preserved",
                            serde_json::json!({ "reason": reason }),
                        )
                    })
                    .await;
                report.errors.push(format!(
                    "workspace {} was not safe to remove: {error}",
                    orphan.id
                ));
            }
        }
    }
    report
}

/// Runtime counterpart of boot reconciliation. Four clocks remain distinct in
/// the journal; review/human waiting can therefore be tuned without pretending
/// that an agent was inactive, and a total campaign deadline cannot be reset by
/// noisy progress messages.
pub fn start_resilience_watchdog(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let expired = match state
                .db
                .with_conn(|conn| {
                    crate::db::orchestration::expired_execution_timeouts(conn, chrono::Utc::now())
                })
                .await
            {
                Ok(expired) => expired,
                Err(error) => {
                    tracing::warn!("orchestration timeout scan failed: {error}");
                    continue;
                }
            };
            for (execution_id, kind) in expired {
                let keys = {
                    let id = execution_id.clone();
                    state
                        .db
                        .with_conn(move |conn| {
                            Ok(
                                crate::db::orchestration::get_task_execution(conn, &id)?.map(
                                    |execution| {
                                        (execution.dispatch_job_id, execution.sub_discussion_id)
                                    },
                                ),
                            )
                        })
                        .await
                        .ok()
                        .flatten()
                };
                if matches!(
                    kind,
                    crate::models::ExecutionTimeoutKind::Activity
                        | crate::models::ExecutionTimeoutKind::TotalDuration
                ) {
                    if let (Some((dispatch, child)), Ok(registry)) =
                        (keys, state.cancel_registry.lock())
                    {
                        for key in [dispatch.as_deref(), child.as_deref()]
                            .into_iter()
                            .flatten()
                        {
                            if let Some(token) = registry.get(key) {
                                token.cancel();
                            }
                        }
                    }
                }
                let id = execution_id.clone();
                if let Err(error) = state
                    .db
                    .with_conn(move |conn| {
                        crate::db::orchestration::apply_execution_timeout(conn, &id, kind)
                    })
                    .await
                {
                    tracing::warn!(
                        execution_id,
                        ?kind,
                        "orchestration timeout apply failed: {error}"
                    );
                }
            }

            // A disappeared provider process has no activity event to advance
            // the execution clocks. The dispatch token is the honest liveness
            // source: after a short grace period, absence means the run died.
            let candidates = state
                .db
                .with_conn(|conn| {
                    crate::db::agent_dispatch::list_watchdog_candidates(
                        conn,
                        chrono::Utc::now() - chrono::Duration::seconds(60),
                    )
                })
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!("agent dispatch watchdog scan failed: {error}");
                    Vec::new()
                });
            for job in candidates {
                let is_live = state
                    .cancel_registry
                    .lock()
                    .ok()
                    .is_some_and(|registry| registry.contains_key(&job.id));
                if is_live || job.failure_kind.as_deref() == Some("quota_exhausted") {
                    continue;
                }
                let job_id = job.id.clone();
                let transition = state
                    .db
                    .with_conn(move |conn| {
                        let transition =
                            crate::db::agent_dispatch::apply_watchdog_stall(conn, &job_id)?;
                        if transition == crate::db::agent_dispatch::WatchdogTransition::Escalated {
                            crate::db::agent_jobs::mark_completion_dispatch_failure(
                                conn,
                                &job_id,
                                crate::models::AgentResumeJobStatus::Escalated,
                                crate::models::AgentResumeFailureKind::DispatchStalled,
                                "completion dispatch stalled after one watchdog redispatch",
                            )?;
                        }
                        Ok(transition)
                    })
                    .await;
                match transition {
                    Ok(crate::db::agent_dispatch::WatchdogTransition::Redispatched) => {
                        tracing::warn!(dispatch_job_id = %job.id, "dead agent dispatch requeued once by watchdog");
                        state.agent_dispatch_notify.notify_one();
                    }
                    Ok(crate::db::agent_dispatch::WatchdogTransition::Escalated) => {
                        tracing::error!(dispatch_job_id = %job.id, "agent dispatch stalled again; human intervention required");
                        let discussion_id = job.discussion_id.clone();
                        let message = orchestrator_message(
                            format!("agent-dispatch-watchdog:{}", job.id),
                            format!(
                                "**Intervention humaine requise — agent bloqué**\n\nLe dispatch `{}` n'a plus de processus vivant après son unique relance automatique. Kronn ne le relancera plus à l'aveugle. Vérifie le provider, puis relance ou réaffecte explicitement.",
                                job.id
                            ),
                        );
                        if let Err(error) = state
                            .db
                            .with_conn(move |conn| {
                                crate::db::discussions::insert_message(
                                    conn,
                                    &discussion_id,
                                    &message,
                                )?;
                                Ok(())
                            })
                            .await
                        {
                            tracing::warn!("could not post dispatch stall escalation: {error}");
                        }
                    }
                    Ok(crate::db::agent_dispatch::WatchdogTransition::Unchanged) => {}
                    Err(error) => {
                        tracing::warn!(dispatch_job_id = %job.id, "dispatch watchdog transition failed: {error}")
                    }
                }
            }
        }
    });
}

async fn classify_interrupted_execution(
    db: &Database,
    exec_id: &str,
    available_agents: &[AgentType],
) -> Result<()> {
    let id = exec_id.to_string();
    let (
        execution,
        run,
        workspace,
        parent_active,
        child_exists,
        cli_available,
        project_path,
        prior_recovery,
    ) = db
        .with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &id)?
                .context("interrupted execution vanished")?;
            let run = crate::db::orchestration::get_orchestration_run(
                conn,
                &execution.orchestration_run_id,
            )?
            .context("orchestration run vanished")?;
            let workspace = crate::db::discussion_workspaces::get_managed_for_execution(conn, &id)?;
            let parent_active =
                crate::db::discussions::get_discussion(conn, &execution.parent_discussion_id)?
                    .is_some_and(|discussion| !discussion.archived);
            let child_exists = match execution.sub_discussion_id.as_deref() {
                Some(child) => crate::db::discussions::get_discussion(conn, child)?
                    .is_some_and(|discussion| !discussion.archived),
                None => false,
            };
            let cli_available = match (
                execution.worker_target_kind,
                execution.worker_cli_session_id,
                execution.worker_agent_type.as_deref(),
            ) {
                (Some(MessageTargetKind::Cli), Some(session), Some(agent_type)) => conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM discussion_sessions \
                     WHERE id = ?1 AND disc_id = ?2 AND agent_type = ?3 \
                       AND status <> 'left')",
                    rusqlite::params![session, execution.parent_discussion_id, agent_type],
                    |row| row.get(0),
                )?,
                (Some(MessageTargetKind::Cli), _, _) => false,
                _ => true,
            };
            let project_path = run
                .project_id
                .as_deref()
                .map(|project_id| crate::db::projects::get_project(conn, project_id))
                .transpose()?
                .flatten()
                .map(|project| project.path);
            let prior_recovery = crate::db::orchestration::get_execution_recovery(conn, &id)?;
            Ok((
                execution,
                run,
                workspace,
                parent_active,
                child_exists,
                cli_available,
                project_path,
                prior_recovery,
            ))
        })
        .await?;

    let interrupted_origin = execution
        .interrupted_from_status
        .context("Interrupted execution has no durable origin")?;
    // An interrupted hold has two checkpoints: Interrupted -> Blocked first,
    // then Blocked -> its real owner state. Classification must see through
    // that nested hold without letting the state-machine resume skip it.
    let origin = if interrupted_origin == TaskExecutionStatus::Blocked {
        execution
            .blocked_from_status
            .context("Interrupted Blocked execution has no durable blocked origin")?
    } else {
        interrupted_origin
    };
    let native_agent_available = if execution.worker_target_kind == Some(MessageTargetKind::Cli) {
        true
    } else {
        execution
            .worker_agent_type
            .as_deref()
            .map(crate::db::orchestration::agent_type_from_db)
            .transpose()?
            .is_none_or(|provider| available_agents.contains(&provider))
    };
    let needs_child = !matches!(
        origin,
        TaskExecutionStatus::Pending | TaskExecutionStatus::Provisioning
    );
    let needs_workspace = matches!(
        origin,
        TaskExecutionStatus::Working
            | TaskExecutionStatus::AwaitingReview
            | TaskExecutionStatus::Approved
            | TaskExecutionStatus::ChangesRequested
            | TaskExecutionStatus::Integrating
            | TaskExecutionStatus::Validating
            | TaskExecutionStatus::Applying
    );

    let durable_human_gate = prior_recovery.as_ref().filter(|recovery| {
        recovery.recovery_action == ExecutionRecoveryAction::AwaitHuman
            && (recovery
                .recovery_reason
                .starts_with("worker_failed_without_delivery")
                || recovery
                    .recovery_reason
                    .starts_with("worker_completed_without_delivery")
                || recovery.recovery_reason.starts_with("quota_exhausted:"))
    });

    let (action, reason) = if !parent_active {
        (
            ExecutionRecoveryAction::BlockMissingDiscussion,
            "the principal discussion is archived or unavailable".to_string(),
        )
    } else if needs_child && !child_exists {
        (
            ExecutionRecoveryAction::BlockMissingDiscussion,
            "the worker sub-discussion was deleted or is unavailable".to_string(),
        )
    } else if needs_workspace
        && workspace
            .as_ref()
            .and_then(|value| value.canonical_path.as_deref())
            .is_none_or(|path| !scanner::resolve_host_path(path).exists())
    {
        (
            ExecutionRecoveryAction::BlockMissingWorkspace,
            "the managed task worktree is missing".to_string(),
        )
    } else if !cli_available {
        (
            ExecutionRecoveryAction::BlockAgentUnavailable,
            "the exact CLI worker session is no longer available; reassign explicitly".to_string(),
        )
    } else if !native_agent_available {
        (
            ExecutionRecoveryAction::BlockAgentUnavailable,
            "the assigned native/HTTP agent is disabled, unauthenticated or unavailable; reassign explicitly"
            .to_string(),
        )
    } else if let Some(gate) = durable_human_gate {
        // A provider/worker failure was already settled deliberately and the
        // execution was parked for a person. Its Working origin describes
        // where it failed, not permission to call the same provider again on
        // every backend restart. Preserve the human gate; only an explicit
        // reassignment may create the next dispatch.
        (
            ExecutionRecoveryAction::AwaitHuman,
            gate.recovery_reason.clone(),
        )
    } else if matches!(
        origin,
        TaskExecutionStatus::Integrating
            | TaskExecutionStatus::Validating
            | TaskExecutionStatus::Applying
    ) {
        match (project_path.as_deref(), run.target_branch.as_deref()) {
            (None, _) => (
                ExecutionRecoveryAction::BlockMissingWorkspace,
                "the integration project no longer resolves; preserve checkpoints for repair"
                    .into(),
            ),
            (_, None) => (
                ExecutionRecoveryAction::AwaitHuman,
                "the integration target branch is no longer pinned".into(),
            ),
            (Some(project_path), Some(target)) => {
                let repo = scanner::resolve_host_path(project_path);
                let tip = worktree::resolve_commit(&repo, target).ok();
                let dirty = worktree::worktree_dirty_files(&repo)
                    .map(|files| !files.is_empty())
                    .unwrap_or(true);
                match crate::models::saga_resume_action(
                    origin,
                    execution.candidate_target_sha.as_deref(),
                    execution.candidate_merge_sha.as_deref(),
                    execution.integrated_sha.as_deref(),
                    tip.as_deref(),
                    dirty,
                ) {
                    SagaResumeAction::RebuildCandidate => (
                        ExecutionRecoveryAction::RebuildCandidate,
                        "Git target drifted or the candidate checkpoint is incomplete; rebuild from the real tip".into(),
                    ),
                    SagaResumeAction::RunValidations => (
                        ExecutionRecoveryAction::RunValidations,
                        "candidate exists at the pinned target; validations must be replayed".into(),
                    ),
                    SagaResumeAction::ApplyFastForward => (
                        ExecutionRecoveryAction::ApplyFastForward,
                        "validated candidate is pending a clean guarded fast-forward".into(),
                    ),
                    SagaResumeAction::IdempotentClose => (
                        ExecutionRecoveryAction::IdempotentClose,
                        "the real target already equals the candidate; close without replaying Git".into(),
                    ),
                    SagaResumeAction::BlockDirtyTarget => (
                        ExecutionRecoveryAction::BlockDirtyTarget,
                        "the target worktree is dirty; preserve both histories and wait".into(),
                    ),
                    SagaResumeAction::NoOp => (
                        ExecutionRecoveryAction::AwaitHuman,
                        "the integration checkpoint cannot be advanced automatically".into(),
                    ),
                }
            }
        }
    } else {
        match origin {
            TaskExecutionStatus::Pending
            | TaskExecutionStatus::Provisioning
            | TaskExecutionStatus::Blocked => (
                ExecutionRecoveryAction::ResumeProvisioning,
                format!(
                    "resume the provisioning checkpoint from {}",
                    origin.as_str()
                ),
            ),
            TaskExecutionStatus::Working | TaskExecutionStatus::ChangesRequested => (
                ExecutionRecoveryAction::ResumeWorker,
                format!(
                    "resume the same child discussion and worktree from {}",
                    origin.as_str()
                ),
            ),
            TaskExecutionStatus::AwaitingReview => (
                ExecutionRecoveryAction::AwaitReview,
                "delivery is durable; restore the principal review obligation".into(),
            ),
            TaskExecutionStatus::Approved => (
                ExecutionRecoveryAction::RebuildCandidate,
                "approval is durable; restart protected integration from the real target".into(),
            ),
            TaskExecutionStatus::Escalated => (
                ExecutionRecoveryAction::AwaitHuman,
                "the prior human gate remains authoritative after restart".into(),
            ),
            TaskExecutionStatus::Integrating
            | TaskExecutionStatus::Validating
            | TaskExecutionStatus::Applying
            | TaskExecutionStatus::Interrupted
            | TaskExecutionStatus::Done
            | TaskExecutionStatus::Failed
            | TaskExecutionStatus::Cancelled => unreachable!(),
        }
    };
    db.with_conn(move |conn| {
        crate::db::orchestration::set_execution_recovery(conn, &execution, &run, action, &reason)?;
        Ok(())
    })
    .await
}

// KT-400 — a handoff must match what the execution has actually produced.
// The "history is authoritative" wording was written for a worker interrupted
// MID-work; sent to one that never started, it reads as "go check what is
// already done" and produces an inventory instead of a commit. Measured on
// three consecutive generations of a real delegation: each answered "I'll
// resume by re-establishing the durable state" and wrote no code.
fn has_recorded_delivery(conn: &rusqlite::Connection, exec_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_execution_deliveries \
         WHERE task_execution_id = ?1)",
        [exec_id],
        |row| row.get(0),
    )
}

/// Result of checking git status for the handoff notice.
enum GitStatusForHandoff {
    /// Worktree has uncommitted changes; includes the count.
    Dirty(usize),
    /// Worktree is clean (no uncommitted changes).
    Clean,
    /// Could not determine git status (conservative case).
    InspectionFailed,
}

/// Check git status and return its state.
fn check_dirty_files_for_handoff(
    tx: &rusqlite::Connection,
    execution_id: &str,
) -> GitStatusForHandoff {
    use crate::core::worktree;
    use std::path::Path;

    let workspace =
        match crate::db::discussion_workspaces::get_managed_for_execution(tx, execution_id) {
            Ok(Some(ws)) => ws,
            _ => return GitStatusForHandoff::InspectionFailed,
        };

    let path = match workspace.canonical_path.as_deref() {
        Some(p) => p,
        None => return GitStatusForHandoff::InspectionFailed,
    };

    match worktree::worktree_dirty_files(Path::new(path)) {
        Ok(dirty_files) => {
            if dirty_files.is_empty() {
                GitStatusForHandoff::Clean
            } else {
                GitStatusForHandoff::Dirty(dirty_files.len())
            }
        }
        Err(_) => GitStatusForHandoff::InspectionFailed,
    }
}

/// The notice handed to a worker being woken or reassigned. `generation` is
/// `None` for a plain resume; a reassignment numbers its handoffs.
/// If `tx` and `execution_id` are provided, the message will reflect any uncommitted
/// changes in the worktree; otherwise a generic message is used.
#[allow(dead_code)]
fn handoff_notice(has_delivered: bool, generation: Option<u32>) -> String {
    handoff_notice_with_context(has_delivered, generation, None, None)
}

fn handoff_notice_with_context(
    has_delivered: bool,
    generation: Option<u32>,
    tx: Option<&rusqlite::Connection>,
    execution_id: Option<&str>,
) -> String {
    let suffix = match generation {
        Some(number) => format!(" — génération {number}"),
        None => String::new(),
    };
    if has_delivered {
        format!(
            "**Reprise{suffix}**\n\n\
             Reprends exactement l'étape inachevée dans ce même worktree. \
             L'historique, les manifests, les constats et les SHA déjà persistés \
             restent autoritatifs ; ne rejoue pas une action déjà prouvée."
        )
    } else {
        let git_status = match (tx, execution_id) {
            (Some(tx), Some(exec_id)) => Some(check_dirty_files_for_handoff(tx, exec_id)),
            _ => None,
        };

        match git_status {
            Some(GitStatusForHandoff::Dirty(file_count)) => {
                format!(
                    "**Démarre la tâche{suffix}**\n\n\
                     Du travail non commité t'attend dans ce worktree ({file_count} fichier{}).\n\
                     Relire le `git_diff` de ce qui précède, complète, puis utilise l'outil de commit déclaré \
                     (`task_exec_commit` pour un host CLI, `git_commit` pour un agent HTTP) avec un message \
                     qui explique le contexte — ce que la génération précédente laissait inachevé et \
                     pourquoi tu le termines. Ensuite livre avec `task_exec_deliver`.",
                    if file_count == 1 { "" } else { "s" }
                )
            }
            Some(GitStatusForHandoff::InspectionFailed) => {
                format!(
                    "**Démarre la tâche{suffix}**\n\n\
                     L'inspection du worktree n'a pas pu vérifier complètement son état. \
                     Vérifier d'abord avec `git status` et `git diff` si du travail t'attend : \
                     relire les changements, compléter si besoin, puis procéder en conséquence."
                )
            }
            _ => {
                // Either no database context (None) or Clean status
                format!(
                    "**Démarre la tâche{suffix}**\n\n\
                     Aucun travail n'a encore été enregistré pour cette exécution : \
                     pas de livraison, rien à reprendre.\n\n\
                     Le chemin le plus court : `search_text` pour trouver l'endroit, \
                     `read_file` avec `offset` et `limit` pour lire cette seule région, \
                     `edit_file` pour remplacer exactement ce passage — jamais besoin de \
                     lire ou réécrire le fichier entier —, l'outil de commit déclaré \
                     (`task_exec_commit` ou `git_commit`), puis livre avec \
                     `task_exec_deliver`."
                )
            }
        }
    }
}

async fn wake_recovered_worker(db: &Database, exec_id: &str) -> Result<String> {
    let id = exec_id.to_string();
    db.with_conn(move |conn| {
        let tx = conn.unchecked_transaction()?;
        let execution = crate::db::orchestration::get_task_execution(&tx, &id)?
            .context("execution vanished before worker wake")?;
        let child = execution
            .sub_discussion_id
            .as_deref()
            .context("execution has no child discussion")?;
        if let Some(dispatch_id) = execution.dispatch_job_id.as_deref() {
            if crate::db::agent_dispatch::get(&tx, dispatch_id)?.is_some_and(|job| {
                matches!(
                    job.status,
                    crate::db::agent_dispatch::DispatchStatus::Pending
                        | crate::db::agent_dispatch::DispatchStatus::Running
                )
            }) {
                if execution.status != TaskExecutionStatus::Working {
                    crate::db::orchestration::transition_execution(
                        &tx,
                        &id,
                        TaskExecutionStatus::Working,
                        &backend_actor(),
                        serde_json::json!({ "recovery": "resume_worker" }),
                    )?;
                }
                tx.commit()?;
                return Ok("existing durable worker dispatch resumed".into());
            }
        }
        let target = worker_target_from_execution(&execution)?;
        let message_id = format!("orch-resume-worker:{}:{}", id, execution.attempt_no);
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1)",
            [&message_id],
            |row| row.get(0),
        )?;
        if !exists {
            let has_delivered = has_recorded_delivery(&tx, &id)?;
            let message = orchestrator_message(
                message_id,
                handoff_notice_with_context(has_delivered, None, Some(&tx), Some(&id)),
            );
            if target.kind == MessageTargetKind::Cli {
                crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
                    &tx,
                    child,
                    &message,
                    std::slice::from_ref(&target),
                    &[],
                    None,
                )?;
            } else {
                crate::db::discussions::insert_message(&tx, child, &message)?;
            }
        }
        if target.kind != MessageTargetKind::Cli {
            let dispatch_id = Uuid::new_v4().to_string();
            let dedupe_base = format!("orch-resume-worker:{}:{}", id, execution.attempt_no);
            let prior_status = tx
                .query_row(
                    "SELECT status FROM agent_dispatch_jobs WHERE dedupe_key = ?1",
                    [&dedupe_base],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            // The base key makes the first wake idempotent. A terminal row is
            // evidence that this wake is no longer active, not that the worker
            // resumed successfully. Reusing it after another backend restart
            // permanently stranded the execution in Interrupted. The UUID is
            // safe here because enqueue + execution attachment share this
            // transaction: a concurrent retry observes the newly attached
            // Pending/Running dispatch at the top of this function.
            let dedupe = if prior_status
                .as_deref()
                .is_some_and(|status| !matches!(status, "Pending" | "Running"))
            {
                format!("{dedupe_base}:retry:{}", Uuid::new_v4())
            } else {
                dedupe_base
            };
            let job = crate::db::agent_dispatch::enqueue_for_latest_user(
                &tx,
                crate::db::agent_dispatch::NewLatestUserDispatch {
                    id: &dispatch_id,
                    discussion_id: child,
                    dedupe_key: &dedupe,
                    agent_override: Some(&target.agent_type),
                    chain_prompt_ids: &[],
                    batch_item: None,
                    group_id: None,
                    group_concurrency_limit: None,
                },
            )?;
            if !matches!(
                job.status,
                crate::db::agent_dispatch::DispatchStatus::Pending
                    | crate::db::agent_dispatch::DispatchStatus::Running
            ) {
                bail!(
                    "worker recovery dedupe resolved to terminal dispatch {} ({:?})",
                    job.id,
                    job.status
                );
            }
            crate::db::orchestration::attach_execution_dispatch(&tx, &id, &job.id)?;
        }
        crate::db::orchestration::transition_execution(
            &tx,
            &id,
            TaskExecutionStatus::Working,
            &backend_actor(),
            serde_json::json!({ "recovery": "resume_worker" }),
        )?;
        tx.commit()?;
        Ok(if target.kind == MessageTargetKind::Cli {
            "exact CLI worker session was woken in the existing child discussion".into()
        } else {
            "replacement durable dispatch queued in the existing child discussion".into()
        })
    })
    .await
}

async fn wake_recovered_principal(db: &Database, exec_id: &str) -> Result<String> {
    let id = exec_id.to_string();
    db.with_conn(move |conn| {
        let tx = conn.unchecked_transaction()?;
        let execution = crate::db::orchestration::get_task_execution(&tx, &id)?
            .context("execution vanished before principal wake")?;
        if crate::db::agent_dispatch::has_active_for_discussion(
            &tx,
            &execution.parent_discussion_id,
        )? {
            tx.commit()?;
            return Ok("existing principal dispatch resumed".into());
        }
        let principal = crate::db::discussions::get_discussion(
            &tx,
            &execution.parent_discussion_id,
        )?
        .context("principal discussion vanished")?;
        let message_id = format!("orch-recovery-review:{}:{}", id, execution.attempt_no);
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1)",
            [&message_id],
            |row| row.get(0),
        )?;
        if !exists {
            let message = orchestrator_message(
                message_id,
                format!(
                    "**Revue à reprendre après redémarrage**\n\n\
                     L'exécution `{}` possède déjà une livraison durable. Relis le manifeste, \
                     les constats et les SHA persistés puis rends la décision de revue ; ne demande \
                     pas au worker de rejouer son travail.",
                    id
                ),
            );
            crate::db::discussions::insert_message(&tx, &execution.parent_discussion_id, &message)?;
        }
        let dispatch_id = Uuid::new_v4().to_string();
        let dedupe = format!("orch-recovery-review:{}:{}", id, execution.attempt_no);
        crate::db::agent_dispatch::enqueue_for_latest_user(
            &tx,
            crate::db::agent_dispatch::NewLatestUserDispatch {
                id: &dispatch_id,
                discussion_id: &execution.parent_discussion_id,
                dedupe_key: &dedupe,
                agent_override: Some(&principal.agent),
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )?;
        tx.commit()?;
        Ok("principal review obligation re-dispatched".into())
    })
    .await
}

/// Everything Phase A resolves and hands to the Git-interleaving phases.
struct Prepared {
    execution: TaskExecution,
    project_id: String,
    repo_path: String,
    task_reference: String,
    task_title: String,
    task_description: String,
    dod: Vec<PlanningDodItem>,
    /// The idempotent replay found an execution already past `Provisioning`
    /// (running/terminal) — the saga returns it untouched.
    already_launched: bool,
}

/// Launch a single ready task. A native worker drives to `Working` (its task
/// `InProgress`); a `Cli` worker opens a durable control offer in the origin room
/// and parks `Blocked(awaiting_worker_acceptance)` until the exact session accepts
/// (KT-328 tranche 2). On any refusal/failure the returned error is structured and
/// the durable state is resumable (DoD-6).
pub async fn provision_single_task_execution(
    db: &Database,
    input: ProvisionInput,
) -> Result<TaskExecution, ProvisionError> {
    provision_task_execution_inner(db, input, None, None, None, Vec::new()).await
}

/// Launch one task with mechanical gates selected by the authorized principal.
/// The worker cannot influence this policy through its brief or delivery manifest.
pub async fn provision_single_task_execution_with_validations(
    db: &Database,
    input: ProvisionInput,
    validations: Vec<crate::models::ValidationSpec>,
) -> Result<TaskExecution, ProvisionError> {
    provision_task_execution_inner(db, input, None, None, None, validations).await
}

pub async fn provision_single_task_execution_with_scope_and_validations(
    db: &Database,
    input: ProvisionInput,
    worker_scope: Option<TaskWorkerScope>,
    validations: Vec<crate::models::ValidationSpec>,
) -> Result<TaskExecution, ProvisionError> {
    provision_task_execution_inner(db, input, None, None, worker_scope, validations).await
}

async fn resume_provisioning_execution(
    db: &Database,
    input: ProvisionInput,
    execution_id: &str,
) -> Result<TaskExecution, ProvisionError> {
    provision_task_execution_inner(db, input, None, Some(execution_id), None, Vec::new()).await
}

pub async fn provision_campaign_task_execution(
    db: &Database,
    input: CampaignProvisionInput,
) -> Result<(TaskExecution, String), ProvisionError> {
    let run_id = input.orchestration_run_id.clone();
    let worker_override = input.worker_override.clone();
    let (run, selection, selection_reason) = db
        .with_conn(move |conn| {
            let run = crate::db::orchestration::get_orchestration_run(conn, &run_id)?
                .context("orchestration run not found")?;
            let (selection, explanation) = crate::db::orchestration::resolve_campaign_worker(
                conn,
                &run,
                worker_override.as_ref(),
            )?;
            Ok((run, selection, explanation))
        })
        .await
        .map_err(|error| ProvisionError::NotLaunchable(error.to_string()))?;
    let context = CampaignLaunchContext {
        run_id: run.id,
        selection: selection.clone(),
    };
    let execution = provision_task_execution_inner(
        db,
        ProvisionInput {
            task_reference: input.task_reference,
            parent_discussion_id: run.discussion_id,
            worker: selection.target,
            base_rev: run.target_branch,
            idempotency_key: input.idempotency_key,
        },
        Some(context),
        None,
        None,
        Vec::new(),
    )
    .await?;
    Ok((execution, selection_reason))
}

async fn provision_task_execution_inner(
    db: &Database,
    input: ProvisionInput,
    campaign: Option<CampaignLaunchContext>,
    resume_execution_id: Option<&str>,
    worker_scope: Option<TaskWorkerScope>,
    validations: Vec<crate::models::ValidationSpec>,
) -> Result<TaskExecution, ProvisionError> {
    // Phases A–D are worker-agnostic: the worker kind forks ONLY at Phase E (native
    // dispatch vs. CLI control offer), so a Cli worker provisions its sub-discussion
    // and worktree exactly like a native one and diverges only at the final step.

    // ── Phase A — validation + idempotent launch → Provisioning (one commit) ──
    let prepared = {
        let task_ref = input.task_reference.clone();
        let parent = input.parent_discussion_id.clone();
        let worker = input.worker.clone();
        let target_branch = input.base_rev.clone();
        let idem = input.idempotency_key.clone();
        let campaign = campaign.clone();
        let validations = validations.clone();
        let worker_scope = worker_scope.clone();
        let resume_execution_id = resume_execution_id.map(str::to_string);
        db.with_conn(move |conn| {
            begin_provisioning(
                conn,
                &task_ref,
                &parent,
                &worker,
                BeginProvisioningContext {
                    target_branch: target_branch.as_deref(),
                    idempotency_key: idem.as_deref(),
                    validations: &validations,
                    campaign: campaign.as_ref(),
                    resume_execution_id: resume_execution_id.as_deref(),
                    worker_scope: worker_scope.as_ref(),
                },
                &backend_actor(),
            )
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?
    };
    let prepared = match prepared {
        Ok(p) => p,
        Err(refusal) => return Err(refusal),
    };
    if prepared.already_launched {
        return Ok(prepared.execution);
    }

    let repo_path = std::path::PathBuf::from(&prepared.repo_path);

    // ── Phase B — pin the base commit (git). Nothing external created yet, so a
    // failure only marks the execution resumable. ──
    let base_rev = input.base_rev.clone().unwrap_or_else(|| "main".to_string());
    let base_sha = match worktree::resolve_commit(&repo_path, &base_rev) {
        Ok(sha) => sha,
        Err(e) => {
            mark_blocked(
                db,
                &prepared.execution.id,
                format!("cannot pin base '{base_rev}': {e}"),
            )
            .await;
            return Err(ProvisionError::WorkspaceFailed {
                reason: e,
                compensated: true,
            });
        }
    };

    // ── Phase C — sub-discussion. Reuse the one THIS execution already links
    // (resume, keyed by the execution row — never a title search), else create a
    // fresh one so exactly one sub-discussion serves one execution (ADR §1). ──
    let sub_disc_id = match &prepared.execution.sub_discussion_id {
        Some(id) => id.clone(),
        None => {
            let disc = build_sub_discussion(&prepared, &input.worker);
            let sub_id = disc.id.clone();
            let exec_id = prepared.execution.id.clone();
            let sub_id_for_link = sub_id.clone();
            db.with_conn(move |conn| {
                // One SQLite transaction so a crash between the insert and the link
                // can never leave an orphan sub-discussion (mirrors Phase D3): the
                // sub-disc is created AND linked to this execution atomically, or not
                // at all. Resume is keyed by the execution row, so an all-or-nothing
                // commit here is what makes "reuse only what THIS execution created"
                // hold on the discussion side too.
                let tx = conn.unchecked_transaction()?;
                crate::db::discussions::insert_discussion(&tx, &disc)?;
                crate::db::orchestration::set_execution_sub_discussion(
                    &tx,
                    &exec_id,
                    &sub_id_for_link,
                )?;
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|e| ProvisionError::Internal(e.to_string()))?;
            sub_id
        }
    };

    // ── Phase D — managed workspace + worktree. Skip entirely if THIS execution
    // already links a workspace (resume). ──
    let (worktree_path, branch) = worktree::task_worktree_layout(
        &repo_path,
        &prepared.task_reference,
        exec_short(&prepared.execution.id),
    )
    .map_err(|e| ProvisionError::WorkspaceFailed {
        reason: e,
        compensated: false,
    })?;
    let worktree_path_str = worktree_path.to_string_lossy().to_string();

    if prepared.execution.workspace_id.is_none() {
        // (D1) Durable INTENT before the physical checkout (ADR §4bis): the managed
        // row records the exact deterministic path keyed by this execution, so a
        // crash here resumes from the row, never leaves an orphan worktree.
        let ws = {
            let exec_id = prepared.execution.id.clone();
            let sub = sub_disc_id.clone();
            let parent = input.parent_discussion_id.clone();
            let pid = prepared.project_id.clone();
            let tid = prepared.execution.task_id.clone();
            let path = worktree_path_str.clone();
            let br = branch.clone();
            let base = base_sha.clone();
            db.with_conn(move |conn| {
                crate::db::discussion_workspaces::upsert_managed(
                    conn,
                    &exec_id,
                    &sub,
                    &parent,
                    Some(&tid),
                    &pid,
                    &path,
                    &path,
                    &br,
                    &base,
                    &base,
                )
            })
            .await
            .map_err(|e| ProvisionError::Internal(e.to_string()))?
        };

        // (D2) Physical checkout from the EXACT pinned SHA. A prior attempt may
        // already have created it (resume): adopt it only after proving HEAD==base,
        // else create it fresh (fail-closed on any foreign/stale collision).
        let create_result = if worktree_path.exists() {
            worktree::verify_worktree_head(&worktree_path, &base_sha).map(|_| ())
        } else {
            worktree::create_task_worktree(
                &repo_path,
                &prepared.task_reference,
                exec_short(&prepared.execution.id),
                &base_sha,
            )
            .map(|_| ())
        };

        if let Err(e) = create_result {
            // (Compensation — physical then intent) remove ONLY what we own, then
            // the intent row; leave the execution resumable `Blocked`.
            let _ =
                worktree::remove_task_worktree(&repo_path, &worktree_path_str, &branch, &base_sha);
            let exec_id = prepared.execution.id.clone();
            let compensated = db
                .with_conn(move |conn| {
                    crate::db::discussion_workspaces::delete_managed_for_execution(conn, &exec_id)
                })
                .await
                .unwrap_or(false);
            mark_blocked(
                db,
                &prepared.execution.id,
                format!("worktree provisioning failed: {e}"),
            )
            .await;
            return Err(ProvisionError::WorkspaceFailed {
                reason: e,
                compensated,
            });
        }

        // (D3) Finalize atomically: link the workspace to the execution AND point
        // the sub-discussion at the worktree so the native worker runs inside the
        // isolated checkout.
        let exec_id = prepared.execution.id.clone();
        let ws_id = ws.id.clone();
        let sub = sub_disc_id.clone();
        let path = worktree_path_str.clone();
        let br = branch.clone();
        let base = base_sha.clone();
        db.with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            crate::db::orchestration::set_execution_workspace(&tx, &exec_id, &ws_id, &base, &br)?;
            crate::db::discussions::update_discussion_workspace(&tx, &sub, &path, &br)?;
            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?;
    }

    // A prelocalized scope is a mechanical promise, not prose. Validate it
    // against the exact SHA-pinned worktree before any worker dispatch exists.
    // The runner will still perform its own fresh read to obtain the edit CAS.
    if let Some(scope) = prepared.execution.worker_scope.as_ref() {
        if let Err(reason) = validate_worker_scope_in_worktree(&worktree_path, scope) {
            mark_blocked(
                db,
                &prepared.execution.id,
                format!("prelocalized worker scope refused: {reason}"),
            )
            .await;
            return Err(ProvisionError::WorkspaceFailed {
                reason,
                compensated: false,
            });
        }
    }

    // ── Phase E — fork on the durably-pinned worker kind. ──
    // Reconstruct the worker from the PERSISTED identity (pinned at launch), not
    // whatever the caller re-passed on a retry — the dispatched worker must match
    // exactly what was durably recorded.
    let target = worker_target_from_execution(&prepared.execution)
        .map_err(|e| ProvisionError::Internal(e.to_string()))?;

    // A joined CLI worker cannot be woken by a native dispatch inside the child (a
    // session owns exactly one room, and `wait_for_peer` only wakes in that room).
    // Instead of the native launchable-checkpoint, open a durable CONTROL OFFER in
    // the ORIGIN room targeted at the exact session and park the execution
    // `Blocked(awaiting_worker_acceptance)`; acceptance (KT-328 tranche 2) drives it
    // to Working. Phases A–D already provisioned its sub-disc + worktree.
    if matches!(target.kind, MessageTargetKind::Cli) {
        return open_cli_worker_control_offer(
            db,
            &prepared,
            &input.parent_discussion_id,
            &target,
            &sub_disc_id,
        )
        .await;
    }

    // ── Native worker: the single atomic "durably launchable" checkpoint. ──
    let brief = build_brief(&prepared, &worktree_path_str, &branch, &base_sha);
    let outcome = {
        let exec_id = prepared.execution.id.clone();
        let sub = sub_disc_id.clone();
        let task_ref = prepared.task_reference.clone();
        let attempt = prepared.execution.attempt_no;
        db.with_conn(move |conn| {
            crate::db::orchestration::commit_provisioning_checkpoint(
                conn,
                &ProvisioningCheckpoint {
                    exec_id: &exec_id,
                    sub_discussion_id: &sub,
                    task_reference: &task_ref,
                    attempt_no: attempt,
                    brief: &brief,
                    target: &target,
                    actor: &backend_actor(),
                },
            )
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?
    };

    match outcome {
        CheckpointOutcome::Committed { .. } => {
            let exec_id = prepared.execution.id.clone();
            db.with_conn(move |conn| {
                crate::db::orchestration::get_task_execution(conn, &exec_id)?
                    .context("execution vanished right after the provisioning checkpoint")
            })
            .await
            .map_err(|e| ProvisionError::Internal(e.to_string()))
        }
        CheckpointOutcome::TaskNotStarted(reason) => {
            let msg = checkpoint_refusal_reason(&reason);
            mark_blocked(
                db,
                &prepared.execution.id,
                format!("final checkpoint refused: {msg}"),
            )
            .await;
            Err(ProvisionError::CheckpointRefused(msg))
        }
        CheckpointOutcome::ExecutionRaced => Err(ProvisionError::CheckpointRefused(
            "execution raced out of Provisioning before the checkpoint".into(),
        )),
    }
}

fn validate_worker_scope_in_worktree(
    worktree: &std::path::Path,
    scope: &TaskWorkerScope,
) -> Result<(), String> {
    scope.validate()?;
    match scope {
        TaskWorkerScope::PrelocalizedEdit {
            path,
            start_line,
            end_line,
        } => {
            let limit = end_line.saturating_sub(*start_line).saturating_add(1) as usize;
            let payload = crate::api::agent_workspace_tools::read_file_payload(
                worktree,
                path,
                Some(*start_line as usize),
                Some(limit),
            )?;
            if !payload["found"].as_bool().unwrap_or(false) {
                return Err(format!("worker_scope target `{path}` does not exist"));
            }
            let total_lines = payload["total_lines"].as_u64().unwrap_or(0);
            if u64::from(*end_line) > total_lines {
                return Err(format!(
                    "worker_scope range {start_line}..={end_line} exceeds `{path}` ({total_lines} lines)"
                ));
            }
            Ok(())
        }
        TaskWorkerScope::PrelocalizedInsertAfter { path, anchor_line } => {
            let payload = crate::api::agent_workspace_tools::read_file_payload(
                worktree,
                path,
                Some(*anchor_line as usize),
                Some(1),
            )?;
            if !payload["found"].as_bool().unwrap_or(false) {
                return Err(format!("worker_scope target `{path}` does not exist"));
            }
            let total_lines = payload["total_lines"].as_u64().unwrap_or(0);
            if u64::from(*anchor_line) > total_lines {
                return Err(format!(
                    "worker_scope anchor {anchor_line} exceeds `{path}` ({total_lines} lines)"
                ));
            }
            Ok(())
        }
    }
}

/// What one integration attempt did (KT-320 DoD-3/7/8).
#[derive(Debug)]
pub enum IntegrationOutcome {
    /// The parent branch advanced onto the validated candidate.
    Integrated { sha: String },
    /// The candidate could not be built or did not validate. Branch, worktree and
    /// sub-discussion are untouched and the worker is back in the loop.
    SentBack { reason: String },
    /// A precondition was not met, so nothing was attempted at all.
    Refused { reason: String },
    /// The execution is not `Approved` — a replay of an integration already run
    /// lands here rather than doing it twice.
    NotIntegrable { status: TaskExecutionStatus },
}

/// Run the `TwoPhaseFfOnly` integration for an approved execution.
///
/// Every step records its checkpoint BEFORE acting, so a crash anywhere leaves the
/// row describing what was actually attempted and `saga_resume_action` can pick it
/// up. The parent is only ever fast-forwarded, and only after a verified backup ref
/// exists — there is no path here that rewrites its history.
pub async fn run_integration(
    db: &Database,
    exec_id: &str,
) -> Result<IntegrationOutcome, ProvisionError> {
    let internal = |e: String| ProvisionError::Internal(e);

    // ── Load the durable context ──
    let (execution, run, workspace) = {
        let id = exec_id.to_string();
        db.with_conn(move |conn| {
            let exec = crate::db::orchestration::get_task_execution(conn, &id)?;
            let Some(exec) = exec else {
                return Ok((None, None, None));
            };
            let run =
                crate::db::orchestration::get_orchestration_run(conn, &exec.orchestration_run_id)?;
            let ws = crate::db::discussion_workspaces::get_managed_for_execution(conn, &id)?;
            Ok((Some(exec), run, ws))
        })
        .await
        .map_err(|e| internal(e.to_string()))?
    };
    let Some(execution) = execution else {
        return Ok(IntegrationOutcome::Refused {
            reason: "unknown execution".into(),
        });
    };
    let Some(run) = run else {
        return Ok(IntegrationOutcome::Refused {
            reason: "orchestration run vanished".into(),
        });
    };
    if execution.status != TaskExecutionStatus::Approved {
        // The final DB checkpoint precedes physical cleanup. A crash in that
        // narrow gap re-enters with Done: finish the idempotent managed cleanup
        // instead of treating terminality as proof it already happened.
        if execution.status == TaskExecutionStatus::Done {
            if let (Some(integrated), Some(workspace), Some(project_id)) = (
                execution.integrated_sha.as_deref(),
                workspace.as_ref(),
                run.project_id.as_deref(),
            ) {
                if let Some(child_path) = workspace.canonical_path.as_deref() {
                    let pid = project_id.to_string();
                    if let Some(project_path) = db
                        .with_conn(move |conn| {
                            Ok(crate::db::projects::get_project(conn, &pid)?.map(|p| p.path))
                        })
                        .await
                        .map_err(|e| internal(e.to_string()))?
                    {
                        let repo_path = scanner::resolve_host_path(&project_path);
                        cleanup_integrated_execution(
                            db, &execution, &repo_path, child_path, integrated,
                        )
                        .await?;
                    }
                }
            }
        }
        return Ok(IntegrationOutcome::NotIntegrable {
            status: execution.status,
        });
    }

    // The target must be pinned: an unpinned branch is exactly the case where the
    // engine would guess which history to advance.
    let Some(target_branch) = run.target_branch.clone() else {
        return Ok(IntegrationOutcome::Refused {
            reason: "no pinned target branch".into(),
        });
    };
    let Some(project_id) = run.project_id.clone() else {
        return Ok(IntegrationOutcome::Refused {
            reason: "run has no project".into(),
        });
    };
    let Some(child_path) = workspace.and_then(|w| w.canonical_path) else {
        return Ok(IntegrationOutcome::Refused {
            reason: "no managed worktree".into(),
        });
    };

    let project_path = {
        let pid = project_id.clone();
        db.with_conn(move |conn| Ok(crate::db::projects::get_project(conn, &pid)?.map(|p| p.path)))
            .await
            .map_err(|e| internal(e.to_string()))?
    };
    let Some(project_path) = project_path else {
        return Ok(IntegrationOutcome::Refused {
            reason: "project vanished".into(),
        });
    };
    let repo_path = scanner::resolve_host_path(&project_path);
    let child = std::path::Path::new(&child_path);

    // ── Preflight: never apply over uncommitted work ──
    match worktree::worktree_dirty_files(&repo_path) {
        Ok(dirty) if !dirty.is_empty() => {
            return Ok(IntegrationOutcome::Refused {
                reason: format!("target has {} uncommitted file(s)", dirty.len()),
            });
        }
        Err(e) => return Ok(IntegrationOutcome::Refused { reason: e }),
        Ok(_) => {}
    }

    // ── Anchor: pin the tip the candidate is built on ──
    let target_sha = match worktree::resolve_commit(&repo_path, &target_branch) {
        Ok(sha) => sha,
        Err(e) => return Ok(IntegrationOutcome::Refused { reason: e }),
    };
    checkpoint(db, exec_id, CheckpointStep::Anchored(target_sha.clone())).await?;

    // ── Phase 1: build the candidate in the CHILD worktree ──
    let merge_sha = match worktree::build_candidate(child, &target_sha) {
        Ok(worktree::CandidateOutcome::Built { sha }) => sha,
        Ok(worktree::CandidateOutcome::Conflict { files }) => {
            send_back(
                db,
                exec_id,
                format!("merge conflict in {}", files.join(", ")),
            )
            .await?;
            return Ok(IntegrationOutcome::SentBack {
                reason: format!("conflict in {}", files.join(", ")),
            });
        }
        Err(e) => return Ok(IntegrationOutcome::Refused { reason: e }),
    };
    checkpoint(db, exec_id, CheckpointStep::Built(merge_sha.clone())).await?;

    // ── Validations: a failure sends the work back, it never blocks the parent ──
    checkpoint(db, exec_id, CheckpointStep::Validating).await?;
    for spec in &run.validations {
        let (code, ms, summary) = run_one_validation(spec, child).await;
        let recorded = {
            let (id, spec, merge) = (exec_id.to_string(), spec.clone(), merge_sha.clone());
            let summary = summary.clone();
            db.with_conn(move |conn| {
                crate::db::orchestration::record_validation_run(
                    conn,
                    &id,
                    Some(&merge),
                    &spec,
                    code,
                    Some(ms),
                    Some(&summary),
                )
            })
            .await
            .map_err(|e| internal(e.to_string()))?
        };
        if !recorded.passed() {
            send_back(db, exec_id, format!("validation failed: {}", spec.command)).await?;
            return Ok(IntegrationOutcome::SentBack {
                reason: format!("validation failed: {}", spec.command),
            });
        }
    }

    // ── Arm: the backup ref must exist and read back before the parent may move ──
    let slug = format!("{}-{}", execution.task_id, exec_short(&execution.id));
    let backup = match worktree::write_backup_ref(&repo_path, &slug, &target_sha) {
        Ok(r) => r,
        Err(e) => return Ok(IntegrationOutcome::Refused { reason: e }),
    };
    checkpoint(db, exec_id, CheckpointStep::Armed(backup)).await?;

    // ── Phase 2: advance the parent, fast-forward only ──
    let integrated = match worktree::fast_forward_to(&repo_path, &merge_sha) {
        Ok(sha) => sha,
        Err(e) => return Ok(IntegrationOutcome::Refused { reason: e }),
    };
    checkpoint(db, exec_id, CheckpointStep::Integrated(integrated.clone())).await?;

    cleanup_integrated_execution(db, &execution, &repo_path, &child_path, &integrated).await?;

    advance_campaign_after_integration(db, &run).await;

    Ok(IntegrationOutcome::Integrated { sha: integrated })
}

/// Consume one durable boot-recovery decision for the integration saga. Every
/// path re-checks the real target ref immediately before mutation; a recorded
/// checkpoint is evidence, never permission to replay Git blindly.
async fn resume_recovered_integration(
    db: &Database,
    exec_id: &str,
    action: ExecutionRecoveryAction,
) -> Result<IntegrationOutcome, ProvisionError> {
    let internal = |error: String| ProvisionError::Internal(error);
    let (execution, run, workspace, project_path) = {
        let id = exec_id.to_string();
        db.with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &id)?
                .context("execution vanished during recovery")?;
            let run = crate::db::orchestration::get_orchestration_run(
                conn,
                &execution.orchestration_run_id,
            )?
            .context("orchestration run vanished during recovery")?;
            let workspace = crate::db::discussion_workspaces::get_managed_for_execution(conn, &id)?
                .context("managed worktree vanished during recovery")?;
            let project_id = run.project_id.as_deref().context("run has no project")?;
            let project_path = crate::db::projects::get_project(conn, project_id)?
                .context("project vanished during recovery")?
                .path;
            Ok((execution, run, workspace, project_path))
        })
        .await
        .map_err(|error| internal(error.to_string()))?
    };
    let child_path = workspace
        .canonical_path
        .as_deref()
        .ok_or_else(|| internal("managed worktree has no canonical path".into()))?;
    let child = std::path::Path::new(child_path);
    let repo = scanner::resolve_host_path(&project_path);
    let target_branch = run
        .target_branch
        .as_deref()
        .ok_or_else(|| internal("run has no pinned target branch".into()))?;

    match action {
        ExecutionRecoveryAction::RebuildCandidate => {
            let target_sha = worktree::resolve_commit(&repo, target_branch)
                .map_err(|error| internal(format!("cannot resolve recovery target: {error}")))?;
            let id = exec_id.to_string();
            let anchor = target_sha.clone();
            db.with_conn(move |conn| {
                crate::db::orchestration::resume_rebuild_candidate(
                    conn,
                    &id,
                    &anchor,
                    &backend_actor(),
                )
            })
            .await
            .map_err(|error| internal(error.to_string()))?;
            let merge_sha = match worktree::build_candidate(child, &target_sha) {
                Ok(worktree::CandidateOutcome::Built { sha }) => sha,
                Ok(worktree::CandidateOutcome::Conflict { files }) => {
                    let reason = format!("merge conflict after recovery in {}", files.join(", "));
                    send_back(db, exec_id, reason.clone()).await?;
                    return Ok(IntegrationOutcome::SentBack { reason });
                }
                Err(error) => return Ok(IntegrationOutcome::Refused { reason: error }),
            };
            checkpoint(db, exec_id, CheckpointStep::Built(merge_sha)).await?;
            checkpoint(db, exec_id, CheckpointStep::Validating).await?;
        }
        ExecutionRecoveryAction::RunValidations => {
            let id = exec_id.to_string();
            db.with_conn(move |conn| {
                crate::db::orchestration::transition_execution(
                    conn,
                    &id,
                    TaskExecutionStatus::Validating,
                    &backend_actor(),
                    serde_json::json!({ "recovery": "run_validations" }),
                )?;
                Ok(())
            })
            .await
            .map_err(|error| internal(error.to_string()))?;
        }
        ExecutionRecoveryAction::ApplyFastForward => {
            return finish_recovered_apply(db, &execution, &run, &repo, child_path, true).await;
        }
        ExecutionRecoveryAction::IdempotentClose => {
            return finish_recovered_apply(db, &execution, &run, &repo, child_path, true).await;
        }
        ExecutionRecoveryAction::BlockDirtyTarget => {
            let id = exec_id.to_string();
            db.with_conn(move |conn| {
                crate::db::orchestration::block_execution(
                    conn,
                    &id,
                    &backend_actor(),
                    "target worktree is dirty after restart",
                    None,
                )?;
                Ok(())
            })
            .await
            .map_err(|error| internal(error.to_string()))?;
            return Ok(IntegrationOutcome::Refused {
                reason: "target worktree is dirty; execution parked".into(),
            });
        }
        other => {
            return Ok(IntegrationOutcome::Refused {
                reason: format!("{other:?} is not an integration recovery action"),
            })
        }
    }

    // Re-read after the rebuild checkpoints: the initial snapshot intentionally
    // contained the stale candidate that caused reconciliation.
    let recovered_execution = {
        let id = exec_id.to_string();
        db.with_conn(move |conn| crate::db::orchestration::get_task_execution(conn, &id))
            .await
            .map_err(|error| internal(error.to_string()))?
            .ok_or_else(|| internal("execution vanished after recovered candidate build".into()))?
    };
    let merge_sha = recovered_execution
        .candidate_merge_sha
        .clone()
        .ok_or_else(|| internal("recovery candidate SHA is missing".into()))?;
    if let Err(error) = worktree::verify_worktree_head(child, &merge_sha) {
        let reason = format!("candidate worktree drifted before recovered validation: {error}");
        send_back(db, exec_id, reason.clone()).await?;
        return Ok(IntegrationOutcome::SentBack { reason });
    }
    for spec in &run.validations {
        let (code, duration, summary) = run_one_validation(spec, child).await;
        let id = exec_id.to_string();
        let spec = spec.clone();
        let command = spec.command.clone();
        let candidate = merge_sha.clone();
        let summary_for_db = summary.clone();
        let recorded = db
            .with_conn(move |conn| {
                crate::db::orchestration::record_validation_run(
                    conn,
                    &id,
                    Some(&candidate),
                    &spec,
                    code,
                    Some(duration),
                    Some(&summary_for_db),
                )
            })
            .await
            .map_err(|error| internal(error.to_string()))?;
        if !recorded.passed() {
            let reason = format!("recovered validation failed: {command}");
            send_back(db, exec_id, reason.clone()).await?;
            return Ok(IntegrationOutcome::SentBack { reason });
        }
    }
    let target_sha = recovered_execution
        .candidate_target_sha
        .as_deref()
        .ok_or_else(|| internal("candidate target SHA is missing".into()))?;
    let backup = worktree::write_backup_ref(
        &repo,
        &format!("{}-{}", recovered_execution.task_id, exec_short(exec_id)),
        target_sha,
    )
    .map_err(|error| internal(format!("cannot arm recovered apply: {error}")))?;
    checkpoint(db, exec_id, CheckpointStep::Armed(backup)).await?;
    finish_recovered_apply(db, &recovered_execution, &run, &repo, child_path, false).await
}

async fn finish_recovered_apply(
    db: &Database,
    execution: &TaskExecution,
    run: &crate::models::OrchestrationRun,
    repo: &std::path::Path,
    child_path: &str,
    claim_apply: bool,
) -> Result<IntegrationOutcome, ProvisionError> {
    let internal = |error: String| ProvisionError::Internal(error);
    let target_branch = run
        .target_branch
        .as_deref()
        .ok_or_else(|| internal("run has no pinned target branch".into()))?;
    let target_sha = execution
        .candidate_target_sha
        .as_deref()
        .ok_or_else(|| internal("candidate target SHA is missing".into()))?;
    let merge_sha = execution
        .candidate_merge_sha
        .as_deref()
        .ok_or_else(|| internal("candidate merge SHA is missing".into()))?;
    let real_tip = match worktree::resolve_commit(repo, target_branch) {
        Ok(real_tip) => real_tip,
        Err(error) => {
            let id = execution.id.clone();
            let execution = execution.clone();
            let run = run.clone();
            let reason = format!("cannot verify target before recovered apply: {error}");
            let durable_reason = reason.clone();
            db.with_conn(move |conn| {
                crate::db::orchestration::set_execution_recovery(
                    conn,
                    &execution,
                    &run,
                    ExecutionRecoveryAction::AwaitHuman,
                    &durable_reason,
                )?;
                crate::db::orchestration::record_reconciliation_event(
                    conn,
                    "execution",
                    &id,
                    "apply_recheck_unavailable",
                    serde_json::json!({ "reason": durable_reason }),
                )
            })
            .await
            .map_err(|db_error| internal(db_error.to_string()))?;
            return Err(internal(reason));
        }
    };
    let integrated = if real_tip == merge_sha {
        // Git already landed before the old process died. Do not replay it.
        merge_sha.to_string()
    } else {
        if real_tip != target_sha {
            let id = execution.id.clone();
            let execution = execution.clone();
            let run = run.clone();
            let reason =
                format!("target drifted from {target_sha} to {real_tip}; rebuild the candidate");
            let durable_reason = reason.clone();
            db.with_conn(move |conn| {
                crate::db::orchestration::set_execution_recovery(
                    conn,
                    &execution,
                    &run,
                    ExecutionRecoveryAction::RebuildCandidate,
                    &durable_reason,
                )?;
                crate::db::orchestration::record_reconciliation_event(
                    conn,
                    "execution",
                    &id,
                    "apply_recheck_drifted",
                    serde_json::json!({ "real_tip": real_tip, "reason": durable_reason }),
                )
            })
            .await
            .map_err(|error| internal(error.to_string()))?;
            return Err(internal(reason));
        }
        let dirty = worktree::worktree_dirty_files(repo)
            .map_err(|error| internal(format!("cannot inspect target cleanliness: {error}")))?;
        if !dirty.is_empty() {
            let id = execution.id.clone();
            db.with_conn(move |conn| {
                crate::db::orchestration::block_execution(
                    conn,
                    &id,
                    &backend_actor(),
                    "target worktree became dirty before recovered apply",
                    None,
                )?;
                Ok(())
            })
            .await
            .map_err(|error| internal(error.to_string()))?;
            return Ok(IntegrationOutcome::Refused {
                reason: "target became dirty; execution parked".into(),
            });
        }
        if claim_apply {
            let id = execution.id.clone();
            let claimed = db
                .with_conn(move |conn| {
                    crate::db::orchestration::transition_execution(
                        conn,
                        &id,
                        TaskExecutionStatus::Applying,
                        &backend_actor(),
                        serde_json::json!({ "recovery": "guarded_apply_recheck_complete" }),
                    )
                })
                .await
                .map_err(|error| internal(error.to_string()))?;
            require_recovered_apply_claim(claimed)?;
        }
        match worktree::fast_forward_to(repo, merge_sha) {
            Ok(sha) => sha,
            Err(error) => {
                let reason = format!("recovered fast-forward refused: {error}");
                interrupt_recovered_apply(db, &execution.id, &reason).await?;
                return Err(internal(reason));
            }
        }
    };
    if real_tip == merge_sha && claim_apply {
        let id = execution.id.clone();
        let claimed = db
            .with_conn(move |conn| {
                crate::db::orchestration::transition_execution(
                    conn,
                    &id,
                    TaskExecutionStatus::Applying,
                    &backend_actor(),
                    serde_json::json!({ "recovery": "idempotent_close_recheck_complete" }),
                )
            })
            .await
            .map_err(|error| internal(error.to_string()))?;
        require_recovered_apply_claim(claimed)?;
    }
    if let Err(error) = checkpoint(
        db,
        &execution.id,
        CheckpointStep::Integrated(integrated.clone()),
    )
    .await
    {
        let reason = format!("integrated Git state could not be checkpointed: {error:?}");
        if let Err(interrupt_error) = interrupt_recovered_apply(db, &execution.id, &reason).await {
            tracing::warn!(
                execution_id = %execution.id,
                error = ?interrupt_error,
                "could not park a recovered apply after its integration checkpoint failed"
            );
        }
        return Err(error);
    }
    cleanup_integrated_execution(db, execution, repo, child_path, &integrated).await?;
    advance_campaign_after_integration(db, run).await;
    Ok(IntegrationOutcome::Integrated { sha: integrated })
}

/// A recovered apply may touch the shared parent checkout only after winning
/// the durable Blocked/Interrupted -> Applying claim. `transition_execution`
/// deliberately reports a lost compare-and-swap as `Ok(false)`; treating that
/// as success would let two recovery requests run Git against the same target.
fn require_recovered_apply_claim(claimed: bool) -> Result<(), ProvisionError> {
    if claimed {
        Ok(())
    } else {
        Err(ProvisionError::CheckpointRefused(
            "recovered apply was already claimed by another caller".into(),
        ))
    }
}

async fn interrupt_recovered_apply(
    db: &Database,
    exec_id: &str,
    reason: &str,
) -> Result<(), ProvisionError> {
    let id = exec_id.to_string();
    let reason = reason.to_string();
    db.with_conn(move |conn| {
        crate::db::orchestration::transition_execution(
            conn,
            &id,
            TaskExecutionStatus::Interrupted,
            &backend_actor(),
            serde_json::json!({ "reason": reason, "recovery": "apply_retry" }),
        )?;
        Ok(())
    })
    .await
    .map_err(|error| ProvisionError::Internal(error.to_string()))
}

/// Continue a campaign only after a clean success and only while no human gate
/// exists. A launch failure never rewrites the already-integrated result: it
/// parks the campaign durably for inspection instead.
async fn advance_campaign_after_integration(db: &Database, run: &crate::models::OrchestrationRun) {
    use crate::models::{OrchestrationControlState, OrchestrationRunKind, PlanningTaskStatus};

    if run.kind != OrchestrationRunKind::Campaign
        || run.control_state != OrchestrationControlState::Running
    {
        return;
    }
    let run_id = run.id.clone();
    let snapshot = db
        .with_conn(move |conn| {
            let current = crate::db::orchestration::get_orchestration_run(conn, &run_id)?
                .context("campaign vanished after integration")?;
            let attention = crate::db::orchestration::principal_attention(conn, &run_id)?;
            let next = crate::db::orchestration::campaign_task_candidates(conn, &run_id, None)?
                .into_iter()
                .find(|candidate| candidate.launchable)
                .map(|candidate| candidate.task.reference);
            let plan = crate::db::planning::get_discussion_plan(conn, &current.discussion_id)?;
            let all_active_done = plan.active.iter().all(|relation| {
                matches!(
                    relation.task.status,
                    PlanningTaskStatus::Done | PlanningTaskStatus::Archived
                )
            });
            Ok((current, attention, next, all_active_done))
        })
        .await;
    let Ok((current, attention, next, all_active_done)) = snapshot else {
        tracing::error!(run_id = %run.id, "could not inspect campaign after integration");
        return;
    };
    if current.control_state != OrchestrationControlState::Running {
        return;
    }
    if attention.awaiting_human > 0 {
        park_campaign_for_human(
            db,
            &current.id,
            "a child execution requires a human decision",
        )
        .await;
        return;
    }
    if let Some(task_reference) = next {
        if !current.auto_continue {
            return;
        }
        let idempotency_key = Some(format!("campaign-auto:{}:{task_reference}", current.id));
        if let Err(error) = provision_campaign_task_execution(
            db,
            CampaignProvisionInput {
                orchestration_run_id: current.id.clone(),
                task_reference,
                worker_override: None,
                idempotency_key,
            },
        )
        .await
        {
            park_campaign_for_human(
                db,
                &current.id,
                &format!("automatic next-task launch failed: {error:?}"),
            )
            .await;
        }
        return;
    }
    if attention.active_executions > 0 {
        return;
    }
    let state = if all_active_done {
        OrchestrationControlState::Completed
    } else {
        OrchestrationControlState::AwaitingHuman
    };
    let reason = if all_active_done {
        "all active plan tasks are complete"
    } else {
        "no launchable task remains; the plan is blocked or needs correction"
    };
    let id = current.id.clone();
    let reason = reason.to_string();
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::orchestration::set_orchestration_control_state(
                conn,
                &id,
                state,
                Some(&reason),
                &backend_actor(),
            )?;
            Ok(())
        })
        .await
    {
        tracing::error!(run_id = %current.id, %error, "could not close campaign after integration");
    }
}

async fn park_campaign_for_human(db: &Database, run_id: &str, reason: &str) {
    let id = run_id.to_string();
    let reason = reason.to_string();
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::orchestration::set_orchestration_control_state(
                conn,
                &id,
                crate::models::OrchestrationControlState::AwaitingHuman,
                Some(&reason),
                &backend_actor(),
            )?;
            Ok(())
        })
        .await
    {
        tracing::error!(run_id, %error, "could not park campaign for human attention");
    }
}

/// Physical-first, ownership-checked post-success cleanup. Idempotent so a
/// `Done` replay can close the crash window between the terminal checkpoint and
/// deleting the checkout/intent row. Divergent evidence is always preserved.
async fn cleanup_integrated_execution(
    db: &Database,
    execution: &TaskExecution,
    repo_path: &std::path::Path,
    child_path: &str,
    integrated_sha: &str,
) -> Result<(), ProvisionError> {
    let Some(branch) = execution.child_branch.as_deref() else {
        return Ok(());
    };
    match worktree::remove_integrated_task_worktree(repo_path, child_path, branch, integrated_sha) {
        Ok(()) => {
            // Never erase the ownership proof while a checkout still exists.
            // Once Git cleanup is confirmed, retire the intent + stale child
            // coordinates atomically; the transcript/sub-discussion survives.
            let id = execution.id.clone();
            let child_discussion = execution.sub_discussion_id.clone();
            db.with_conn(move |conn| {
                let tx = conn.unchecked_transaction()?;
                if let Some(child_discussion) = child_discussion.as_deref() {
                    crate::db::discussions::clear_discussion_workspace(&tx, child_discussion)?;
                }
                let delivered_head = crate::db::worker_deliveries::list_deliveries(&tx, &id)?
                    .last()
                    .map(|delivery| delivery.head_sha.clone());
                crate::db::discussion_workspaces::retire_managed_for_execution(
                    &tx,
                    &id,
                    delivered_head.as_deref(),
                )?;
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|e| {
                ProvisionError::Internal(format!("integrated worktree DB cleanup failed: {e}"))
            })?;
        }
        Err(reason) => {
            tracing::warn!(
                execution_id = %execution.id,
                worktree = %child_path,
                "integrated worktree preserved: {reason}"
            );
            // KT-373 — a preserved worktree is exactly how the 2026-08-21 disk
            // filled: seven of them, kept on purpose because they still held
            // unintegrated work, each carrying 7.5 to 24.6 GiB of Rust build
            // artefacts. The work is in the sources and Git; `target/` is
            // rebuildable and worth nothing. Reclaiming it keeps the reason the
            // worktree was preserved intact.
            reclaim_preserved_worktree_artifacts(db, repo_path, child_path).await;
        }
    }
    Ok(())
}

/// Give back the build artefacts of a worktree that was deliberately kept.
///
/// Never fails the caller: the integration it rides on has already succeeded,
/// and a refusal to reclaim disk is not a reason to report that integration as
/// broken. Every refusal is logged with its reason, because a cleanup that
/// quietly does nothing is how a disk fills again.
async fn reclaim_preserved_worktree_artifacts(
    db: &Database,
    repo_path: &std::path::Path,
    child_path: &str,
) {
    let canonical = child_path.to_string();
    let liveness = match db
        .with_conn(move |conn| {
            crate::db::orchestration::worktree_cleanup_liveness(conn, &canonical)
        })
        .await
    {
        Ok(liveness) => liveness,
        Err(error) => {
            tracing::warn!(worktree = %child_path, "artefact reclaim skipped: {error}");
            return;
        }
    };
    let outcome = match worktree::clean_worktree_build_artifacts(
        repo_path,
        std::path::Path::new(child_path),
        liveness,
    ) {
        Ok(report) => {
            if report.bytes_reclaimed > 0 {
                tracing::info!(
                    worktree = %child_path,
                    bytes = report.bytes_reclaimed,
                    partial = report.bytes_are_partial,
                    "reclaimed build artefacts from a preserved worktree"
                );
            }
            Ok((report.bytes_reclaimed, report.bytes_are_partial))
        }
        Err(reason) => {
            tracing::info!(worktree = %child_path, "artefacts kept: {reason}");
            Err(reason)
        }
    };
    // The durable half: logs answer "what happened" while this process lives,
    // the event answers it afterwards. A refusal is recorded too — a disk that
    // stayed full because cleanup was declined looks nothing like one nobody
    // tried to clean, and only the record tells them apart.
    let canonical = child_path.to_string();
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::orchestration::record_artifact_reclaim(conn, &canonical, outcome)
        })
        .await
    {
        tracing::warn!(worktree = %child_path, "artefact reclaim audit failed: {error}");
    }
}

enum CheckpointStep {
    Anchored(String),
    Built(String),
    Validating,
    Armed(String),
    Integrated(String),
}

/// Commit one saga checkpoint, mapping a lost race to an internal error: the caller
/// is mid-integration and must not carry on against a row that moved.
async fn checkpoint(
    db: &Database,
    exec_id: &str,
    step: CheckpointStep,
) -> Result<(), ProvisionError> {
    use crate::db::orchestration::{commit_integration_checkpoint, IntegrationStep};
    let id = exec_id.to_string();
    let outcome = db
        .with_conn(move |conn| {
            let step = match &step {
                CheckpointStep::Anchored(sha) => {
                    IntegrationStep::CandidateAnchored { target_sha: sha }
                }
                CheckpointStep::Built(sha) => IntegrationStep::CandidateBuilt { merge_sha: sha },
                CheckpointStep::Validating => IntegrationStep::ValidationsStarted,
                CheckpointStep::Armed(r) => IntegrationStep::ApplyArmed { backup_ref: r },
                CheckpointStep::Integrated(sha) => IntegrationStep::Integrated {
                    integrated_sha: sha,
                },
            };
            commit_integration_checkpoint(conn, &id, step, &backend_actor())
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?;
    match outcome {
        crate::db::orchestration::IntegrationCheckpointOutcome::Committed { .. } => Ok(()),
        other => Err(ProvisionError::Internal(format!(
            "integration checkpoint refused: {other:?}"
        ))),
    }
}

/// Hand the work back to the worker without touching branch, worktree or task.
async fn send_back(db: &Database, exec_id: &str, reason: String) -> Result<(), ProvisionError> {
    let id = exec_id.to_string();
    db.with_conn(move |conn| {
        crate::db::orchestration::transition_execution(
            conn,
            &id,
            TaskExecutionStatus::ChangesRequested,
            &backend_actor(),
            serde_json::json!({ "reason": reason }),
        )?;
        Ok(())
    })
    .await
    .map_err(|e| ProvisionError::Internal(e.to_string()))
}

/// Run one declared validation inside the child worktree, through the same
/// allowlist every other Kronn-run command goes through.
async fn run_one_validation(
    spec: &crate::models::ValidationSpec,
    cwd: &std::path::Path,
) -> (Option<i32>, i64, String) {
    let mut parts = spec.command.split_whitespace();
    let Some(binary) = parts.next() else {
        return (None, 0, "empty validation command".into());
    };
    use crate::core::quick_exec;
    let quick = quick_exec::QuickExecSpec {
        binary: binary.to_string(),
        argv: parts.map(str::to_string).collect(),
        cwd: cwd.to_path_buf(),
        timeout_secs: spec.timeout_secs.map(u64::from),
        // A validation reads nothing: leaving stdin open would make a command that
        // waits for input indistinguishable from one that hangs.
        stdin: None,
        summariser: match spec.command.split_whitespace().next() {
            Some("cargo") if spec.command.contains("clippy") => quick_exec::Summariser::Clippy,
            Some("cargo") => quick_exec::Summariser::CargoTest,
            Some("tsc") => quick_exec::Summariser::Tsc,
            Some("vitest") => quick_exec::Summariser::Vitest,
            _ => quick_exec::Summariser::Generic,
        },
    };
    let validated = match quick_exec::validate(&quick, &[cwd.to_path_buf()]) {
        Ok(v) => v,
        // A refused command is a failed validation, never a silent skip.
        Err(rejection) => return (None, 0, format!("refused: {rejection}")),
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    match quick_exec::run(&validated, None, &cancel).await {
        Ok(result) => (result.exit_code, result.duration_ms as i64, result.summary),
        Err(e) => (None, 0, format!("validation could not run: {e}")),
    }
}

fn checkpoint_refusal_reason(reason: &StartTaskCheckpoint) -> String {
    match reason {
        StartTaskCheckpoint::NotTodo => "task is no longer Todo".into(),
        StartTaskCheckpoint::BlockedByActive => "an active blocker appeared".into(),
        StartTaskCheckpoint::Started => "started".into(),
    }
}

/// Phase E for a joined-CLI worker (KT-328). In ONE SQLite transaction — so the
/// dispatcher and `wait_for_peer` see nothing until it commits (DoD-7) — this:
///  1. opens a durable control offer keyed to `(execution, attempt)` and the exact
///     target session (idempotent reattach on a resume, session guard on a clash);
///  2. on a fresh open, posts the opaque control message in the ORIGIN room
///     targeted to that session (NO native dispatch → no spawn, wakes only the
///     joined session via KT-330) and records it as the offer's provenance;
///  3. parks the execution `Provisioning → Blocked(awaiting_worker_acceptance)`.
///
/// The task stays `Todo`; the work brief is NOT posted here (it lands in the child
/// only at acceptance). A `SessionCommittedElsewhere` clash is a resumable
/// `Blocked` naming the holder — never a `Failed` on the raw UNIQUE index.
async fn open_cli_worker_control_offer(
    db: &Database,
    prepared: &Prepared,
    origin_discussion_id: &str,
    target: &MessageTarget,
    sub_disc_id: &str,
) -> Result<TaskExecution, ProvisionError> {
    let session_pk = target.cli_session_id.ok_or_else(|| {
        ProvisionError::Internal("cli worker identity has no cli_session_id".into())
    })?;
    let exec_id = prepared.execution.id.clone();
    let attempt = prepared.execution.attempt_no;
    let origin = origin_discussion_id.to_string();
    let child = sub_disc_id.to_string();
    let task_ref = prepared.task_reference.clone();
    let task_title = prepared.task_title.clone();
    let agent_type = target.agent_type.clone();

    db.with_conn(move |conn| {
        let tx = conn.unchecked_transaction()?;
        // 1. Open the offer INSIDE the tx (idempotent reattach / session guard). A
        //    rollback of this tx drops the offer with everything else.
        let new = crate::db::worker_offers::NewWorkerOffer {
            id: None,
            task_execution_id: &exec_id,
            attempt_no: attempt,
            target_cli_session_id: session_pk,
            origin_discussion_id: &origin,
            child_discussion_id: &child,
            // No deadline in V1: the offer waits until accepted/declined/re-offered
            // (a deadline is a KT-321 policy knob). Lazy expiry is already wired for
            // when one is set.
            expires_at: None,
            offer_message_id: None,
            reason: None,
        };
        let (reason, code) = match crate::db::worker_offers::open_worker_offer(&tx, &new)? {
            crate::db::worker_offers::OpenOutcome::Opened(offer) => {
                // Post the control message ONCE — a reattached (resumed) offer already
                // carries its provenance id, so a resume never double-posts.
                if offer.offer_message_id.is_none() {
                    let msg = build_control_offer_message(
                        &exec_id,
                        attempt,
                        &offer.id,
                        &task_ref,
                        &task_title,
                        &child,
                    );
                    let cli_target = MessageTarget::cli(agent_type.clone(), session_pk);
                    // Targeted to the exact session with NO dispatch spec: wakes the
                    // joined session's `wait_for_peer` (KT-330) without enqueuing a
                    // native agent or flipping `awaiting_agent` in the origin room.
                    crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
                        &tx,
                        &origin,
                        &msg,
                        &[cli_target],
                        &[],
                        None,
                    )?;
                    // Provenance now that the message exists (the
                    // `offer_message_id → messages(id)` FK holds).
                    crate::db::worker_offers::set_offer_message(&tx, &offer.id, &msg.id)?;
                }
                // Normal park — the exact session is expected to accept.
                (
                    "awaiting_worker_acceptance".to_string(),
                    BlockedReasonCode::AwaitingWorkerAcceptance,
                )
            }
            crate::db::worker_offers::OpenOutcome::SessionCommittedElsewhere { blocking } => {
                // The target session already holds a live offer for another execution.
                // Park with a structured reason + code naming the holder so the human
                // can re-offer to another session or pick a native worker.
                (
                    format!(
                        "worker session already committed to execution {} (attempt {}) — \
                         re-offer or choose a native worker",
                        blocking.task_execution_id, blocking.attempt_no
                    ),
                    BlockedReasonCode::WorkerSessionCommittedElsewhere,
                )
            }
        };
        // Provisioning → Blocked(reason, code). `transition_execution` records
        // `blocked_from_status = Provisioning` (KT-317) so acceptance resumes to the
        // exact origin state. Nothing above is visible until this commits.
        crate::db::orchestration::block_execution(
            &tx,
            &exec_id,
            &backend_actor(),
            &reason,
            Some(code),
        )?;
        tx.commit()?;
        crate::db::orchestration::get_task_execution(conn, &exec_id)?
            .context("execution vanished right after the CLI control-offer checkpoint")
    })
    .await
    .map_err(|e| ProvisionError::Internal(e.to_string()))
}

/// The verdict of a CLI worker accepting its control offer and attaching to its
/// sub-discussion (KT-328 tranche 2, commit 2).
#[derive(Debug)]
// Outcome enum: the success payload travels inline rather than boxed, so the
// nominal path pays no allocation.
#[allow(clippy::large_enum_variant)]
pub enum AcceptAttachOutcome {
    /// The exact target session accepted: its durable binding + membership moved to the
    /// child, the work brief is posted there (Cli-targeted, no dispatch), the execution
    /// is `Working`, the task `InProgress`, the offer `accepted`, and the origin room
    /// carries the durable attach notice. Carries the child disc id + refreshed execution.
    Attached {
        child_discussion_id: String,
        execution: TaskExecution,
    },
    /// No offer with that opaque id.
    NotFound,
    /// The caller is not the session this offer targets (same provider included) or its
    /// identity is unresolvable — no mutation.
    WrongAcceptor,
    /// The exact live target has no reload-stable binding in the offer origin/child.
    /// This is actionable only after exact-target authorization and mutates nothing.
    BindingMismatch,
    /// The offer is no longer acceptable; `status` names the real state.
    NotAcceptable {
        status: crate::models::WorkerOfferStatus,
    },
    /// The offer expired at read (deadline passed or target session left the room).
    Expired,
    /// The final checkpoint refused (task raced out of Todo, or the execution moved);
    /// the checkpoint rolled back — execution resumable, task untouched.
    CheckpointRefused(String),
}

/// Accept a CLI worker control offer and attach the worker to its sub-discussion
/// (KT-328 tranche 2, commit 2). Both identities are derived by the trusted bridge:
/// the live `(source_agent, source_session_id)` resolves the exact target session,
/// while `source_binding_session_id` names its separate reload-stable room binding.
/// The model supplies neither. Three durable steps, each idempotent so a crash between
/// any two resumes cleanly:
///   1. stage the accept (CAS `pending → accepting`, commit 1) — reversible, no external
///      effect yet;
///   2. move the worker session origin → child — durable source binding (idempotent,
///      fail-closed on an ownership race) AND `discussion_sessions` membership, so the
///      brief targeted at it in the child routes/wakes correctly and the invariant "one
///      active session = one discussion" holds literally;
///   3. the final atomic checkpoint (brief in child, `Working`, task `InProgress`, offer
///      `accepted`, durable attach notice in origin).
///
/// The two-phase move (step 2) and checkpoint (step 3) are deliberately separate durable
/// txns — the server binding and the bridge's local binding/cursor cannot share one — so
/// a crash after the transfer, before the checkpoint, resumes: the transfer replays as a
/// no-op and the checkpoint runs from the still-`accepting` offer.
pub async fn accept_worker_offer_and_attach(
    db: &Database,
    offer_id: &str,
    source_agent: &str,
    source_session_id: &str,
    source_binding_session_id: &str,
) -> Result<AcceptAttachOutcome, ProvisionError> {
    use crate::db::worker_offers::AcceptOutcome;
    use crate::models::WorkerOfferStatus;

    // ── 1. Stage the accept (CAS pending → accepting). ──
    let offer = {
        let (oid, agent, sess, binding_session) = (
            offer_id.to_string(),
            source_agent.to_string(),
            source_session_id.to_string(),
            source_binding_session_id.to_string(),
        );
        let staged = db
            .with_conn(move |conn| {
                crate::db::worker_offers::accept_worker_offer(
                    conn,
                    &oid,
                    &agent,
                    &sess,
                    &binding_session,
                )
            })
            .await
            .map_err(|e| ProvisionError::Internal(e.to_string()))?;
        match staged {
            AcceptOutcome::Accepting(o) => o,
            // Idempotent resume: `accept_worker_offer` returns NotAcceptable{accepted}
            // only AFTER verifying caller == target, so re-read and finalize — every step
            // below is idempotent, so a crash after a prior accept still converges.
            AcceptOutcome::NotAcceptable {
                status: WorkerOfferStatus::Accepted,
            } => {
                let oid = offer_id.to_string();
                match db
                    .with_conn(move |conn| crate::db::worker_offers::get_worker_offer(conn, &oid))
                    .await
                    .map_err(|e| ProvisionError::Internal(e.to_string()))?
                {
                    Some(o) => o,
                    None => return Ok(AcceptAttachOutcome::NotFound),
                }
            }
            AcceptOutcome::NotFound => return Ok(AcceptAttachOutcome::NotFound),
            AcceptOutcome::WrongAcceptor => return Ok(AcceptAttachOutcome::WrongAcceptor),
            AcceptOutcome::BindingMismatch => return Ok(AcceptAttachOutcome::BindingMismatch),
            AcceptOutcome::Expired => return Ok(AcceptAttachOutcome::Expired),
            AcceptOutcome::NotAcceptable { status } => {
                return Ok(AcceptAttachOutcome::NotAcceptable { status })
            }
        }
    };

    let origin = offer.origin_discussion_id.clone();
    let child = offer.child_discussion_id.clone();
    let exec_id = offer.task_execution_id.clone();
    let session_pk = offer.target_cli_session_id;

    // ── Rework re-accept fast path (KT-319 tranche 3b, DoD-9). A re-offer sets origin == child
    // (the worker never left its sub-discussion during the review), so there is NO session to
    // move and the task is already `InProgress`. Route straight to the rework checkpoint
    // (`Blocked → Provisioning → Working` + settle the offer), skipping the provisioning
    // session-move and task-CAS. Idempotent: a resumed already-`accepted` offer converges here
    // to Attached. ──
    if origin == child {
        use crate::db::orchestration::CliReworkOutcome;
        let (eid, oid) = (exec_id.clone(), offer.id.clone());
        let outcome = db
            .with_conn(move |conn| {
                crate::db::orchestration::commit_cli_rework_checkpoint(
                    conn,
                    &eid,
                    &oid,
                    &backend_actor(),
                )
            })
            .await
            .map_err(|e| ProvisionError::Internal(e.to_string()))?;
        return match outcome {
            CliReworkOutcome::Resumed | CliReworkOutcome::AlreadyResumed => {
                let eid = exec_id.clone();
                let execution = db
                    .with_conn(move |conn| {
                        crate::db::orchestration::get_task_execution(conn, &eid)?
                            .context("execution vanished right after the rework checkpoint")
                    })
                    .await
                    .map_err(|e| ProvisionError::Internal(e.to_string()))?;
                Ok(AcceptAttachOutcome::Attached {
                    child_discussion_id: child,
                    execution,
                })
            }
            CliReworkOutcome::OfferNotAccepting { status } => {
                Ok(AcceptAttachOutcome::NotAcceptable { status })
            }
            CliReworkOutcome::ExecutionRaced => Ok(AcceptAttachOutcome::CheckpointRefused(
                "execution raced out of the rework checkpoint".into(),
            )),
        };
    }

    // ── 2. Move the worker session origin → child (binding + membership). The accepting
    // caller's LIVE identity IS the target session's (accept verified the pk match).
    // Transfer its independently-derived DURABLE room binding. Keeping these values
    // separate is essential after an MCP reload: the active row rotates to `adhoc-*`,
    // while the source-history owner remains `cli-*`. Idempotent + fail-closed. ──
    {
        let (o, c) = (origin.clone(), child.clone());
        let (agent, binding_session) = (
            source_agent.to_string(),
            source_binding_session_id.to_string(),
        );
        db.with_conn(move |conn| {
            crate::db::disc_source::transfer_source_binding(
                conn,
                &o,
                &c,
                &agent,
                &binding_session,
            )?;
            crate::db::discussion_sessions::move_session_to_discussion(conn, session_pk, &c)?;
            Ok(())
        })
        .await
        .map_err(|e| ProvisionError::Internal(format!("session transfer failed: {e}")))?;
    }

    // CLI reassignment uses the same durable offer + exact-session transfer as
    // initial provisioning, but the plan task is already InProgress. Settle a
    // bounded handoff without replaying the task CAS or reposting the original
    // brief.
    let reassignment = {
        let eid = exec_id.clone();
        db.with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &eid)?
                .context("execution vanished after reassignment session transfer")?;
            let recovery = crate::db::orchestration::get_execution_recovery(conn, &eid)?;
            Ok((execution, recovery))
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?
    };
    if reassignment.0.status == TaskExecutionStatus::Interrupted
        && reassignment
            .1
            .as_ref()
            .is_some_and(|recovery| recovery.pending && recovery.assignment_generation > 0)
    {
        let recovery = reassignment.1.expect("checked above");
        let target = worker_target_from_execution(&reassignment.0)
            .map_err(|e| ProvisionError::Internal(e.to_string()))?;
        let handoff = orchestrator_message(
            format!(
                "orch-reassign-handoff:{}:{}",
                exec_id, recovery.assignment_generation
            ),
            format!(
                "**Réassignation acceptée — génération {}**\n\n\
                 Reprends exactement l'étape inachevée. Cette sous-discussion, son worktree, \
                 les manifests, constats, revues et SHA existants restent la source de vérité. \
                 Motif : {}",
                recovery.assignment_generation, recovery.recovery_reason
            ),
        );
        let (eid, oid, child_id) = (exec_id.clone(), offer.id.clone(), child.clone());
        let outcome = db
            .with_conn(move |conn| {
                crate::db::orchestration::commit_cli_reassignment_checkpoint(
                    conn,
                    &eid,
                    &oid,
                    &child_id,
                    &handoff,
                    &target,
                    &backend_actor(),
                )
            })
            .await
            .map_err(|e| ProvisionError::Internal(e.to_string()))?;
        use crate::db::orchestration::CliReassignmentOutcome;
        return match outcome {
            CliReassignmentOutcome::Resumed | CliReassignmentOutcome::AlreadyResumed => {
                let eid = exec_id.clone();
                let execution = db
                    .with_conn(move |conn| {
                        crate::db::orchestration::get_task_execution(conn, &eid)?
                            .context("execution vanished after CLI reassignment")
                    })
                    .await
                    .map_err(|e| ProvisionError::Internal(e.to_string()))?;
                Ok(AcceptAttachOutcome::Attached {
                    child_discussion_id: child,
                    execution,
                })
            }
            CliReassignmentOutcome::OfferNotAccepting { status } => {
                Ok(AcceptAttachOutcome::NotAcceptable { status })
            }
            CliReassignmentOutcome::ExecutionRaced => Ok(AcceptAttachOutcome::CheckpointRefused(
                "execution raced out of the CLI reassignment checkpoint".into(),
            )),
        };
    }

    // ── 3. Rebuild the brief + attach notice from the execution + task + managed
    // workspace, then run the final atomic checkpoint. ──
    let (execution, task, workspace, alias) = {
        let eid = exec_id.clone();
        db.with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &eid)?
                .context("execution vanished before the acceptance checkpoint")?;
            let task = crate::db::planning::get_task(conn, &execution.task_id)?
                .context("task vanished before the acceptance checkpoint")?;
            let workspace =
                crate::db::discussion_workspaces::get_managed_for_execution(conn, &eid)?
                    .context("managed workspace vanished before the acceptance checkpoint")?;
            let (_ordinal, alias) =
                crate::db::discussion_sessions::cli_session_identity(conn, session_pk)?;
            Ok((execution, task, workspace, alias))
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?
    };

    let worktree_path = workspace.workspace_path.clone().unwrap_or_default();
    let branch = workspace.branch.clone();
    let base_sha = workspace.base_sha.clone().unwrap_or_default();
    let target = worker_target_from_execution(&execution)
        .map_err(|e| ProvisionError::Internal(e.to_string()))?;

    let brief = build_cli_worker_brief(
        &exec_id,
        execution.attempt_no,
        &task.summary.reference,
        &task.summary.title,
        &task.description,
        &task.definition_of_done,
        &worktree_path,
        &branch,
        &base_sha,
    );
    let notice = build_attach_notice(
        &exec_id,
        execution.attempt_no,
        &task.summary.reference,
        &task.summary.title,
        &child,
        alias.as_deref(),
    );

    let outcome = {
        let (eid, sub, orig, tref, oid) = (
            exec_id.clone(),
            child.clone(),
            origin.clone(),
            task.summary.reference.clone(),
            offer.id.clone(),
        );
        db.with_conn(move |conn| {
            crate::db::orchestration::commit_cli_provisioning_checkpoint(
                conn,
                &crate::db::orchestration::CliProvisioningCheckpoint {
                    exec_id: &eid,
                    sub_discussion_id: &sub,
                    origin_discussion_id: &orig,
                    task_reference: &tref,
                    offer_id: &oid,
                    brief: &brief,
                    target: &target,
                    attach_notice: &notice,
                    actor: &backend_actor(),
                },
            )
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?
    };

    use crate::db::orchestration::CliCheckpointOutcome;
    match outcome {
        CliCheckpointOutcome::Committed | CliCheckpointOutcome::AlreadyCommitted => {
            let eid = exec_id.clone();
            let execution = db
                .with_conn(move |conn| {
                    crate::db::orchestration::get_task_execution(conn, &eid)?
                        .context("execution vanished right after the acceptance checkpoint")
                })
                .await
                .map_err(|e| ProvisionError::Internal(e.to_string()))?;
            Ok(AcceptAttachOutcome::Attached {
                child_discussion_id: child,
                execution,
            })
        }
        CliCheckpointOutcome::TaskNotStarted(reason) => Ok(AcceptAttachOutcome::CheckpointRefused(
            checkpoint_refusal_reason(&reason),
        )),
        CliCheckpointOutcome::ExecutionRaced => Ok(AcceptAttachOutcome::CheckpointRefused(
            "execution raced out of the acceptance checkpoint".into(),
        )),
        CliCheckpointOutcome::OfferNotAccepting { status } => {
            Ok(AcceptAttachOutcome::NotAcceptable { status })
        }
    }
}

/// The verdict of a worker delivery (KT-319 tranche 2). Refusals are typed — never an
/// `Err` masquerading as a business outcome (DB errors stay in the outer `Err`).
#[derive(Debug)]
// Outcome enum: the success payload travels inline rather than boxed, so the
// nominal path pays no allocation.
#[allow(clippy::large_enum_variant)]
pub enum DeliverOutcome {
    /// Manifest persisted, execution `AwaitingReview`, the principal review obligation
    /// recorded + the review request posted in the parent room.
    Delivered {
        /// The parent (principal) room the review request landed in.
        review_discussion_id: String,
        execution: TaskExecution,
    },
    /// The execution is unknown OR the caller is not its exact worker — FUSED into ONE
    /// opaque refusal (anti-oracle): a stranger cannot probe which executions exist or who
    /// owns them.
    NotAddressed,
    /// The caller IS the worker, but the execution is not deliverable (not `Working`);
    /// `status` names the real state — reachable only after authz, so not an oracle.
    NotDeliverable { status: TaskExecutionStatus },
    /// The manifest failed the v1 contract; `detail` explains (never a silent accept).
    InvalidManifest(String),
}

/// A worker submits its DeliveryManifest for review (KT-319 tranche 2, DoD-1/2/3). The
/// caller's identity is DERIVED SERVER-SIDE from the bridge-supplied durable pair (the
/// model never supplies a session pk); the caller must be THIS execution's exact CLI
/// worker. On success the manifest is persisted, the execution flips `Working →
/// AwaitingReview` through the guarded CAS, and a principal-targeted review request is
/// posted in the parent room — the durable, queryable review obligation.
pub async fn deliver_worker_manifest(
    db: &Database,
    task_execution_id: &str,
    source_agent: &str,
    source_session_id: &str,
    manifest_json: &str,
) -> Result<DeliverOutcome, ProvisionError> {
    // ── 1. Resolve the execution + the caller's session server-side. ──
    let (exec, caller_session_id) = {
        let (eid, agent, sess) = (
            task_execution_id.to_string(),
            source_agent.to_string(),
            source_session_id.to_string(),
        );
        db.with_conn(move |conn| {
            let exec = crate::db::orchestration::get_task_execution(conn, &eid)?;
            let session = crate::db::discussion_sessions::find_active_session(conn, &agent, &sess)?;
            Ok((exec, session.map(|s| s.id)))
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?
    };

    // ── 2. Authorize: caller must be this execution's exact CLI worker. An unknown
    // execution and a wrong caller FUSE into NotAddressed (anti-oracle). ──
    let Some(exec) = exec else {
        return Ok(DeliverOutcome::NotAddressed);
    };
    let authorized = matches!(
        (exec.worker_cli_session_id, caller_session_id),
        (Some(worker), Some(caller)) if worker == caller
    );
    if !authorized {
        return Ok(DeliverOutcome::NotAddressed);
    }
    let caller_session_id = caller_session_id.expect("authorized implies a resolved session");

    let alias = {
        let fallback = source_agent.to_string();
        db.with_conn(move |conn| {
            Ok(
                crate::db::discussion_sessions::cli_session_identity(conn, caller_session_id)?
                    .1
                    .unwrap_or(fallback),
            )
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?
    };
    deliver_authorized_worker_manifest(
        db,
        exec,
        &alias,
        Some(source_session_id.to_string()),
        manifest_json,
        DeliveryManifestAuthorship::Full,
    )
    .await
}

#[derive(Clone, Copy)]
enum DeliveryManifestAuthorship {
    Full,
    NativeProjection,
}

/// Trusted native identity assembled by Kronn's executor, never from model tool
/// arguments. The dispatch trigger pins a particular run even when two agents
/// of the same provider coexist in one room.
#[derive(Clone, Copy)]
pub(crate) struct NativeExecutionCaller<'a> {
    pub discussion_id: &'a str,
    pub agent_type: &'a AgentType,
    pub source_message_id: Option<&'a str>,
    pub alias: &'a str,
    pub actor_session_id: Option<&'a str>,
}

/// Authenticate the exact native/spawned worker selected for one execution.
///
/// The caller tuple is assembled from trusted executor state (native HTTP) or
/// from the runner-owned bridge environment (spawned host CLI), never from the
/// model's tool arguments. Keeping this check shared prevents commit and
/// delivery from drifting into subtly different authority rules.
async fn native_worker_execution_for_caller(
    db: &Database,
    task_execution_id: &str,
    caller: NativeExecutionCaller<'_>,
) -> Result<Option<TaskExecution>, ProvisionError> {
    let (execution, dispatch_matches) = {
        let execution_id = task_execution_id.to_string();
        let source_message_id = caller.source_message_id.map(str::to_string);
        db.with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?;
            let dispatch_matches = execution
                .as_ref()
                .map(|execution| {
                    native_worker_dispatch_matches(conn, execution, source_message_id.as_deref())
                })
                .transpose()?
                .unwrap_or(false);
            Ok((execution, dispatch_matches))
        })
        .await
        .map_err(|error| ProvisionError::Internal(error.to_string()))?
    };
    let Some(execution) = execution else {
        return Ok(None);
    };
    let provider_matches = execution
        .worker_agent_type
        .as_deref()
        .map(crate::db::orchestration::agent_type_from_db)
        .transpose()
        .map_err(|error| ProvisionError::Internal(error.to_string()))?
        .as_ref()
        == Some(caller.agent_type);
    let authorized = execution.worker_cli_session_id.is_none()
        && execution.sub_discussion_id.as_deref() == Some(caller.discussion_id)
        && provider_matches
        && dispatch_matches;
    Ok(authorized.then_some(execution))
}

/// Native HTTP agents do not own a CLI session. Their trusted executor supplies the
/// current discussion, typed provider and dispatch trigger, so the backend can
/// authenticate the worker without accepting identity from model arguments.
pub(crate) async fn deliver_native_worker_manifest(
    db: &Database,
    task_execution_id: &str,
    caller: NativeExecutionCaller<'_>,
    manifest_json: &str,
) -> Result<DeliverOutcome, ProvisionError> {
    let Some(execution) = native_worker_execution_for_caller(db, task_execution_id, caller).await?
    else {
        return Ok(DeliverOutcome::NotAddressed);
    };
    deliver_authorized_worker_manifest(
        db,
        execution,
        caller.alias,
        caller.actor_session_id.map(str::to_string),
        manifest_json,
        DeliveryManifestAuthorship::NativeProjection,
    )
    .await
}

async fn deliver_authorized_worker_manifest(
    db: &Database,
    exec: TaskExecution,
    alias: &str,
    actor_session_id: Option<String>,
    manifest_json: &str,
    authorship: DeliveryManifestAuthorship,
) -> Result<DeliverOutcome, ProvisionError> {
    if !matches!(
        exec.status,
        TaskExecutionStatus::Working | TaskExecutionStatus::AwaitingReview
    ) {
        return Ok(DeliverOutcome::NotDeliverable {
            status: exec.status,
        });
    }
    // ── 3. Resolve the task + the parent's principal AFTER authz. A stranger
    // still gets no task, manifest or worktree validation oracle. ──
    let (task, principal_agent) = {
        let (tid, parent) = (exec.task_id.clone(), exec.parent_discussion_id.clone());
        db.with_conn(move |conn| {
            let task = crate::db::planning::get_task(conn, &tid)?
                .context("task vanished before delivery")?;
            let parent = crate::db::discussions::get_discussion(conn, &parent)?
                .context("parent discussion vanished before delivery")?;
            Ok((task, parent.agent))
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?
    };

    // ── 4. Validate and normalize the manifest. CLI workers submit the public
    // DeliveryManifest v1 unchanged. Native HTTP workers submit only semantic
    // assertions: Kronn injects opaque/mechanical facts from trusted state. ──
    let (manifest, normalized_manifest_json, git_facts) = match authorship {
        DeliveryManifestAuthorship::Full => {
            let manifest = match parse_delivery_manifest(manifest_json) {
                Ok(manifest) => manifest,
                Err(error) => return Ok(DeliverOutcome::InvalidManifest(error.to_string())),
            };
            (manifest, manifest_json.to_string(), None)
        }
        DeliveryManifestAuthorship::NativeProjection => {
            let projected =
                match prepare_native_worker_manifest(manifest_json, &task.definition_of_done) {
                    Ok(projected) => projected,
                    Err(detail) => {
                        return Ok(DeliverOutcome::InvalidManifest(format!(
                            "DeliveryManifest v1 invalide : {detail}"
                        )))
                    }
                };
            let current_dod_ids = task
                .definition_of_done
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>();
            match exec.worker_dod_ids.as_deref() {
                Some(snapshot)
                    if snapshot
                        .iter()
                        .map(String::as_str)
                        .eq(current_dod_ids.iter().copied()) => {}
                Some(_) => {
                    return Ok(DeliverOutcome::InvalidManifest(
                        "task Definition of Done changed since this execution was launched; cancel it and launch a fresh execution before delivering ordered native assertions"
                            .into(),
                    ))
                }
                None => {
                    return Ok(DeliverOutcome::InvalidManifest(
                        "execution has no launch-time Definition of Done snapshot; cancel it and launch a fresh execution before native projected delivery"
                            .into(),
                    ))
                }
            }
            let facts = match delivery_git_facts(db, &exec).await {
                Ok(facts) => facts,
                Err(detail) => return Ok(DeliverOutcome::InvalidManifest(detail)),
            };
            let (manifest, normalized) = match normalize_native_worker_manifest(
                projected,
                &task.summary.reference,
                &facts,
            ) {
                Ok(normalized) => normalized,
                Err(detail) => return Ok(DeliverOutcome::InvalidManifest(detail)),
            };
            (manifest, normalized, Some(facts))
        }
    };
    if manifest.task_ref != task.summary.reference {
        return Ok(DeliverOutcome::InvalidManifest(format!(
            "task_ref `{}` does not match execution task `{}`",
            manifest.task_ref, task.summary.reference
        )));
    }
    if let Err(detail) = validate_manifest_claims(&task.definition_of_done, &manifest) {
        return Ok(DeliverOutcome::InvalidManifest(detail));
    }
    let git_validation = match git_facts.as_ref() {
        Some(facts) => validate_delivery_git_facts(facts, &manifest),
        None => validate_delivery_git_state(db, &exec, &manifest).await,
    };
    if let Err(detail) = git_validation {
        return Ok(DeliverOutcome::InvalidManifest(detail));
    }
    let principal_target = MessageTarget::discussion_agent(principal_agent);
    let child = exec.sub_discussion_id.clone().unwrap_or_default();
    let review_request = build_review_request_message(
        &exec.id,
        exec.attempt_no,
        &task.summary.reference,
        &task.summary.title,
        &child,
        &manifest.head_sha,
        &manifest.summary,
    );
    // The queryable obligation payload carries the exact TARGETED principal identity (DoD-3).
    let review_requested_changes = serde_json::json!({
        "principal_discussion_id": exec.parent_discussion_id,
        "principal_target": principal_target.clone(),
        "delivery_attempt": exec.attempt_no,
        "head_sha": manifest.head_sha,
        "child_discussion_id": child,
    });

    // ── 5. Atomic delivery checkpoint. ──
    let outcome = {
        let (eid, attempt) = (exec.id.clone(), exec.attempt_no);
        let alias = alias.to_string();
        let actor_session_id = actor_session_id.clone();
        let (parent, head, mj) = (
            exec.parent_discussion_id.clone(),
            manifest.head_sha.clone(),
            normalized_manifest_json,
        );
        db.with_conn(move |conn| {
            crate::db::orchestration::commit_delivery_checkpoint(
                conn,
                &crate::db::orchestration::DeliveryCheckpoint {
                    exec_id: &eid,
                    attempt_no: attempt,
                    head_sha: &head,
                    manifest_json: &mj,
                    parent_discussion_id: &parent,
                    review_request: &review_request,
                    principal_target: &principal_target,
                    review_requested_changes,
                    actor: &agent_actor(&alias, actor_session_id.as_deref()),
                },
            )
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?
    };

    use crate::db::orchestration::DeliveryCheckpointOutcome;
    match outcome {
        DeliveryCheckpointOutcome::Delivered | DeliveryCheckpointOutcome::AlreadyDelivered => {
            let eid = exec.id.clone();
            let execution = db
                .with_conn(move |conn| {
                    crate::db::orchestration::get_task_execution(conn, &eid)?
                        .context("execution vanished right after the delivery checkpoint")
                })
                .await
                .map_err(|e| ProvisionError::Internal(e.to_string()))?;
            Ok(DeliverOutcome::Delivered {
                review_discussion_id: exec.parent_discussion_id.clone(),
                execution,
            })
        }
        DeliveryCheckpointOutcome::NotDeliverable { status } => {
            Ok(DeliverOutcome::NotDeliverable { status })
        }
        DeliveryCheckpointOutcome::ExecutionRaced => Ok(DeliverOutcome::NotDeliverable {
            status: exec.status,
        }),
    }
}

#[derive(Debug)]
struct DeliveryGitFacts {
    repo: std::path::PathBuf,
    base_sha: String,
    head_sha: String,
    files_touched: Vec<ManifestFile>,
}

/// Expand a native HTTP worker's semantic assertions into the same durable
/// DeliveryManifest v1 used by CLI workers. The model never copies opaque ids
/// or Git facts: their sole authority is the authorized task execution.
fn prepare_native_worker_manifest(
    manifest_json: &str,
    task_dod: &[PlanningDodItem],
) -> Result<serde_json::Value, String> {
    let mut value: serde_json::Value = serde_json::from_str(manifest_json)
        .map_err(|error| format!("native delivery assertions are not valid JSON: {error}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "native delivery assertions must be a JSON object".to_string())?;
    const ROOT_FIELDS: &[&str] = &[
        "tests",
        "dod_status",
        "docs",
        "migrations",
        "risks",
        "limitations",
        "summary",
    ];
    const MECHANICAL_FIELDS: &[&str] = &["version", "task_ref", "head_sha", "files_touched"];
    let mut mechanical = object
        .keys()
        .filter(|key| MECHANICAL_FIELDS.contains(&key.as_str()))
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>();
    let mut unsupported = object
        .keys()
        .filter(|key| {
            !ROOT_FIELDS.contains(&key.as_str()) && !MECHANICAL_FIELDS.contains(&key.as_str())
        })
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>();
    if let Some(statuses) = object
        .get("dod_status")
        .and_then(serde_json::Value::as_array)
    {
        for (position, status) in statuses.iter().enumerate() {
            if let Some(status) = status.as_object() {
                for key in status.keys() {
                    if key == "dod_id" {
                        mechanical.push(format!("`dod_status[{position}].dod_id`"));
                    } else if !matches!(key.as_str(), "met" | "evidence") {
                        unsupported.push(format!("`dod_status[{position}].{key}`"));
                    }
                }
            }
        }
    }
    if let Some(tests) = object.get("tests").and_then(serde_json::Value::as_array) {
        for (position, test) in tests.iter().enumerate() {
            if let Some(test) = test.as_object() {
                unsupported.extend(
                    test.keys()
                        .filter(|key| !matches!(key.as_str(), "name" | "status" | "evidence"))
                        .map(|key| format!("`tests[{position}].{key}`")),
                );
            }
        }
    }
    let mut refusals = Vec::new();
    if !mechanical.is_empty() {
        refusals.push(format!(
            "native delivery must not author {}; Kronn derives these mechanical fields from trusted state",
            mechanical.join(", ")
        ));
    }
    if !unsupported.is_empty() {
        refusals.push(format!(
            "native delivery contains unsupported fields: {}",
            unsupported.join(", ")
        ));
    }
    if !refusals.is_empty() {
        return Err(refusals.join("; "));
    }
    let dod_status = object
        .get_mut("dod_status")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| "native delivery `dod_status` must be an array".to_string())?;
    if dod_status.len() != task_dod.len() {
        return Err(format!(
            "native delivery `dod_status` must contain exactly {} item(s), in Definition of Done order; got {}",
            task_dod.len(),
            dod_status.len()
        ));
    }
    for (position, (status, dod)) in dod_status.iter_mut().zip(task_dod).enumerate() {
        let status = status
            .as_object_mut()
            .ok_or_else(|| format!("native delivery `dod_status[{position}]` must be an object"))?;
        status.insert("dod_id".into(), serde_json::json!(dod.id));
    }
    Ok(value)
}

fn normalize_native_worker_manifest(
    mut value: serde_json::Value,
    task_reference: &str,
    facts: &DeliveryGitFacts,
) -> Result<(DeliveryManifestV1, String), String> {
    let object = value.as_object_mut().ok_or_else(|| {
        "prepared native delivery ceased to be an object before normalization".to_string()
    })?;
    object.insert(
        "version".into(),
        serde_json::json!(DELIVERY_CONTRACT_VERSION),
    );
    object.insert("task_ref".into(), serde_json::json!(task_reference));
    object.insert("head_sha".into(), serde_json::json!(facts.head_sha));
    object.insert(
        "files_touched".into(),
        serde_json::to_value(&facts.files_touched)
            .map_err(|error| format!("cannot serialize committed file inventory: {error}"))?,
    );
    let manifest = parse_delivery_manifest(
        &serde_json::to_string(&value)
            .map_err(|error| format!("cannot serialize projected delivery manifest: {error}"))?,
    )
    .map_err(|error| error.to_string())?;
    // Persist only the typed roundtrip. Even if the generic JSON-subset
    // validator becomes permissive, model-authored unknown fields cannot leak
    // into the durable review payload.
    let normalized = serde_json::to_string(&manifest)
        .map_err(|error| format!("cannot serialize normalized delivery manifest: {error}"))?;
    Ok((manifest, normalized))
}

/// Validate that the manifest describes a durable Git state, rather than a
/// plausible story about edits that only exist in the worker process or its
/// dirty worktree. This runs after worker authorization, so its detailed
/// refusals do not become an execution/workspace oracle.
async fn validate_delivery_git_state(
    db: &Database,
    exec: &TaskExecution,
    manifest: &DeliveryManifestV1,
) -> Result<(), String> {
    let facts = delivery_git_facts(db, exec).await?;
    validate_delivery_git_facts(&facts, manifest)
}

async fn delivery_git_facts(
    db: &Database,
    exec: &TaskExecution,
) -> Result<DeliveryGitFacts, String> {
    let execution_id = exec.id.clone();
    let workspace = db
        .with_conn(move |conn| {
            crate::db::discussion_workspaces::get_managed_for_execution(conn, &execution_id)
        })
        .await
        .map_err(|error| format!("managed worktree lookup failed: {error}"))?
        .and_then(|workspace| workspace.canonical_path)
        .ok_or_else(|| "execution has no managed worktree".to_string())?;
    let repo = std::path::PathBuf::from(workspace);

    let dirty = worktree::worktree_dirty_files(&repo)
        .map_err(|error| format!("cannot verify worktree cleanliness: {error}"))?;
    if !dirty.is_empty() {
        let files = dirty
            .iter()
            .map(|file| format!("{} {}", file.status, file.path))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "worktree has uncommitted changes ({files}); commit them and deliver the new HEAD"
        ));
    }

    let base_rev = exec
        .base_sha
        .as_deref()
        .ok_or_else(|| "execution has no pinned base_sha".to_string())?;
    delivery_git_facts_from_repo(repo, base_rev)
}

fn delivery_git_facts_from_repo(
    repo: std::path::PathBuf,
    base_rev: &str,
) -> Result<DeliveryGitFacts, String> {
    let base = worktree::resolve_commit(&repo, base_rev)
        .map_err(|error| format!("cannot resolve execution base_sha: {error}"))?;
    let current = worktree::resolve_commit(&repo, "HEAD")
        .map_err(|error| format!("cannot resolve worktree HEAD: {error}"))?;
    let files_touched = worktree::committed_file_changes(&repo, &base, &current)
        .map_err(|error| format!("cannot inspect committed delivery diff: {error}"))?
        .into_iter()
        .map(|change| {
            let kind = match change.kind {
                'A' => FileChangeKind::Added,
                'M' => FileChangeKind::Modified,
                'D' => FileChangeKind::Deleted,
                other => return Err(format!("unsupported committed file status `{other}`")),
            };
            Ok(ManifestFile {
                path: change.path,
                kind,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(DeliveryGitFacts {
        repo,
        base_sha: base,
        head_sha: current,
        files_touched,
    })
}

fn validate_delivery_git_facts(
    facts: &DeliveryGitFacts,
    manifest: &DeliveryManifestV1,
) -> Result<(), String> {
    let delivered = worktree::resolve_commit(&facts.repo, &manifest.head_sha)
        .map_err(|error| format!("manifest head_sha is not a commit in the worktree: {error}"))?;
    if delivered != facts.head_sha {
        return Err(format!(
            "manifest head_sha `{delivered}` is not the current worktree HEAD `{}`",
            facts.head_sha
        ));
    }
    validate_committed_file_inventory(facts, manifest)
}

fn validate_manifest_claims(
    task_dod: &[PlanningDodItem],
    manifest: &DeliveryManifestV1,
) -> Result<(), String> {
    let expected = task_dod
        .iter()
        .map(|item| item.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut seen = std::collections::HashSet::new();
    for status in &manifest.dod_status {
        if !seen.insert(status.dod_id.as_str()) {
            return Err(format!(
                "dod_status contains duplicate DoD id `{}`",
                status.dod_id
            ));
        }
        if !expected.contains(status.dod_id.as_str()) {
            return Err(format!(
                "dod_status references unknown DoD id `{}`",
                status.dod_id
            ));
        }
        if status
            .evidence
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            return Err(format!("DoD `{}` has no non-empty evidence", status.dod_id));
        }
    }
    let missing = task_dod
        .iter()
        .filter(|item| !seen.contains(item.id.as_str()))
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "dod_status must cover every task DoD exactly once; missing: {}",
            missing.join(", ")
        ));
    }
    if let Some(test) = manifest.tests.iter().find(|test| {
        test.evidence
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
    }) {
        return Err(format!(
            "test `{}` has status {:?} without non-empty evidence",
            test.name, test.status
        ));
    }
    Ok(())
}

fn validate_committed_file_inventory(
    facts: &DeliveryGitFacts,
    manifest: &DeliveryManifestV1,
) -> Result<(), String> {
    let actual = facts
        .files_touched
        .iter()
        .map(|file| {
            let kind = match file.kind {
                FileChangeKind::Added => 'A',
                FileChangeKind::Modified => 'M',
                FileChangeKind::Deleted => 'D',
            };
            (file.path.clone(), kind)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut claimed = std::collections::BTreeMap::new();
    for file in &manifest.files_touched {
        let kind = match file.kind {
            crate::models::FileChangeKind::Added => 'A',
            crate::models::FileChangeKind::Modified => 'M',
            crate::models::FileChangeKind::Deleted => 'D',
        };
        if claimed.insert(file.path.clone(), kind).is_some() {
            return Err(format!(
                "files_touched contains duplicate path `{}`",
                file.path
            ));
        }
    }
    if claimed != actual {
        return Err(format!(
            "files_touched does not match the committed diff from `{}` to `{}`: claimed {claimed:?}, actual {actual:?}",
            facts.base_sha, facts.head_sha
        ));
    }
    Ok(())
}

/// A structured reason an approve is refused by the DoD-5 guard. Every variant is reachable
/// ONLY after the service authorizes the caller as a party to the execution, so naming the
/// precise cause to the (authorized) principal is not an oracle — it is the actionable
/// feedback the principal needs to decide what to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApproveBlockReason {
    /// No DeliveryManifest is persisted for the current attempt — there is nothing to approve.
    NoManifest,
    /// A legacy or corrupt manifest does not cover the task's current DoD
    /// exactly once, or claims a green status without evidence.
    ManifestClaimsInvalid(String),
    /// The approval did not name the delivered commit the reviewer inspected.
    ReviewedHeadMismatch { reviewed: String, delivered: String },
    /// Review-owned DoD evidence is malformed, foreign to the task, or was
    /// supplied by a worker rather than the principal.
    ReviewEvidenceInvalid(String),
    /// The worktree HEAD moved since the delivery: the reviewed state no longer matches what
    /// would be integrated. Both shas are the full, `resolve_commit`-normalized form, so a
    /// short delivered sha vs. the long worktree HEAD never triggers a spurious refusal.
    HeadDrifted { delivered: String, current: String },
    /// The manifest self-reports one or more DoD items NOT met — the mandatory condition
    /// (DoD-5) is unsatisfied. `unmet` names the offending `dod_id`s.
    DodNotMet { unmet: Vec<String> },
    /// The managed worktree could not be resolved/read, so non-drift cannot be confirmed;
    /// approving blind would risk integrating a drifted state.
    WorktreeUnavailable(String),
    /// The worker modified/staged/untracked files after delivery (or delivered
    /// them without committing). The reviewed commit is not the whole state.
    WorktreeDirty { files: Vec<String> },
    /// The persisted manifest's file inventory does not describe the committed
    /// base..HEAD diff. This also protects rows accepted by an older backend.
    ManifestDiffMismatch(String),
}

/// The verdict of a principal review decision (KT-319 tranche 3a). Refusals are typed — never
/// an `Err` masquerading as a business outcome (DB errors stay in the outer `Err`).
#[derive(Debug)]
pub enum ReviewOutcome {
    /// The decision was applied: approve → `Approved`, request_changes → `ChangesRequested`
    /// (round bumped, findings delivered to the worker in the child).
    Reviewed {
        verdict: ReviewVerdict,
        execution: TaskExecution,
    },
    /// The execution is unknown OR the caller is neither its worker nor a principal (member)
    /// of its parent room — FUSED into ONE opaque refusal (anti-oracle): a stranger cannot
    /// probe which executions exist, who their worker is, or which room is their principal.
    NotAddressed,
    /// The caller IS the execution's own worker and the run does not allow self-review
    /// (DoD-7). Reachable only after establishing the caller is the worker, so it is not an
    /// oracle — the worker already knows it is the worker.
    SelfReviewForbidden,
    /// The caller is a party, but the execution is not `AwaitingReview`; `status` names the
    /// real state (reachable only after authz — not an oracle).
    NotReviewable { status: TaskExecutionStatus },
    /// approve was refused by the DoD-5 guard; `reason` is the precise, actionable cause.
    ApproveBlocked { reason: ApproveBlockReason },
    /// request_changes exhausted the review budget (`review_rounds` reached `max_review_rounds`):
    /// the execution is `Escalated` and the principal has been solicited in the parent room
    /// (DoD-6). No re-offer — a human decides. Carries the refreshed execution.
    Escalated { execution: TaskExecution },
    /// The ReviewDecision failed the v1 contract; `detail` explains (validated AFTER authz, so
    /// a stranger gets no validation oracle).
    InvalidDecision(String),
}

/// The principal decides a delivered attempt (KT-319 tranche 3a, DoD-2/4/5/7/8). The caller's
/// identity is DERIVED SERVER-SIDE from the bridge-supplied durable pair (the model never
/// supplies a session pk); the caller must be a PARTY to the execution — its exact CLI worker,
/// or a member (principal) of its parent room. On approve the DoD-5 guard runs (manifest
/// present, HEAD not drifted, DoD met); on request_changes the round is bumped and structured
/// findings are handed to the worker in the child. The state flip goes through the guarded,
/// journaled CAS — never a bare UPDATE.
pub async fn decide_review(
    db: &Database,
    task_execution_id: &str,
    decision_json: &str,
    source_agent: &str,
    source_session_id: &str,
) -> Result<ReviewOutcome, ProvisionError> {
    // ── 1. Resolve the execution, the caller's session and the run, server-side. ──
    let (exec, caller, run) = {
        let (eid, agent, sess) = (
            task_execution_id.to_string(),
            source_agent.to_string(),
            source_session_id.to_string(),
        );
        db.with_conn(move |conn| {
            let exec = crate::db::orchestration::get_task_execution(conn, &eid)?;
            let caller = crate::db::discussion_sessions::find_active_session(conn, &agent, &sess)?;
            let run = match &exec {
                Some(e) => {
                    crate::db::orchestration::get_orchestration_run(conn, &e.orchestration_run_id)?
                }
                None => None,
            };
            Ok((exec, caller, run))
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?
    };

    // ── 2. Authorize: the caller must be a PARTY to this execution — its exact CLI worker, or
    // a member (principal) of its parent room. An unknown execution and a total stranger FUSE
    // into NotAddressed (anti-oracle). ──
    let Some(exec) = exec else {
        return Ok(ReviewOutcome::NotAddressed);
    };
    let Some(caller) = caller else {
        return Ok(ReviewOutcome::NotAddressed);
    };
    let is_worker = matches!(exec.worker_cli_session_id, Some(w) if w == caller.id);
    let is_principal = caller.disc_id == exec.parent_discussion_id;
    if !is_worker && !is_principal {
        return Ok(ReviewOutcome::NotAddressed);
    }
    // DoD-7: the worker cannot decide its own review unless the run explicitly allows it.
    let allow_self_review = run.as_ref().map(|r| r.allow_self_review).unwrap_or(false);
    if is_worker && !allow_self_review {
        return Ok(ReviewOutcome::SelfReviewForbidden);
    }

    // ── 4. The deciding identity's alias attributes the review event (DoD-2 audit). ──
    let alias = {
        let cid = caller.id;
        let fallback = caller.agent_type.clone();
        db.with_conn(move |conn| crate::db::discussion_sessions::cli_session_identity(conn, cid))
            .await
            .map_err(|e| ProvisionError::Internal(e.to_string()))?
            .1
            .unwrap_or(fallback)
    };

    decide_authorized_review(
        db,
        exec,
        &alias,
        Some(source_session_id.to_string()),
        is_principal,
        decision_json,
    )
    .await
}

/// Review entry point for a native HTTP agent. The executor, not the model,
/// supplies the current room and typed provider. Parent-room callers are principals;
/// child-room callers must match the execution's native worker provider exactly.
pub(crate) async fn decide_native_review(
    db: &Database,
    task_execution_id: &str,
    decision_json: &str,
    caller: NativeExecutionCaller<'_>,
) -> Result<ReviewOutcome, ProvisionError> {
    let (execution, run, dispatch_matches) = {
        let execution_id = task_execution_id.to_string();
        let source_message_id = caller.source_message_id.map(str::to_string);
        db.with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?;
            let run = match &execution {
                Some(execution) => crate::db::orchestration::get_orchestration_run(
                    conn,
                    &execution.orchestration_run_id,
                )?,
                None => None,
            };
            let dispatch_matches = execution
                .as_ref()
                .map(|execution| {
                    native_worker_dispatch_matches(conn, execution, source_message_id.as_deref())
                })
                .transpose()?
                .unwrap_or(false);
            Ok((execution, run, dispatch_matches))
        })
        .await
        .map_err(|error| ProvisionError::Internal(error.to_string()))?
    };
    let Some(execution) = execution else {
        return Ok(ReviewOutcome::NotAddressed);
    };
    let provider_matches = execution
        .worker_agent_type
        .as_deref()
        .map(crate::db::orchestration::agent_type_from_db)
        .transpose()
        .map_err(|error| ProvisionError::Internal(error.to_string()))?
        .as_ref()
        == Some(caller.agent_type);
    let is_worker = execution.worker_cli_session_id.is_none()
        && execution.sub_discussion_id.as_deref() == Some(caller.discussion_id)
        && provider_matches
        && dispatch_matches;
    let is_principal = execution.parent_discussion_id == caller.discussion_id;
    if !is_worker && !is_principal {
        return Ok(ReviewOutcome::NotAddressed);
    }
    if is_worker && !run.as_ref().is_some_and(|run| run.allow_self_review) {
        return Ok(ReviewOutcome::SelfReviewForbidden);
    }
    decide_authorized_review(
        db,
        execution,
        caller.alias,
        caller.actor_session_id.map(str::to_string),
        is_principal,
        decision_json,
    )
    .await
}

/// Pin a native worker call to the exact durable dispatch that launched it.
/// Room + provider alone are not identities: another run of the same provider
/// can coexist in a child room. The trusted executor carries the dispatch
/// trigger message outside model-controlled arguments, and the execution pins
/// the corresponding job id across retries/restarts.
pub(crate) fn native_worker_dispatch_matches(
    conn: &rusqlite::Connection,
    execution: &TaskExecution,
    caller_source_message_id: Option<&str>,
) -> anyhow::Result<bool> {
    let (Some(dispatch_job_id), Some(source_message_id), Some(sub_discussion_id)) = (
        execution.dispatch_job_id.as_deref(),
        caller_source_message_id,
        execution.sub_discussion_id.as_deref(),
    ) else {
        return Ok(false);
    };
    let Some(job) = crate::db::agent_dispatch::get(conn, dispatch_job_id)? else {
        return Ok(false);
    };
    Ok(job.discussion_id == sub_discussion_id && job.trigger_message_id == source_message_id)
}

async fn decide_authorized_review(
    db: &Database,
    exec: TaskExecution,
    alias: &str,
    actor_session_id: Option<String>,
    reviewer_is_principal: bool,
    decision_json: &str,
) -> Result<ReviewOutcome, ProvisionError> {
    // Validate only after the caller has been authorized by its transport-specific
    // entry point, preserving the same anti-oracle behavior for CLI and native agents.
    let decision = match parse_review_decision(decision_json) {
        Ok(decision) => decision,
        Err(error) => return Ok(ReviewOutcome::InvalidDecision(error.to_string())),
    };

    // Approval and integration are two durable checkpoints. If Git refused
    // after the review row was committed (for example because the parent
    // checkout was dirty), a client retry must consume the existing approval
    // instead of demanding an impossible second review. The persisted row is
    // authoritative: only the same approve verdict resumes; request_changes
    // after approval remains a conflict and no review/event is duplicated.
    if exec.status == TaskExecutionStatus::Approved {
        if decision.decision != ReviewVerdict::Approve {
            return Ok(ReviewOutcome::NotReviewable {
                status: exec.status,
            });
        }
        let execution_id = exec.id.clone();
        let attempt_no = exec.attempt_no;
        let approved = db
            .with_conn(move |conn| {
                Ok(
                    crate::db::worker_reviews::get_review(conn, &execution_id, attempt_no)?
                        .is_some_and(|review| review.decision == "approve"),
                )
            })
            .await
            .map_err(|error| ProvisionError::Internal(error.to_string()))?;
        return if approved {
            Ok(ReviewOutcome::Reviewed {
                verdict: ReviewVerdict::Approve,
                execution: exec,
            })
        } else {
            Ok(ReviewOutcome::NotReviewable {
                status: exec.status,
            })
        };
    }

    // ── 5. approve is GUARDED (DoD-5): manifest present, HEAD not drifted, DoD met. ──
    if decision.decision == ReviewVerdict::Approve {
        if let Some(reason) = approve_guards(db, &exec, &decision, reviewer_is_principal).await? {
            return Ok(ReviewOutcome::ApproveBlocked { reason });
        }
    }

    // ── 6. Atomic review checkpoint (request_changes hands findings to the worker in the
    // child; approve touches neither child nor worktree). ──
    let outcome = {
        let eid = exec.id.clone();
        let attempt = exec.attempt_no;
        let verdict = decision.decision;
        let decision_owned = decision_json.to_string();
        let alias_owned = alias.to_string();
        let actor_session_id = actor_session_id.clone();
        // ── request_changes pre-build (owned to move into the DB closure); approve builds none
        // of this. The checkpoint decides re-offer vs escalate inside its tx (DoD-6). ──
        let (findings_owned, escalation_owned, reactivation_owned, native_dispatch_owned) =
            if verdict == ReviewVerdict::RequestChanges {
                let (parent, tid) = (exec.parent_discussion_id.clone(), exec.task_id.clone());
                let (parent_agent, task) = db
                    .with_conn(move |conn| {
                        let parent = crate::db::discussions::get_discussion(conn, &parent)?
                            .context("parent discussion vanished before review")?;
                        let task = crate::db::planning::get_task(conn, &tid)?
                            .context("task vanished before review")?;
                        Ok((parent.agent, task))
                    })
                    .await
                    .map_err(|e| ProvisionError::Internal(e.to_string()))?;
                let child = exec.sub_discussion_id.clone().unwrap_or_default();
                let worker_target = worker_target_from_execution(&exec)
                    .map_err(|e| ProvisionError::Internal(e.to_string()))?;

                // (a) findings → the worker in the child (DoD-4).
                let findings_msg =
                    build_review_findings_message(&exec.id, exec.attempt_no, &decision, &child);
                let findings = (child.clone(), findings_msg, worker_target.clone());

                // (b) escalation solicitation → the principal (used only if the budget is exhausted
                // inside the checkpoint tx).
                let escalation_msg = build_escalation_message(
                    &exec.id,
                    exec.attempt_no,
                    &task.summary.reference,
                    &task.summary.title,
                    exec.review_rounds + 1,
                    exec.max_review_rounds,
                    &child,
                );
                let escalation = (
                    exec.parent_discussion_id.clone(),
                    escalation_msg,
                    MessageTarget::discussion_agent(parent_agent),
                );

                // (c) re-offer → re-activate the CLI worker for the next attempt (DoD-9). Only for a
                // CLI worker (a native worker's re-dispatch is KT-335 — the review path is
                // CLI-centric in V1). The offer id is minted server-side FIRST so the control
                // message embeds the exact id; the checkpoint opens the offer with it in its tx.
                // Used only on the below-budget branch.
                let reactivation = if exec.worker_cli_session_id.is_some() {
                    let new_attempt = exec.attempt_no + 1;
                    let offer_id = Uuid::new_v4().to_string();
                    let control_msg = build_control_offer_message(
                        &exec.id,
                        new_attempt,
                        &offer_id,
                        &task.summary.reference,
                        &task.summary.title,
                        &child,
                    );
                    Some((
                        offer_id,
                        new_attempt,
                        worker_target.clone(),
                        child.clone(),
                        control_msg,
                    ))
                } else {
                    None
                };

                let native_dispatch = if exec.worker_cli_session_id.is_none() {
                    let new_attempt = exec.attempt_no + 1;
                    Some((
                        Uuid::new_v4().to_string(),
                        format!("orch-rework:{}:{}", exec.id, new_attempt),
                    ))
                } else {
                    None
                };

                (
                    Some(findings),
                    Some(escalation),
                    reactivation,
                    native_dispatch,
                )
            } else {
                (None, None, None, None)
            };
        db.with_conn(move |conn| {
            let actor = agent_actor(&alias_owned, actor_session_id.as_deref());
            let findings = findings_owned.as_ref().map(|(child, msg, target)| {
                crate::db::orchestration::ReviewFindingsDelivery {
                    child_discussion_id: child,
                    message: msg,
                    worker_target: target,
                }
            });
            let escalation = escalation_owned.as_ref().map(|(parent, msg, target)| {
                crate::db::orchestration::EscalationDelivery {
                    parent_discussion_id: parent,
                    message: msg,
                    principal_target: target,
                }
            });
            let reactivation =
                reactivation_owned
                    .as_ref()
                    .map(|(offer_id, new_attempt, target, child, msg)| {
                        crate::db::orchestration::ReworkReoffer {
                            offer_id,
                            new_attempt_no: *new_attempt,
                            target_cli_session_id: target
                                .cli_session_id
                                .expect("a CLI worker target carries a session pk"),
                            sub_discussion_id: child,
                            control_message: msg,
                            control_target: target,
                        }
                    });
            let native_dispatch = native_dispatch_owned.as_ref().map(|(job_id, dedupe_key)| {
                crate::db::orchestration::NativeReworkDispatch { job_id, dedupe_key }
            });
            crate::db::orchestration::commit_review_checkpoint(
                conn,
                &crate::db::orchestration::ReviewCheckpoint {
                    exec_id: &eid,
                    attempt_no: attempt,
                    verdict,
                    decision_json: &decision_owned,
                    findings,
                    escalation,
                    reactivation,
                    native_dispatch,
                    actor: &actor,
                },
            )
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?
    };

    use crate::db::orchestration::ReviewCheckpointOutcome;
    match outcome {
        ReviewCheckpointOutcome::Approved | ReviewCheckpointOutcome::ChangesRequested => {
            let eid = exec.id.clone();
            let execution = db
                .with_conn(move |conn| {
                    crate::db::orchestration::get_task_execution(conn, &eid)?
                        .context("execution vanished right after the review checkpoint")
                })
                .await
                .map_err(|e| ProvisionError::Internal(e.to_string()))?;
            Ok(ReviewOutcome::Reviewed {
                verdict: decision.decision,
                execution,
            })
        }
        ReviewCheckpointOutcome::Escalated => {
            let eid = exec.id.clone();
            let execution = db
                .with_conn(move |conn| {
                    crate::db::orchestration::get_task_execution(conn, &eid)?
                        .context("execution vanished right after the escalation checkpoint")
                })
                .await
                .map_err(|e| ProvisionError::Internal(e.to_string()))?;
            Ok(ReviewOutcome::Escalated { execution })
        }
        ReviewCheckpointOutcome::NotReviewable { status } => {
            Ok(ReviewOutcome::NotReviewable { status })
        }
        ReviewCheckpointOutcome::ExecutionRaced => {
            // Lost the CAS to a concurrent decider — report the real (new) state honestly.
            let eid = exec.id.clone();
            let refreshed = db
                .with_conn(move |conn| crate::db::orchestration::get_task_execution(conn, &eid))
                .await
                .map_err(|e| ProvisionError::Internal(e.to_string()))?;
            let status = refreshed.map(|e| e.status).unwrap_or(exec.status);
            Ok(ReviewOutcome::NotReviewable { status })
        }
    }
}

/// A successful approval is the integration trigger, not a durable parking
/// state. Keep the review checkpoint separate (so it stays idempotent and
/// auditable), then consume that checkpoint through the same protected saga for
/// every transport that can review: joined CLIs, native HTTP agents and humans.
///
/// Returning the refreshed execution matters: callers must see `Done`, a
/// validation send-back, or an explicit integration refusal — never a silent
/// `Approved` row that no public tool can advance.
pub(crate) async fn continue_approved_review(
    db: &Database,
    outcome: ReviewOutcome,
) -> Result<ReviewOutcome, ProvisionError> {
    let execution = match outcome {
        ReviewOutcome::Reviewed {
            verdict: ReviewVerdict::Approve,
            execution,
        } => execution,
        other => return Ok(other),
    };

    let execution_id = execution.id.clone();
    if let IntegrationOutcome::Refused { reason } = run_integration(db, &execution_id).await? {
        return Err(ProvisionError::NotLaunchable(format!(
            "review approved, but protected integration was refused: {reason}"
        )));
    }

    let refreshed_id = execution_id.clone();
    let refreshed = db
        .with_conn(move |conn| {
            crate::db::orchestration::get_task_execution(conn, &refreshed_id)?
                .context("execution vanished right after protected integration")
        })
        .await
        .map_err(|error| ProvisionError::Internal(error.to_string()))?;
    Ok(ReviewOutcome::Reviewed {
        verdict: ReviewVerdict::Approve,
        execution: refreshed,
    })
}

/// The DoD-5 approve guard. Returns `Some(reason)` if approve must be refused, `None` if the
/// delivery is present, its reported DoD are all met, and the worktree HEAD has NOT drifted
/// from the delivered `head_sha`. Both shas pass through `resolve_commit`, so an abbreviated
/// delivered sha and the full worktree HEAD compare equal — the abbreviated case never
/// false-refuses (the one case that distinguishes normalization from a raw string compare).
async fn approve_guards(
    db: &Database,
    exec: &TaskExecution,
    decision: &ReviewDecisionV1,
    reviewer_is_principal: bool,
) -> Result<Option<ApproveBlockReason>, ProvisionError> {
    let (delivery, workspace, task) = {
        let eid = exec.id.clone();
        let task_id = exec.task_id.clone();
        let attempt = exec.attempt_no;
        db.with_conn(move |conn| {
            let d = crate::db::worker_deliveries::get_delivery(conn, &eid, attempt)?;
            let w = crate::db::discussion_workspaces::get_managed_for_execution(conn, &eid)?;
            let task = crate::db::planning::get_task(conn, &task_id)?;
            Ok((d, w, task))
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?
    };
    let Some(delivery) = delivery else {
        return Ok(Some(ApproveBlockReason::NoManifest));
    };
    // The stored manifest was validated on insert; a corrupt stored one is an internal error,
    // not a business refusal.
    let manifest = parse_delivery_manifest(&delivery.manifest_json)
        .map_err(|e| ProvisionError::Internal(format!("stored manifest is corrupt: {e}")))?;
    let Some(task) = task else {
        return Ok(Some(ApproveBlockReason::ManifestClaimsInvalid(
            "execution task no longer exists".into(),
        )));
    };
    if let Err(detail) = validate_manifest_claims(&task.definition_of_done, &manifest) {
        return Ok(Some(ApproveBlockReason::ManifestClaimsInvalid(detail)));
    }
    if !reviewer_is_principal && !decision.dod_verifications.is_empty() {
        return Ok(Some(ApproveBlockReason::ReviewEvidenceInvalid(
            "a worker review cannot supply principal DoD evidence".into(),
        )));
    }
    let task_dod_ids = task
        .definition_of_done
        .iter()
        .map(|item| item.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    if let Some(foreign) = decision
        .dod_verifications
        .iter()
        .find(|verification| !task_dod_ids.contains(verification.dod_id.as_str()))
    {
        return Ok(Some(ApproveBlockReason::ReviewEvidenceInvalid(format!(
            "review evidence references unknown DoD id `{}`",
            foreign.dod_id
        ))));
    }
    if reviewer_is_principal {
        let submitted = decision
            .dod_verifications
            .iter()
            .map(|verification| verification.dod_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let missing = task
            .definition_of_done
            .iter()
            .filter(|item| !submitted.contains(item.id.as_str()))
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Ok(Some(ApproveBlockReason::ReviewEvidenceInvalid(format!(
                "principal approval must cover every DoD item; missing {}",
                missing.join(", ")
            ))));
        }
        let rejected = decision
            .dod_verifications
            .iter()
            .filter(|verification| !verification.met)
            .map(|verification| verification.dod_id.clone())
            .collect::<Vec<_>>();
        if !rejected.is_empty() {
            return Ok(Some(ApproveBlockReason::DodNotMet { unmet: rejected }));
        }
    }
    let verified = if reviewer_is_principal {
        decision
            .dod_verifications
            .iter()
            .filter(|verification| verification.met)
            .map(|verification| verification.dod_id.as_str())
            .collect::<std::collections::HashSet<_>>()
    } else {
        std::collections::HashSet::new()
    };
    let unmet = manifest
        .dod_status
        .iter()
        .filter(|status| !status.met && !verified.contains(status.dod_id.as_str()))
        .map(|d| d.dod_id.clone())
        .collect::<Vec<_>>();
    if !unmet.is_empty() {
        return Ok(Some(ApproveBlockReason::DodNotMet { unmet }));
    }
    // HEAD drift (DoD-5): the reviewed state must still be the worktree HEAD. resolve_commit on
    // BOTH sides normalizes short↔long, so an abbreviated delivered sha compares equal.
    let Some(path) = workspace.and_then(|w| w.canonical_path) else {
        return Ok(Some(ApproveBlockReason::WorktreeUnavailable(
            "no managed worktree for the execution".to_string(),
        )));
    };
    let repo = std::path::Path::new(&path);
    let dirty = match worktree::worktree_dirty_files(repo) {
        Ok(files) => files,
        Err(error) => return Ok(Some(ApproveBlockReason::WorktreeUnavailable(error))),
    };
    if !dirty.is_empty() {
        return Ok(Some(ApproveBlockReason::WorktreeDirty {
            files: dirty
                .into_iter()
                .map(|file| format!("{} {}", file.status, file.path))
                .collect(),
        }));
    }
    let delivered = match worktree::resolve_commit(repo, &delivery.head_sha) {
        Ok(sha) => sha,
        Err(e) => return Ok(Some(ApproveBlockReason::WorktreeUnavailable(e))),
    };
    let current = match worktree::resolve_commit(repo, "HEAD") {
        Ok(sha) => sha,
        Err(e) => return Ok(Some(ApproveBlockReason::WorktreeUnavailable(e))),
    };
    if delivered != current {
        return Ok(Some(ApproveBlockReason::HeadDrifted { delivered, current }));
    }
    let Some(reviewed_head_sha) = decision.reviewed_head_sha.as_deref() else {
        return Ok(Some(ApproveBlockReason::ReviewedHeadMismatch {
            reviewed: "missing".into(),
            delivered,
        }));
    };
    let reviewed = match worktree::resolve_commit(repo, reviewed_head_sha) {
        Ok(sha) => sha,
        Err(error) => {
            return Ok(Some(ApproveBlockReason::ReviewedHeadMismatch {
                reviewed: format!("unresolvable: {error}"),
                delivered,
            }))
        }
    };
    if reviewed != delivered {
        return Ok(Some(ApproveBlockReason::ReviewedHeadMismatch {
            reviewed,
            delivered,
        }));
    }
    let Some(base) = exec.base_sha.as_deref() else {
        return Ok(Some(ApproveBlockReason::WorktreeUnavailable(
            "execution has no pinned base_sha".into(),
        )));
    };
    let facts = match delivery_git_facts_from_repo(repo.to_path_buf(), base) {
        Ok(facts) => facts,
        Err(error) => {
            return Ok(Some(ApproveBlockReason::WorktreeUnavailable(error)));
        }
    };
    if let Err(detail) = validate_committed_file_inventory(&facts, &manifest) {
        return Ok(Some(ApproveBlockReason::ManifestDiffMismatch(detail)));
    }
    Ok(None)
}

/// The structured findings hand-off posted to the worker in the CHILD on request_changes
/// (DoD-4): the principal's comment + structured findings, telling the worker to fix them in
/// the SAME worktree and re-deliver. Deterministic id per `(exec, attempt)` so a resume never
/// double-posts.
fn build_review_findings_message(
    exec_id: &str,
    attempt_no: u32,
    decision: &ReviewDecisionV1,
    _child_discussion_id: &str,
) -> DiscussionMessage {
    let comment = decision.comment.as_deref().unwrap_or("").trim();
    let findings = if decision.findings.is_empty() {
        "_(aucun point structuré — voir le commentaire)_".to_string()
    } else {
        decision
            .findings
            .iter()
            .map(|f: &ReviewFinding| {
                let loc = match (&f.path, f.line) {
                    (Some(p), Some(l)) => format!("`{p}:{l}` — "),
                    (Some(p), None) => format!("`{p}` — "),
                    _ => String::new(),
                };
                format!("- {loc}{issue}", issue = f.issue)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let content = format!(
        "**Changements demandés — revue du tour livré**\n\n\
         Le principal a renvoyé ton travail avec `request_changes`. Corrige les points \
         ci-dessous **dans ce worktree** (il est conservé, comme cette sous-discussion), \
         puis re-livre un nouveau DeliveryManifest via `task_exec_deliver`.\n\n\
         ## Commentaire\n{comment}\n\n\
         ## Points à traiter\n{findings}",
        comment = if comment.is_empty() {
            "_(voir les points ci-dessous)_"
        } else {
            comment
        },
        findings = findings,
    );
    orchestrator_message(
        format!("orch-review-findings:{exec_id}:{attempt_no}"),
        content,
    )
}

/// Phase A body (one SQLite commit): validate the task (DoD-1), resolve its single
/// project + repo path, launch the idempotent execution and move it to
/// `Provisioning`. Returns the business refusal in the inner `Err` (DB errors in
/// the outer `Err`).
fn begin_provisioning(
    conn: &rusqlite::Connection,
    task_reference: &str,
    parent_discussion_id: &str,
    worker: &MessageTarget,
    context: BeginProvisioningContext<'_>,
    actor: &OrchestrationActor,
) -> Result<Result<Prepared, ProvisionError>> {
    let BeginProvisioningContext {
        target_branch,
        idempotency_key,
        validations,
        campaign,
        resume_execution_id,
        worker_scope,
    } = context;
    let task = match crate::db::planning::get_task(conn, task_reference)? {
        Some(t) => t,
        None => return Ok(Err(ProvisionError::TaskNotFound)),
    };

    // An existing active execution matched by idempotency key is a RESUME/replay,
    // not a fresh launch: it must BYPASS the "task is Todo" gate (a prior attempt
    // legitimately advanced the task) and reuse the very same execution. The final
    // checkpoint CAS remains the anti-race authority for the Todo → InProgress flip.
    let existing = crate::db::orchestration::get_active_execution_for_task(conn, &task.summary.id)?;
    let idempotent_replay = matches!(
        (&existing, idempotency_key),
        (Some(e), Some(key)) if e.idempotency_key.as_deref() == Some(key)
    );
    let explicit_resume = matches!(
        (&existing, resume_execution_id),
        (Some(execution), Some(expected)) if execution.id == expected
    );
    if resume_execution_id.is_some() && !explicit_resume {
        return Ok(Err(ProvisionError::NotLaunchable(
            "the requested recovery execution is no longer the task's active execution".into(),
        )));
    }
    let is_replay = idempotent_replay || explicit_resume;

    if let Some(scope) = worker_scope {
        if let Err(reason) = scope.validate() {
            return Ok(Err(ProvisionError::NotLaunchable(reason)));
        }
        if worker.kind != MessageTargetKind::DiscussionAgent
            || !crate::agents::runner::is_http_chat_agent(&worker.agent_type)
        {
            return Ok(Err(ProvisionError::NotLaunchable(
                "worker_scope is supported only by native HTTP discussion_agent workers".into(),
            )));
        }
    }
    if is_replay
        && worker_scope.is_some()
        && existing
            .as_ref()
            .is_some_and(|execution| execution.worker_scope.as_ref() != worker_scope)
    {
        return Ok(Err(ProvisionError::NotLaunchable(
            "idempotent replay cannot change the persisted worker_scope".into(),
        )));
    }

    // Fail closed before the execution row, sub-discussion or worktree exists.
    // The public preflight is advisory UX; this is the authoritative boundary
    // for direct, campaign and future internal callers.
    if !is_replay {
        if let Some(reason) = worker_launch_refusal(conn, parent_discussion_id, worker)? {
            return Ok(Err(ProvisionError::NotLaunchable(format!(
                "{}: {}",
                reason.code, reason.detail
            ))));
        }
    }

    // Fresh-launch DoD-1 gate (skipped on a replay).
    if !is_replay {
        if task.summary.status != PlanningTaskStatus::Todo {
            return Ok(Err(ProvisionError::NotLaunchable(format!(
                "task {task_reference} is {:?}, not Todo",
                task.summary.status
            ))));
        }
        if task.definition_of_done.is_empty() {
            return Ok(Err(ProvisionError::NotLaunchable(
                "task has no Definition of Done to work against".into(),
            )));
        }
    }

    // Project + repo path — needed by every proceed path.
    let project_id = match task.summary.project_ids.as_slice() {
        [one] => one.clone(),
        [] => {
            return Ok(Err(ProvisionError::NotLaunchable(
                "task has no project — cannot resolve a repository".into(),
            )))
        }
        _ => {
            return Ok(Err(ProvisionError::NotLaunchable(
                "task maps to several projects — ambiguous repository".into(),
            )))
        }
    };
    let project = match crate::db::projects::get_project(conn, &project_id)? {
        Some(p) => p,
        None => {
            return Ok(Err(ProvisionError::NotLaunchable(
                "task's project no longer exists".into(),
            )))
        }
    };
    let repo_path = scanner::resolve_host_path(&project.path)
        .to_string_lossy()
        .to_string();

    // The child checkout is SHA-pinned later, but the integration target must
    // stay a named local branch. A raw SHA/tag is immutable and would make the
    // apply drift check observe that object forever instead of the real parent.
    let target_branch = if is_replay {
        target_branch.unwrap_or("main").to_string()
    } else {
        match worktree::resolve_local_branch(
            std::path::Path::new(&repo_path),
            target_branch.unwrap_or("main"),
        ) {
            Ok(branch) => branch,
            Err(reason) => return Ok(Err(ProvisionError::NotLaunchable(reason))),
        }
    };

    if !is_replay {
        let active_blockers = task
            .blockers
            .iter()
            .filter(|b| {
                !matches!(
                    b.status,
                    PlanningTaskStatus::Done | PlanningTaskStatus::Archived
                )
            })
            .count();
        if active_blockers > 0 {
            return Ok(Err(ProvisionError::NotLaunchable(format!(
                "task has {active_blockers} active blocker(s)"
            ))));
        }
        // A *different* active execution means a concurrent launch — refuse.
        if existing.is_some() {
            return Ok(Err(ProvisionError::NotLaunchable(
                "task already has an active execution".into(),
            )));
        }
    }

    // Launch (idempotent) with the typed worker identity.
    let mut launch =
        crate::models::LaunchSingleTaskInput::new(&task.summary.id, parent_discussion_id);
    launch.project_id = Some(project_id.clone());
    // A single-task launch uses the pinned base revision as its integration
    // target (default `main`). Campaigns reuse their already-persisted run, so
    // this field is ignored by `launch_task_in_run` there.
    launch.target_branch = Some(target_branch);
    launch.validations = validations.to_vec();
    launch.worker_target_kind = Some(worker.kind);
    launch.worker_cli_session_id = worker.cli_session_id;
    launch.worker_agent_type = Some(crate::db::orchestration::agent_type_to_db(
        &worker.agent_type,
    ));
    launch.worker_model_tier = worker
        .tier
        .as_ref()
        .map(|t| model_tier_to_db(t).to_string());
    launch.worker_scope = worker_scope.cloned();
    launch.worker_dod_ids = Some(
        task.definition_of_done
            .iter()
            .map(|item| item.id.clone())
            .collect(),
    );
    if let Some(campaign) = campaign {
        launch.worker_model = campaign.selection.model.clone();
        launch.worker_profile_id = campaign.selection.profile_id.clone();
    }
    launch.idempotency_key = idempotency_key.map(|s| s.to_string());
    let execution = if explicit_resume {
        existing.expect("explicit resume matched the active execution")
    } else if let Some(campaign) = campaign {
        let run = crate::db::orchestration::get_orchestration_run(conn, &campaign.run_id)?
            .context("campaign vanished before launch")?;
        launch.max_review_rounds = run.max_review_rounds;
        crate::db::orchestration::launch_task_in_run(
            conn,
            &campaign.run_id,
            &launch,
            &campaign.selection,
            actor,
        )?
        .execution
    } else {
        crate::db::orchestration::launch_single_task(conn, &launch, actor)?.execution
    };

    let make = |execution: TaskExecution, already_launched: bool| Prepared {
        execution,
        project_id: project_id.clone(),
        repo_path: repo_path.clone(),
        task_reference: task.summary.reference.clone(),
        task_title: task.summary.title.clone(),
        task_description: task.description.clone(),
        dod: task.definition_of_done.clone(),
        already_launched,
    };

    // A replay of an already-running/terminal execution returns untouched.
    if !matches!(
        execution.status,
        TaskExecutionStatus::Pending
            | TaskExecutionStatus::Provisioning
            | TaskExecutionStatus::Blocked
            | TaskExecutionStatus::Interrupted
    ) {
        return Ok(Ok(make(execution, true)));
    }
    let execution = advance_to_provisioning(conn, execution, actor)?;
    Ok(Ok(make(execution, false)))
}

/// Move a freshly launched (`Pending`) or resumed (`Blocked` from a prior
/// `Provisioning` hold) execution into `Provisioning`. Already-`Provisioning` is a
/// no-op resume.
fn advance_to_provisioning(
    conn: &rusqlite::Connection,
    execution: TaskExecution,
    actor: &OrchestrationActor,
) -> Result<TaskExecution> {
    use TaskExecutionStatus::*;
    match execution.status {
        Provisioning => Ok(execution),
        Pending | Blocked => {
            crate::db::orchestration::transition_execution(
                conn,
                &execution.id,
                Provisioning,
                actor,
                serde_json::json!({ "phase": "provisioning_start" }),
            )?;
            crate::db::orchestration::get_task_execution(conn, &execution.id)?
                .context("execution vanished after the Provisioning transition")
        }
        Interrupted => {
            let origin = execution
                .interrupted_from_status
                .context("interrupted provisioning execution has no durable origin")?;
            if origin == Blocked {
                crate::db::orchestration::transition_execution(
                    conn,
                    &execution.id,
                    Blocked,
                    actor,
                    serde_json::json!({ "phase": "restore_provisioning_block" }),
                )?;
            }
            crate::db::orchestration::transition_execution(
                conn,
                &execution.id,
                Provisioning,
                actor,
                serde_json::json!({ "phase": "resume_provisioning" }),
            )?;
            crate::db::orchestration::get_task_execution(conn, &execution.id)?
                .context("execution vanished after recovered Provisioning transition")
        }
        other => bail!(
            "execution {} is {:?}, not resumable into Provisioning",
            execution.id,
            other
        ),
    }
}

async fn mark_blocked(db: &Database, exec_id: &str, reason: String) {
    let exec_id = exec_id.to_string();
    let _ = db
        .with_conn(move |conn| {
            // A native checkpoint-refused block carries no structured code in V1.
            crate::db::orchestration::block_execution(
                conn,
                &exec_id,
                &backend_actor(),
                &reason,
                None,
            )
        })
        .await;
}

fn model_tier_to_db(t: &ModelTier) -> &'static str {
    match t {
        ModelTier::Economy => "economy",
        ModelTier::Default => "default",
        ModelTier::Reasoning => "reasoning",
    }
}

fn model_tier_from_db(s: &str) -> Option<ModelTier> {
    match s {
        "economy" => Some(ModelTier::Economy),
        "default" => Some(ModelTier::Default),
        "reasoning" => Some(ModelTier::Reasoning),
        _ => None,
    }
}

/// Reconstruct the durable worker target from the persisted execution identity —
/// the authoritative source for the dispatch (never whatever the caller re-passed
/// on a retry), so the worker the checkpoint dispatches always matches what launch
/// pinned. Fails loudly on a corrupt/incomplete identity row.
fn worker_target_from_execution(execution: &TaskExecution) -> Result<MessageTarget> {
    let kind = execution
        .worker_target_kind
        .context("execution has no worker_target_kind — cannot reconstruct the worker")?;
    let agent_type = crate::db::orchestration::agent_type_from_db(
        execution
            .worker_agent_type
            .as_deref()
            .context("execution has no worker_agent_type")?,
    )?;
    let tier = execution
        .worker_model_tier
        .as_deref()
        .and_then(model_tier_from_db);
    Ok(MessageTarget {
        kind,
        agent_type,
        cli_session_id: execution.worker_cli_session_id,
        tier,
    })
}

/// A fresh, empty sub-discussion owned by the execution. Its breadcrumb to the
/// principal room + task lives on the `task_executions` row (parent_discussion_id,
/// sub_discussion_id, task_id) and is queryable via `get_execution_lineage` — the
/// task is never duplicated as a second source of truth (DoD-4). Created empty; the
/// brief is inserted only in the final atomic checkpoint.
fn build_sub_discussion(prepared: &Prepared, worker: &MessageTarget) -> Discussion {
    let now = chrono::Utc::now();
    Discussion {
        awaiting_agent: false,
        agent_running: false,
        id: Uuid::new_v4().to_string(),
        project_id: Some(prepared.project_id.clone()),
        title: format!("{} — {}", prepared.task_reference, prepared.task_title),
        agent: worker.agent_type.clone(),
        language: "en".to_string(),
        participants: vec![worker.agent_type.clone()],
        messages: vec![],
        message_count: 0,
        non_system_message_count: 0,
        skill_ids: vec![],
        profile_ids: prepared
            .execution
            .worker_profile_id
            .iter()
            .cloned()
            .collect(),
        directive_ids: vec![],
        tier: worker.tier.unwrap_or_default(),
        model: prepared.execution.worker_model.clone(),
        // The first message is the versioned worker brief: objective, ordered
        // DoD and delivery contract. A resumed local worker must
        // never lose it merely because later tool traces filled the context.
        // Measured on KT-404, this costs 5,306 chars of a 32,540-char Ollama
        // prompt budget: material but bounded, and smaller than reconstructing
        // a lost protocol from repeated failed turns.
        pin_first_message: true,
        archived: false,
        pinned: false,
        workspace_mode: "Isolated".to_string(),
        workspace_path: None,
        worktree_branch: None,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: SummaryStrategy::default(),
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    }
}

/// The worker brief (KT-318 DoD-6): objective, DoD, decisions/scope, constraints,
/// tests, workspace and the concrete delivery format — the focused context, not
/// the whole plan. Its id is deterministic per `(execution, attempt)` so a
/// re-committed checkpoint can never silently double-post (the PK would reject it).
fn build_brief(
    prepared: &Prepared,
    worktree_path: &str,
    branch: &str,
    base_sha: &str,
) -> DiscussionMessage {
    let can_run_shell = !matches!(
        prepared.execution.worker_agent_type.as_deref(),
        Some("Ollama" | "LiteLlm" | "Nvidia")
    );
    let content = worker_brief_markdown(
        &prepared.task_reference,
        &prepared.task_title,
        &prepared.task_description,
        &prepared.dod,
        worktree_path,
        branch,
        base_sha,
        can_run_shell,
        true,
        prepared.execution.worker_scope.as_ref(),
    );
    orchestrator_message(
        format!(
            "orch-brief:{}:{}",
            prepared.execution.id, prepared.execution.attempt_no
        ),
        content,
    )
}

/// The CLI-worker brief, rebuilt at ACCEPTANCE time (KT-328 tranche 2) from the
/// execution + task + managed workspace — the native path builds the same content in
/// [`build_brief`]; both share [`worker_brief_markdown`] so the two can never diverge.
/// The id is the same deterministic `orch-brief:{exec}:{attempt}` so a re-committed
/// checkpoint can never double-post (the message PK rejects it).
#[allow(clippy::too_many_arguments)]
fn build_cli_worker_brief(
    exec_id: &str,
    attempt_no: u32,
    task_reference: &str,
    task_title: &str,
    task_description: &str,
    dod: &[PlanningDodItem],
    worktree_path: &str,
    branch: &str,
    base_sha: &str,
) -> DiscussionMessage {
    let content = worker_brief_markdown(
        task_reference,
        task_title,
        task_description,
        dod,
        worktree_path,
        branch,
        base_sha,
        true,
        false,
        None,
    );
    orchestrator_message(format!("orch-brief:{exec_id}:{attempt_no}"), content)
}

/// The durable "session attached" notice posted in the ORIGIN room at acceptance
/// (KT-328 DoD-6): the worker session just LEFT this room for the sub-discussion, so
/// its departure must be visible, never silent. Deterministic id so a resume never
/// double-posts.
fn build_attach_notice(
    exec_id: &str,
    attempt_no: u32,
    task_reference: &str,
    task_title: &str,
    child_discussion_id: &str,
    worker_alias: Option<&str>,
) -> DiscussionMessage {
    let who = worker_alias
        .map(|a| format!("La session worker `{a}`"))
        .unwrap_or_else(|| "La session worker".to_string());
    let content = format!(
        "{who} a accepté **{reference} : {title}** et a rejoint sa sous-discussion \
         `{child}`. Elle n'est plus présente dans cette room ; le suivi de la tâche \
         se poursuit dans la sous-discussion.",
        who = who,
        reference = task_reference,
        title = task_title,
        child = child_discussion_id,
    );
    orchestrator_message(format!("orch-attach:{exec_id}:{attempt_no}"), content)
}

/// The principal-targeted review request posted in the PARENT room when a worker delivers
/// (KT-319 DoD-3). NOT the work brief: it tells the principal a delivery awaits its review
/// and names the two possible decisions. Deterministic id per `(exec, attempt)` so a resume
/// never double-posts (the message PK rejects it).
fn build_review_request_message(
    exec_id: &str,
    attempt_no: u32,
    task_reference: &str,
    task_title: &str,
    child_discussion_id: &str,
    head_sha: &str,
    summary: &str,
) -> DiscussionMessage {
    let content = format!(
        "**Revue demandée — {reference} : {title}**\n\n\
         Le worker a livré son travail (HEAD `{head}`) dans la sous-discussion `{child}` \
         et soumis un DeliveryManifest. Décide via `task_exec_review` :\n\
         - `approve` — le travail est intégré (refusé si le HEAD a dérivé depuis la \
         livraison — le manifeste épingle l'état exact revu) ;\n\
         - `request_changes` — renvoyé au worker avec un `comment` actionnable.\n\n\
         Résumé du worker : {summary}",
        reference = task_reference,
        title = task_title,
        head = head_sha,
        child = child_discussion_id,
        summary = summary,
    );
    orchestrator_message(
        format!("orch-review-request:{exec_id}:{attempt_no}"),
        content,
    )
}

/// The escalation solicitation posted to the PRINCIPAL in the parent room when the review budget
/// is exhausted (KT-319 DoD-6). It tells the principal the run has paused in `Escalated` and names
/// the human choices — it does NOT re-offer. Deterministic id per `(exec, attempt)` so a resume
/// never double-posts.
fn build_escalation_message(
    exec_id: &str,
    attempt_no: u32,
    task_reference: &str,
    task_title: &str,
    review_rounds: u32,
    max_review_rounds: u32,
    child_discussion_id: &str,
) -> DiscussionMessage {
    let content = format!(
        "**Escalade — budget de revue épuisé ({rounds}/{max}) — {reference} : {title}**\n\n\
         Le worker vient de recevoir `request_changes` et le nombre de tours de revue autorisés \
         est atteint. L'exécution est mise en pause en `Escalated` : elle ne sera **pas** \
         ré-offerte automatiquement — une **décision humaine** est requise. Options :\n\
         - reprendre la main sur la sous-discussion `{child}` (piloter le worker directement) ;\n\
         - relever la limite de tours du run puis ré-offrir explicitement ;\n\
         - basculer sur un worker natif, ou abandonner l'exécution.",
        rounds = review_rounds,
        max = max_review_rounds,
        reference = task_reference,
        title = task_title,
        child = child_discussion_id,
    );
    orchestrator_message(format!("orch-escalation:{exec_id}:{attempt_no}"), content)
}

/// Shared worker-brief markdown (KT-318 DoD-6: objective, DoD, decisions/scope,
/// constraints, tests, workspace and the concrete DeliveryManifest v1 delivery
/// format) — the focused context, not the whole plan. Native and CLI briefs both
/// render through this so their lifecycle contract stays aligned while the
/// concrete method names the tools each transport actually exposes.
#[allow(clippy::too_many_arguments)]
fn worker_brief_markdown(
    task_reference: &str,
    task_title: &str,
    task_description: &str,
    dod: &[PlanningDodItem],
    worktree_path: &str,
    branch: &str,
    base_sha: &str,
    can_run_shell: bool,
    native_delivery_projection: bool,
    worker_scope: Option<&TaskWorkerScope>,
) -> String {
    let mediated_host_commit = can_run_shell && native_delivery_projection;
    let dod = if dod.is_empty() {
        "_(aucune)_".to_string()
    } else {
        if native_delivery_projection {
            dod.iter()
                .enumerate()
                .map(|(index, item)| format!("{}. [ ] {}", index + 1, item.sentence))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            dod.iter()
                .map(|item| format!("- [ ] `{}` — {}", item.id, item.sentence))
                .collect::<Vec<_>>()
                .join("\n")
        }
    };
    let objective = if task_description.trim().is_empty() {
        task_title.to_string()
    } else {
        task_description.to_string()
    };
    if !can_run_shell {
        if let Some(scope) = worker_scope {
            let (target, mutation_tool, mutation_instruction) = match scope {
                TaskWorkerScope::PrelocalizedEdit {
                    path,
                    start_line,
                    end_line,
                } => (
                    format!(
                        "- Fichier : `{path}`\n- Plage inclusive : `{start_line}..={end_line}`"
                    ),
                    "edit_lines",
                    "remplacer uniquement la plage gelée",
                ),
                TaskWorkerScope::PrelocalizedInsertAfter { path, anchor_line } => (
                    format!(
                        "- Fichier : `{path}`\n- Insertion après la ligne d'ancre : `{anchor_line}`"
                    ),
                    "insert_after_line",
                    "fournir uniquement le nouveau texte ; l'ancre est préservée mécaniquement",
                ),
            };
            let delivery = if native_delivery_projection {
                "Dans `task_exec_deliver`, fournis uniquement `tests`, `dod_status` dans l'ordre, \
                 `docs`, `migrations`, `risks`, `limitations` et `summary`. Kronn injecte les \
                 identifiants, le SHA et l'inventaire depuis le worktree réel."
            } else {
                "Dans `task_exec_deliver`, remplis le DeliveryManifest v1 complet selon le schéma \
                 déclaré par l'outil."
            };
            return format!(
                "# {task_reference} — {task_title}\n\n\
                 ## Objectif\n{objective}\n\n\
                 ## Definition of Done\n{dod}\n\n\
                 ## Cible mécanique prélocalisée\n{target}\n\n\
                 ## Protocole borné\n\
                 1. Appelle l'unique `read_file` contraint.\n\
                 2. Kronn retire définitivement les outils de lecture ; appelle l'unique \
                    `{mutation_tool}` avec le `content_sha256` reçu pour {mutation_instruction}.\n\
                 3. Appelle `git_commit` avec ce seul fichier.\n\
                 4. Appelle `task_exec_deliver`.\n\n\
                 Ne cherche pas ailleurs, ne relis pas et n'élargis jamais la cible. Kronn expose \
                 un seul `read_file` contraint. Si la fenêtre ne suffit pas, indique le \
                 blocker exact : aucun octet ne doit être deviné. Tu n'as pas de shell : \
                 n'affirme aucune validation exécutée ; reporte-la `skipped` avec la commande \
                 que le principal devra lancer. Pas de push, merge ou force-push.\n\n\
                 {delivery}\n\n\
                 Commence maintenant avec `read_file` ; le worktree neuf et la cible sont déjà \
                 établis, il n'y a rien à inventorier."
            );
        }
    }
    let tests = if can_run_shell {
        "Exécute les commandes de validation adaptées au projet et reporte chaque résultat \
         (`pass`/`fail`/`skipped` + preuve non vide) dans `tests`. Un `pass` sans \
         commande ou sortie vérifiable est refusé."
            .to_string()
    } else {
        "Tu n'as pas de shell : n'affirme jamais avoir exécuté `cargo`, `pnpm`, `make` \
         ou une autre commande. Tu peux écrire les tests nécessaires, puis reporte les \
         commandes que le principal doit exécuter avec `status: skipped` et une raison \
         non vide. Le principal exécutera ces validations avant l'approbation et joindra \
         sa preuve au `ReviewDecision`."
            .to_string()
    };
    let (method, first_action, mechanical_scope) = match worker_scope {
        Some(TaskWorkerScope::PrelocalizedEdit {
            path,
            start_line,
            end_line,
        }) => (
            format!(
                "Cette exécution est mécaniquement prélocalisée. Kronn expose d'abord un seul \
                 `read_file` contraint à la fenêtre gelée autour de `{path}:{start_line}-{end_line}`, \
                 puis retire définitivement les outils de lecture et expose uniquement `edit_lines` \
                 sur cette même plage. Ne cherche pas ailleurs, ne relis pas, et ne change ni le \
                 fichier ni la plage. Si la fenêtre ne suffit pas à produire \
                 une édition sûre, indique le blocker exact : Kronn rendra la main au principal. \
                 Après une édition acceptée, committe puis livre."
            ),
            "Premier appel : utilise l'unique `read_file` déclaré avec ses arguments déjà \
             contraints. Au tour suivant, appelle immédiatement l'unique `edit_lines` déclaré \
             avec le `content_sha256` frais reçu."
                .to_string(),
            format!(
                "## Cible mécanique prélocalisée\n- Fichier : `{path}`\n- Plage inclusive : \
                 `{start_line}..={end_line}`\n- La cible est persistée et ne peut pas être \
                 élargie par le worker ; le hash vient de la lecture réelle du worktree.\n\n"
            ),
        ),
        Some(TaskWorkerScope::PrelocalizedInsertAfter { path, anchor_line }) => (
            format!(
                "Cette exécution est mécaniquement prélocalisée pour une insertion. Kronn expose \
                 d'abord un seul `read_file` contraint à la fenêtre gelée autour de \
                 `{path}:{anchor_line}`, puis retire définitivement les outils de lecture et \
                 expose uniquement `insert_after_line` sur cette ancre. Ne cherche pas ailleurs, \
                 ne relis pas et ne change ni le fichier ni l'ancre. L'outil préserve \
                 mécaniquement la ligne d'ancre : fournis uniquement le texte à ajouter. Si la \
                 fenêtre ne suffit pas à produire une insertion sûre, indique le blocker exact. \
                 Après une insertion acceptée, committe puis livre."
            ),
            "Premier appel : utilise l'unique `read_file` déclaré avec ses arguments déjà \
             contraints. Au tour suivant, appelle immédiatement l'unique `insert_after_line` \
             déclaré avec le `content_sha256` frais reçu et uniquement le texte à ajouter."
                .to_string(),
            format!(
                "## Cible mécanique prélocalisée\n- Fichier : `{path}`\n- Insertion après \
                 la ligne d'ancre : `{anchor_line}`\n- La cible est persistée, la ligne d'ancre \
                 est conservée mécaniquement et le hash vient de la lecture réelle du worktree.\n\n"
            ),
        ),
        None if can_run_shell => (
            format!(
                "Utilise les outils natifs de ton CLI (recherche, lecture ciblée, édition et shell). \
                 Cherche d'abord le symbole cité dans l'objectif, lis seulement la région utile, \
                 puis édite dès que tu sais quoi changer. Exécute les validations pertinentes. {}",
                if mediated_host_commit {
                    "Ne lance pas `git commit` dans le shell : appelle `task_exec_commit` avec \
                     uniquement les fichiers explicites et le message. Kronn possède seul \
                     l'accès Git administratif."
                } else {
                    "Crée un commit propre avant la livraison."
                }
            ),
            "Cherche le symbole cité dans l'objectif avec les outils natifs de ton CLI, \
             lis uniquement la région trouvée, puis implémente."
                .to_string(),
            String::new(),
        ),
        None => (
            "Le chemin le plus court, quelle que soit la taille du fichier : \
             `search_text` pour trouver l'endroit exact (fichier + ligne), \
             `read_file` avec `offset` et `limit` pour lire cette seule région, \
             `edit_lines` avec `start_line`, `end_line` et le `content_sha256` reçu \
             pour remplacer cette plage sans recopier une ancre, puis `git_commit`, \
             puis livre. Si le hash a dérivé, relis : Kronn refuse la mutation. \
             Dès que tu sais quoi changer, fais l'édition : relire une région déjà \
             lue ne produit rien de neuf."
                .to_string(),
            "Premier appel : `search_text` sur le symbole cité dans l'objectif. \
             Puis lis UNIQUEMENT la région trouvée, et fais l'édition avec \
             `edit_lines` dès que tu sais quoi changer — lire le fichier en continu, \
             tranche après tranche, consomme ton tour sans rien produire."
                .to_string(),
            String::new(),
        ),
    };
    let delivery_format = if !native_delivery_projection {
        format!(
            "Champs requis :\n\
             - `version` : `\"1\"`\n\
             - `task_ref` : `{task_reference}`\n\
             - `head_sha` : le HEAD exact livré (SHA du dernier commit de la branche)\n\
             - `files_touched` : liste de `{{ path, kind: added|modified|deleted }}`\n\
             - `tests` : liste de `{{ name, status: pass|fail|skipped, evidence }}`\n\
             - `dod_status` : un `{{ dod_id, met, evidence }}` par item de la DoD\n\
             - `docs`, `migrations`, `risks`, `limitations` : listes (mets `[]` explicitement si sans objet)\n\
             - `summary` : résumé court des changements"
        )
    } else {
        "Tu fournis uniquement les assertions sémantiques suivantes :\n\
         - `tests` : liste de `{ name, status: pass|fail|skipped, evidence }`\n\
         - `dod_status` : exactement un `{ met, evidence }` par item de la DoD, dans l'ordre ci-dessus\n\
         - `docs`, `migrations`, `risks`, `limitations` : listes (mets `[]` explicitement si sans objet)\n\
         - `summary` : résumé court des changements\n\
         N'invente et ne recopie aucun UUID, SHA ou inventaire : après autorisation, Kronn injecte lui-même `version`, `task_ref`, `head_sha`, `files_touched` et chaque `dod_id` depuis la tâche et le worktree exact."
            .to_string()
    };
    let commit_boundary = if mediated_host_commit {
        "Avant la livraison, appelle `task_exec_commit` avec les seuls chemins relatifs réellement \
         modifiés et un message concis. N'utilise pas `git commit` dans le shell : les objets et \
         refs partagés restent volontairement hors de ta sandbox."
    } else if can_run_shell {
        "Crée un commit propre dans ce worktree avant la livraison."
    } else {
        "Avant la livraison, appelle `git_commit` avec les seuls chemins relatifs réellement \
         modifiés et un message concis."
    };
    format!(
        "# {reference} — {title}\n\n\
         ## Objectif\n{objective}\n\n\
         ## Definition of Done\n{dod}\n\n\
         ## Décisions & périmètre\n\
         Le périmètre exact de cette tâche EST la Definition of Done ci-dessus : \
         ne touche que ce qui la satisfait, ne déborde ni sur d'autres tâches ni \
         sur des fichiers non liés. Les décisions de conception applicables sont \
         celles déjà consignées dans l'objectif et les documents de conception \
         liés — respecte-les, ne les rejoue pas.\n\n\
         ## Méthode\n\
         {method}\n\n\
         {mechanical_scope}\
         ## Contraintes\n\
         - Travaille UNIQUEMENT dans ce worktree ; ne touche jamais un autre checkout.\n\
         - Pas de `git push`, pas de force-push, pas de merge : l'intégration \
         protégée est faite par Kronn APRÈS revue.\n\
         - Reste dans le périmètre de la DoD.\n\n\
         ## Tests\n\
         {tests}\n\n\
         ## Workspace\n\
         - Branche : `{branch}`\n\
         - Worktree : `{worktree}`\n\
         - Base épinglée : `{base}`\n\n\
         ## Format de livraison\n\
         {commit_boundary}\n\
         Quand la DoD est satisfaite, soumets un **DeliveryManifest v1** via \
         l'outil `task_exec_deliver` (le corps est validé ; un manifeste non \
         conforme est refusé, pas silencieusement accepté).\n\
         {delivery_format}\n\
         Ne signale « prêt à la revue » qu'APRÈS cette soumission : c'est elle \
         qui réveille l'agent principal pour la revue.\n\n\
         ## Commence maintenant\n\
         Ce worktree est neuf : il n'y a AUCUN état à inventorier, c'est établi. \
         {first_action}",
        reference = task_reference,
        title = task_title,
        objective = objective,
        dod = dod,
        tests = tests,
        method = method,
        mechanical_scope = mechanical_scope,
        delivery_format = delivery_format,
        commit_boundary = commit_boundary,
        first_action = first_action,
        branch = branch,
        worktree = worktree_path,
        base = base_sha,
    )
}

/// An orchestrator-authored discussion message (brief, control offer, attach notice):
/// a backend-authored `User`/`Main` message with the `Orchestrateur` pseudonym. The
/// caller sets the deterministic id so a resume can never double-post.
pub(crate) fn orchestrator_message(id: String, content: String) -> DiscussionMessage {
    DiscussionMessage {
        id,
        role: MessageRole::User,
        channel: MessageChannel::Main,
        content,
        agent_type: None,
        timestamp: chrono::Utc::now(),
        tokens_used: 0,
        session_tokens_at_message: None,
        recovered_partial: false,
        auth_mode: None,
        model_tier: None,
        model: None,
        cost_usd: None,
        author_pseudo: Some("Orchestrateur".to_string()),
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        lint_report: None,
        target_agent: None,
        reply_to_message_id: None,
        author_cli_ordinal: None,
    }
}

/// The CLI control-offer message posted in the ORIGIN room (NOT the work brief). Its
/// id is deterministic per `(execution, attempt)` so a resume can never double-post
/// (the PK rejects it), and it carries the opaque `offer_id` the exact session uses
/// to accept (KT-328 tranche 2).
fn build_control_offer_message(
    exec_id: &str,
    attempt_no: u32,
    offer_id: &str,
    task_reference: &str,
    task_title: &str,
    child_discussion_id: &str,
) -> DiscussionMessage {
    let content = format!(
        "**Offre de prise en charge — {reference} : {title}**\n\n\
         Une tâche t'est proposée comme worker. Pour l'accepter et être rattaché à \
         sa sous-discussion, appelle :\n\n\
         `task_exec_accept_worker_offer({{ offer_id: \"{offer}\" }})`\n\n\
         - Sous-discussion : `{child}`\n\
         - Ceci est une offre de contrôle, pas encore le brief de travail : le brief \
         n'arrive dans la sous-discussion qu'après ton acceptation.",
        reference = task_reference,
        title = task_title,
        offer = offer_id,
        child = child_discussion_id,
    );
    DiscussionMessage {
        id: format!("orch-offer:{exec_id}:{attempt_no}"),
        role: MessageRole::User,
        channel: MessageChannel::Main,
        content,
        agent_type: None,
        timestamp: chrono::Utc::now(),
        tokens_used: 0,
        session_tokens_at_message: None,
        recovered_partial: false,
        auth_mode: None,
        model_tier: None,
        model: None,
        cost_usd: None,
        author_pseudo: Some("Orchestrateur".to_string()),
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        lint_report: None,
        target_agent: None,
        reply_to_message_id: None,
        author_cli_ordinal: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP surface (KT-328 tranche 2, commit 3). Until now the provisioning saga and
// the CLI-worker accept path were reachable only from tests — no route, no MCP
// tool — so the control offer instructed a worker to call `task_exec_accept_worker_offer`,
// which resolved to nothing. These thin handlers expose the two primitives; the bridge
// MCP tool calls `accept-offer`. KT-321 now adds the campaign policy/read/launch
// endpoints below; KT-323 owns their full plan/discussion UX.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateCampaignRequest {
    pub discussion_id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub target_workspace_id: Option<String>,
    pub target_branch: String,
    #[serde(default)]
    pub integration_strategy: crate::models::IntegrationStrategy,
    #[serde(default = "default_campaign_review_rounds")]
    pub max_review_rounds: u32,
    #[serde(default = "default_campaign_concurrency")]
    pub max_concurrent_executions: u32,
    #[serde(default = "default_campaign_concurrency")]
    pub max_cli_concurrent_executions: u32,
    #[serde(default)]
    pub token_budget: Option<u64>,
    #[serde(default)]
    pub timeout_secs: Option<u32>,
    #[serde(default)]
    pub activity_timeout_secs: Option<u32>,
    #[serde(default)]
    pub review_timeout_secs: Option<u32>,
    #[serde(default)]
    pub human_wait_timeout_secs: Option<u32>,
    #[serde(default)]
    pub cancellation_cleanup_policy: Option<crate::models::CancellationCleanupPolicy>,
    #[serde(default)]
    pub validations: Vec<crate::models::ValidationSpec>,
    #[serde(default)]
    pub allowed_agents: Vec<crate::models::AgentType>,
    #[serde(default)]
    pub default_worker: Option<crate::models::CampaignWorkerSelection>,
    #[serde(default)]
    pub auto_continue: bool,
}

fn default_campaign_review_rounds() -> u32 {
    3
}

fn default_campaign_concurrency() -> u32 {
    1
}

fn preparation_reason(code: &str, detail: impl Into<String>) -> crate::models::CampaignTaskReason {
    crate::models::CampaignTaskReason {
        code: code.to_string(),
        detail: detail.into(),
    }
}

/// Refusals that depend only on the typed target. The catalogue and preflight
/// deliberately share this helper so an identity cannot be advertised with a
/// different transport/capability verdict than the launch boundary applies.
fn worker_static_refusal(worker: &MessageTarget) -> Option<crate::models::CampaignTaskReason> {
    if let Err(error) = crate::db::orchestration::ensure_task_worker_transport_compatible(worker) {
        return Some(preparation_reason("worker_transport", error.to_string()));
    }
    if matches!(
        worker.kind,
        MessageTargetKind::DiscussionAgent | MessageTargetKind::Agent
    ) && matches!(worker.agent_type, AgentType::Vibe | AgentType::Custom)
    {
        return Some(preparation_reason(
            "worker_capability",
            "this native runtime cannot acknowledge the typed delivery lifecycle; use an exact joined CLI session or another supported agent",
        ));
    }
    None
}

/// Validate the worker identity at the shared launch boundary. HTTP/MCP
/// preflight calls this for a useful refusal, but provisioning calls it again
/// before creating any execution or worktree: internal callers and campaigns
/// must not be able to bypass exact-session routing guarantees.
fn worker_launch_refusal(
    conn: &rusqlite::Connection,
    parent_discussion_id: &str,
    worker: &MessageTarget,
) -> Result<Option<crate::models::CampaignTaskReason>> {
    if let Some(reason) = worker_static_refusal(worker) {
        return Ok(Some(reason));
    }
    let refusal = match worker.kind {
        MessageTargetKind::Cli => match worker.cli_session_id {
            None => Some(preparation_reason(
                "worker_identity",
                "CLI worker requires an exact cli_session_id",
            )),
            Some(session_id) => {
                let expected_agent = crate::db::orchestration::agent_type_to_db(&worker.agent_type);
                let available: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM discussion_sessions \
                     WHERE id = ?1 AND disc_id = ?2 AND agent_type = ?3 \
                       AND status <> 'left')",
                    rusqlite::params![session_id, parent_discussion_id, expected_agent],
                    |row| row.get(0),
                )?;
                (!available).then(|| {
                    preparation_reason(
                        "worker_unavailable",
                        "selected CLI session is not active in the principal discussion for the requested provider",
                    )
                })
            }
        },
        MessageTargetKind::DiscussionAgent | MessageTargetKind::Agent => None,
    };
    Ok(refusal)
}

pub(crate) fn worker_scope_contract_refusal(
    intent: Option<TaskWorkerScopeIntent>,
    scope: Option<&TaskWorkerScope>,
) -> Option<crate::models::CampaignTaskReason> {
    match (intent, scope) {
        (None, _) => Some(preparation_reason(
            "worker_scope_intent_missing",
            "worker_scope_intent is required; the MCP host tool schema may be stale — reconnect the Kronn MCP before retrying",
        )),
        (Some(TaskWorkerScopeIntent::Generic), Some(_)) => Some(preparation_reason(
            "worker_scope_intent_mismatch",
            "worker_scope_intent=generic forbids worker_scope; use scoped or remove the scope",
        )),
        (Some(TaskWorkerScopeIntent::Scoped), None) => Some(preparation_reason(
            "worker_scope_intent_mismatch",
            "worker_scope_intent=scoped requires worker_scope",
        )),
        (Some(TaskWorkerScopeIntent::Generic), None)
        | (Some(TaskWorkerScopeIntent::Scoped), Some(_)) => None,
    }
}

pub(crate) fn worker_scope_refusal(
    worker: &MessageTarget,
    scope: Option<&TaskWorkerScope>,
) -> Option<crate::models::CampaignTaskReason> {
    let scope = scope?;
    if let Err(detail) = scope.validate() {
        return Some(preparation_reason("worker_scope", detail));
    }
    if worker.kind != MessageTargetKind::DiscussionAgent
        || !crate::agents::runner::is_http_chat_agent(&worker.agent_type)
    {
        return Some(preparation_reason(
            "worker_scope_transport",
            "worker_scope is supported only by native HTTP discussion_agent workers",
        ));
    }
    None
}

pub(crate) fn prepare_task_execution(
    conn: &rusqlite::Connection,
    task_reference: &str,
    parent_discussion_id: &str,
    worker: &MessageTarget,
) -> Result<crate::models::TaskExecutionPreparation> {
    let task =
        crate::db::planning::get_task(conn, task_reference)?.context("planning task not found")?;
    let parent = crate::db::discussions::get_discussion(conn, parent_discussion_id)?
        .context("principal discussion not found")?;
    let active_execution =
        crate::db::orchestration::get_active_execution_for_task(conn, &task.summary.id)?;
    let mut reasons = Vec::new();

    if !task
        .summary
        .discussion_ids
        .iter()
        .any(|id| id == parent_discussion_id)
    {
        reasons.push(preparation_reason(
            "task_not_in_discussion_plan",
            "task is not linked to the principal discussion plan",
        ));
    }
    if task.summary.status != PlanningTaskStatus::Todo {
        reasons.push(preparation_reason(
            "status",
            format!("task status is {:?}, not Todo", task.summary.status),
        ));
    }
    if task.definition_of_done.is_empty() {
        reasons.push(preparation_reason(
            "missing_definition_of_done",
            "task has no Definition of Done",
        ));
    }
    let active_blockers = task
        .blockers
        .iter()
        .filter(|blocker| {
            !matches!(
                blocker.status,
                PlanningTaskStatus::Done | PlanningTaskStatus::Archived
            )
        })
        .count();
    if active_blockers > 0 {
        reasons.push(preparation_reason(
            "active_blockers",
            format!("task has {active_blockers} active blocker(s)"),
        ));
    }
    let project_id = match task.summary.project_ids.as_slice() {
        [project_id] => {
            if parent.project_id.as_deref() != Some(project_id.as_str()) {
                reasons.push(preparation_reason(
                    "project_scope",
                    "task project does not match the principal discussion project",
                ));
            }
            if crate::db::projects::get_project(conn, project_id)?.is_none() {
                reasons.push(preparation_reason(
                    "project_missing",
                    "task project no longer exists",
                ));
            }
            Some(project_id.clone())
        }
        [] => {
            reasons.push(preparation_reason(
                "project_missing",
                "task has no project, so no repository can be resolved",
            ));
            None
        }
        _ => {
            reasons.push(preparation_reason(
                "project_ambiguous",
                "task belongs to several projects, so the repository is ambiguous",
            ));
            None
        }
    };
    if active_execution.is_some() {
        reasons.push(preparation_reason(
            "already_running",
            "task already has an active execution; inspect or resume it",
        ));
    }
    if let Some(reason) = worker_launch_refusal(conn, parent_discussion_id, worker)? {
        reasons.push(reason);
    }

    Ok(crate::models::TaskExecutionPreparation {
        task,
        parent_discussion_id: parent_discussion_id.to_string(),
        worker: worker.clone(),
        project_id,
        launchable: reasons.is_empty(),
        reasons,
        active_execution,
    })
}

#[derive(Serialize)]
pub struct CampaignView {
    pub run: crate::models::OrchestrationRun,
    pub candidates: Vec<crate::models::CampaignTaskCandidate>,
    pub principal_attention: crate::models::PrincipalAttention,
}

#[derive(Deserialize)]
pub struct CampaignLaunchRequest {
    #[serde(default)]
    pub task_reference: Option<String>,
    #[serde(default)]
    pub worker_override: Option<crate::models::CampaignWorkerSelection>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Serialize)]
pub struct CampaignLaunchResponse {
    pub execution: TaskExecution,
    pub worker_selection_reason: String,
}

#[derive(Deserialize)]
pub struct CampaignControlRequest {
    pub state: crate::models::OrchestrationControlState,
    #[serde(default)]
    pub reason: Option<String>,
}

fn campaign_view(conn: &rusqlite::Connection, run_id: &str) -> Result<CampaignView> {
    let run = crate::db::orchestration::get_orchestration_run(conn, run_id)?
        .context("orchestration run not found")?;
    let candidates = crate::db::orchestration::campaign_task_candidates(conn, run_id, None)?;
    let principal_attention = crate::db::orchestration::principal_attention(conn, run_id)?;
    Ok(CampaignView {
        run,
        candidates,
        principal_attention,
    })
}

/// Create a durable campaign policy. The target branch is deliberately required:
/// a principal may choose it, but the integration engine never guesses it.
pub async fn create_campaign(
    State(state): State<AppState>,
    Json(request): Json<CreateCampaignRequest>,
) -> Json<ApiResponse<CampaignView>> {
    let target_branch = request.target_branch.trim().to_string();
    if target_branch.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "target_branch is required".to_string(),
        ));
    }
    let resilience = crate::models::OrchestrationResiliencePolicy {
        activity_timeout_secs: request.activity_timeout_secs,
        review_timeout_secs: request.review_timeout_secs,
        human_wait_timeout_secs: request.human_wait_timeout_secs,
        cancellation_cleanup_policy: request
            .cancellation_cleanup_policy
            .unwrap_or(crate::models::CancellationCleanupPolicy::Preserve),
    };
    let result = state
        .db
        .with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            if crate::db::discussions::get_discussion(&tx, &request.discussion_id)?.is_none() {
                bail!("principal discussion not found");
            }
            let input = crate::models::OrchestrationRunInput {
                kind: crate::models::OrchestrationRunKind::Campaign,
                discussion_id: request.discussion_id,
                project_id: request.project_id,
                target_workspace_id: request.target_workspace_id,
                target_branch: Some(target_branch),
                max_review_rounds: request.max_review_rounds,
                max_concurrent_executions: request.max_concurrent_executions,
                token_budget: request.token_budget,
                integration_strategy: request.integration_strategy,
                validations: request.validations,
                escalation_notify_url: None,
                timeout_secs: request.timeout_secs,
                max_cli_concurrent_executions: request.max_cli_concurrent_executions,
                allowed_agents: request.allowed_agents,
                default_worker: request.default_worker,
                auto_continue: request.auto_continue,
            };
            let run = crate::db::orchestration::create_orchestration_run(&tx, &input)?;
            crate::db::orchestration::set_resilience_policy(&tx, &run.id, &resilience)?;
            let view = campaign_view(&tx, &run.id)?;
            tx.commit()?;
            Ok(view)
        })
        .await;
    match result {
        Ok(view) => Json(ApiResponse::ok(view)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            error.to_string(),
        )),
    }
}

pub async fn get_campaign(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Json<ApiResponse<CampaignView>> {
    match state
        .db
        .with_conn(move |conn| campaign_view(conn, &run_id))
        .await
    {
        Ok(view) => Json(ApiResponse::ok(view)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            error.to_string(),
        )),
    }
}

pub async fn get_discussion_campaign(
    State(state): State<AppState>,
    Path(discussion_id): Path<String>,
) -> Json<ApiResponse<Option<CampaignView>>> {
    let result = state
        .db
        .with_conn(move |conn| {
            let Some(run) =
                crate::db::orchestration::get_active_campaign_for_discussion(conn, &discussion_id)?
            else {
                return Ok(None);
            };
            campaign_view(conn, &run.id).map(Some)
        })
        .await;
    match result {
        Ok(view) => Json(ApiResponse::ok(view)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            error.to_string(),
        )),
    }
}

/// Select the first policy-compliant plan task, or validate an explicit task,
/// then reuse the exact KT-318 provisioning saga.
pub async fn launch_campaign_task(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(request): Json<CampaignLaunchRequest>,
) -> Json<ApiResponse<CampaignLaunchResponse>> {
    let chosen = {
        let run_id = run_id.clone();
        let worker = request.worker_override.clone();
        let requested = request.task_reference.clone();
        state
            .db
            .with_conn(move |conn| {
                let candidates = crate::db::orchestration::campaign_task_candidates(
                    conn,
                    &run_id,
                    worker.as_ref(),
                )?;
                match requested.as_deref() {
                    Some(reference) => candidates
                        .into_iter()
                        .find(|candidate| {
                            candidate.task.reference == reference || candidate.task.id == reference
                        })
                        .filter(|candidate| candidate.launchable)
                        .map(|candidate| candidate.task.reference)
                        .context("requested task is not launchable under the campaign policy"),
                    None => candidates
                        .into_iter()
                        .find(|candidate| candidate.launchable)
                        .map(|candidate| candidate.task.reference)
                        .context("campaign has no launchable task"),
                }
            })
            .await
    };
    let task_reference = match chosen {
        Ok(reference) => reference,
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                error.to_string(),
            ))
        }
    };
    match provision_campaign_task_execution(
        &state.db,
        CampaignProvisionInput {
            orchestration_run_id: run_id,
            task_reference,
            worker_override: request.worker_override,
            idempotency_key: request.idempotency_key,
        },
    )
    .await
    {
        Ok((execution, worker_selection_reason)) => Json(ApiResponse::ok(CampaignLaunchResponse {
            execution,
            worker_selection_reason,
        })),
        Err(error) => {
            let (code, message) = provision_error_parts(&error);
            Json(ApiResponse::err_coded(code, message))
        }
    }
}

pub async fn control_campaign(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(request): Json<CampaignControlRequest>,
) -> Json<ApiResponse<CampaignView>> {
    let result = state
        .db
        .with_conn(move |conn| {
            crate::db::orchestration::set_orchestration_control_state(
                conn,
                &run_id,
                request.state,
                request.reason.as_deref(),
                &backend_actor(),
            )?;
            campaign_view(conn, &run_id)
        })
        .await;
    match result {
        Ok(view) => Json(ApiResponse::ok(view)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            error.to_string(),
        )),
    }
}

#[derive(Serialize)]
pub struct ExecutionRecoveryView {
    pub execution: TaskExecution,
    pub recovery: Option<crate::models::TaskExecutionRecovery>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancellation_signal_sent: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub termination_confirmed: Option<bool>,
}

const MAX_RECENT_HTTP_TURNS: usize = 128;

fn summarize_http_turn_usage(
    events: &[crate::models::TaskExecutionEvent],
) -> Option<crate::models::TaskExecutionHttpUsage> {
    use crate::models::{TaskExecutionHttpPhaseUsage, TaskExecutionHttpTurnUsage};
    let mut recent_turns = Vec::<TaskExecutionHttpTurnUsage>::new();
    let mut phases = std::collections::BTreeMap::<
        crate::models::TaskExecutionHttpPhase,
        TaskExecutionHttpPhaseUsage,
    >::new();
    let mut turns = 0u32;
    let mut prompt_tokens = 0u64;
    let mut eval_tokens = 0u64;
    let mut duration_ms = 0u64;
    let mut peak_context_tokens = 0u64;

    for event in events
        .iter()
        .filter(|event| event.action == "http_turn_telemetry")
    {
        if event
            .changes
            .get("version")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
        {
            continue;
        }
        let Ok(mut batch) = serde_json::from_value::<Vec<TaskExecutionHttpTurnUsage>>(
            event.changes.get("turns").cloned().unwrap_or_default(),
        ) else {
            continue;
        };
        for turn in &mut batch {
            turn.dispatch_id.clone_from(&event.actor_session_id);
            turns = turns.saturating_add(1);
            prompt_tokens = prompt_tokens.saturating_add(turn.prompt_tokens);
            eval_tokens = eval_tokens.saturating_add(turn.eval_tokens);
            duration_ms = duration_ms.saturating_add(turn.duration_ms);
            peak_context_tokens = peak_context_tokens.max(turn.prompt_tokens);
            let phase = phases
                .entry(turn.phase)
                .or_insert(TaskExecutionHttpPhaseUsage {
                    phase: turn.phase,
                    turns: 0,
                    prompt_tokens: 0,
                    eval_tokens: 0,
                    duration_ms: 0,
                });
            phase.turns = phase.turns.saturating_add(1);
            phase.prompt_tokens = phase.prompt_tokens.saturating_add(turn.prompt_tokens);
            phase.eval_tokens = phase.eval_tokens.saturating_add(turn.eval_tokens);
            phase.duration_ms = phase.duration_ms.saturating_add(turn.duration_ms);
        }
        recent_turns.extend(batch);
    }
    if turns == 0 {
        return None;
    }
    if recent_turns.len() > MAX_RECENT_HTTP_TURNS {
        recent_turns.drain(..recent_turns.len() - MAX_RECENT_HTTP_TURNS);
    }
    Some(crate::models::TaskExecutionHttpUsage {
        turns,
        prompt_tokens,
        eval_tokens,
        traffic_tokens: prompt_tokens.saturating_add(eval_tokens),
        peak_context_tokens,
        duration_ms,
        phases: phases.into_values().collect(),
        recent_turns,
    })
}

pub(crate) fn execution_detail(
    conn: &rusqlite::Connection,
    exec_id: &str,
) -> Result<crate::models::TaskExecutionDetail> {
    let lineage = crate::db::orchestration::get_execution_lineage(conn, exec_id)?
        .context("task execution not found")?;
    let run = crate::db::orchestration::get_orchestration_run(
        conn,
        &lineage.execution.orchestration_run_id,
    )?
    .context("orchestration run not found")?;
    let task = crate::db::planning::get_task(conn, &lineage.execution.task_id)?
        .context("planning task not found")?;

    let mut attempts = std::collections::BTreeMap::new();
    for attempt_no in 0..=lineage.execution.attempt_no {
        attempts.insert(
            attempt_no,
            crate::models::TaskExecutionAttemptDetail {
                attempt_no,
                delivery: None,
                review: None,
            },
        );
    }
    for persisted in crate::db::worker_deliveries::list_deliveries(conn, exec_id)? {
        let attempt = attempts.entry(persisted.attempt_no).or_insert_with(|| {
            crate::models::TaskExecutionAttemptDetail {
                attempt_no: persisted.attempt_no,
                delivery: None,
                review: None,
            }
        });
        attempt.delivery = Some(
            serde_json::from_str(&persisted.manifest_json)
                .context("persisted delivery manifest is invalid")?,
        );
    }
    for persisted in crate::db::worker_reviews::list_reviews(conn, exec_id)? {
        let attempt = attempts.entry(persisted.attempt_no).or_insert_with(|| {
            crate::models::TaskExecutionAttemptDetail {
                attempt_no: persisted.attempt_no,
                delivery: None,
                review: None,
            }
        });
        attempt.review = Some(
            serde_json::from_str(&persisted.decision_json)
                .context("persisted review decision is invalid")?,
        );
    }

    let (tokens, in_app_cost_usd, in_app_cost_is_partial) =
        if let Some(child_discussion_id) = lineage.sub_discussion_id.as_deref() {
            let tokens = crate::db::cli_telemetry::cost_for_discussion(conn, child_discussion_id)?;
            let (agent_messages, priced_messages, known_cost): (i64, i64, Option<f64>) = conn
                .query_row(
                    "SELECT COUNT(*), COUNT(cost_usd), SUM(cost_usd) \
                       FROM messages \
                      WHERE discussion_id = ?1 AND role = 'Agent' AND channel = 'main'",
                    [child_discussion_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
            (
                tokens,
                known_cost,
                agent_messages > 0 && priced_messages < agent_messages,
            )
        } else {
            (
                crate::db::cli_telemetry::DiscussionTokenCost {
                    disc_id: String::new(),
                    in_app_tokens: 0,
                    in_app_messages: 0,
                    cli_traffic_tokens: None,
                    cli_billable_tokens: None,
                    cli_sessions: 0,
                    cli_sessions_measured: 0,
                    cli_sessions_unmeasured: 0,
                },
                None,
                false,
            )
        };
    let end = lineage
        .execution
        .finished_at
        .unwrap_or_else(chrono::Utc::now);
    let duration_ms = (end - lineage.execution.created_at)
        .num_milliseconds()
        .max(0);

    let recovery = crate::db::orchestration::get_execution_recovery(conn, exec_id)?;
    let http = summarize_http_turn_usage(&crate::db::orchestration::list_execution_events(
        conn, exec_id,
    )?);
    Ok(crate::models::TaskExecutionDetail {
        lineage,
        target_branch: run.target_branch,
        definition_of_done: task.definition_of_done,
        attempts: attempts.into_values().collect(),
        validation_runs: crate::db::orchestration::list_validation_runs(conn, exec_id)?,
        recovery,
        usage: crate::models::TaskExecutionUsage {
            duration_ms,
            in_app_tokens: tokens.in_app_tokens,
            in_app_messages: tokens.in_app_messages,
            in_app_cost_usd,
            in_app_cost_is_partial,
            cli_traffic_tokens: tokens.cli_traffic_tokens,
            cli_billable_tokens: tokens.cli_billable_tokens,
            cli_sessions: tokens.cli_sessions,
            cli_sessions_measured: tokens.cli_sessions_measured,
            cli_sessions_unmeasured: tokens.cli_sessions_unmeasured,
            http,
        },
    })
}

pub async fn get_execution_detail(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
) -> Json<ApiResponse<crate::models::TaskExecutionDetail>> {
    match state
        .db
        .with_conn(move |conn| execution_detail(conn, &exec_id))
        .await
    {
        Ok(view) => Json(ApiResponse::ok(view)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            error.to_string(),
        )),
    }
}

fn execution_state_index(status: TaskExecutionStatus) -> usize {
    use TaskExecutionStatus::*;
    match status {
        Pending => 0,
        Provisioning => 1,
        Blocked => 2,
        Working => 3,
        AwaitingReview => 4,
        Approved => 5,
        ChangesRequested => 6,
        Integrating => 7,
        Validating => 8,
        Applying => 9,
        Escalated => 10,
        Interrupted => 11,
        Done => 12,
        Failed => 13,
        Cancelled => 14,
    }
}

fn execution_metrics(
    execution: &TaskExecution,
    events: &[crate::models::TaskExecutionEvent],
    validations: &[crate::models::TaskExecutionValidationRun],
    usage: crate::models::TaskExecutionUsage,
    observed_at: chrono::DateTime<chrono::Utc>,
) -> crate::models::TaskExecutionMetrics {
    use TaskExecutionStatus::*;

    const STATES: [TaskExecutionStatus; 15] = [
        Pending,
        Provisioning,
        Blocked,
        Working,
        AwaitingReview,
        Approved,
        ChangesRequested,
        Integrating,
        Validating,
        Applying,
        Escalated,
        Interrupted,
        Done,
        Failed,
        Cancelled,
    ];

    let end = execution.finished_at.unwrap_or(observed_at);
    let mut durations = [0_i64; 15];
    let mut current = Pending;
    let mut cursor = execution.created_at;
    let mut ordered = events.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|event| event.created_at);
    for event in ordered {
        let Some(next) = event.to_status else {
            continue;
        };
        if next == current {
            continue;
        }
        let at = event.created_at.max(cursor).min(end);
        durations[execution_state_index(current)] += (at - cursor).num_milliseconds().max(0);
        current = next;
        cursor = at;
    }
    durations[execution_state_index(current)] += (end - cursor).num_milliseconds().max(0);

    let state_durations = STATES
        .into_iter()
        .enumerate()
        .filter(|(_, status)| durations[execution_state_index(*status)] > 0)
        .map(
            |(index, status)| crate::models::TaskExecutionStateDuration {
                status,
                duration_ms: durations[index],
            },
        )
        .collect::<Vec<_>>();
    let waiting_duration_ms = [Pending, Blocked, AwaitingReview, Escalated, Interrupted]
        .into_iter()
        .map(|status| durations[execution_state_index(status)])
        .sum();

    let validation_failures = validations.iter().filter(|run| !run.passed()).count() as u32;
    let escalations = events
        .iter()
        .filter(|event| event.to_status == Some(Escalated))
        .count() as u32;
    let mut failures = Vec::new();
    if validation_failures > 0 {
        failures.push(crate::models::TaskExecutionMetricCount {
            code: "validation_failed".into(),
            count: validation_failures,
        });
    }
    if execution.status == Failed {
        failures.push(crate::models::TaskExecutionMetricCount {
            code: "execution_failed".into(),
            count: 1,
        });
    }
    if escalations > 0 {
        failures.push(crate::models::TaskExecutionMetricCount {
            code: "escalated".into(),
            count: escalations,
        });
    }

    let mut blocked_counts = std::collections::BTreeMap::<String, u32>::new();
    for event in events
        .iter()
        .filter(|event| event.to_status == Some(Blocked))
    {
        let code = event
            .changes
            .get("code")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| value.parse::<crate::models::BlockedReasonCode>().ok())
            .map(|value| value.as_str())
            .unwrap_or("unspecified");
        *blocked_counts.entry(code.to_string()).or_default() += 1;
    }
    for event in events {
        let Some(kind) = event
            .changes
            .get("timeout_kind")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let code = match kind {
            "activity" => "timeout_activity",
            "review" => "timeout_review",
            "total" => "timeout_total",
            "human_wait" => "timeout_human_wait",
            _ => "timeout_unknown",
        };
        *blocked_counts.entry(code.to_string()).or_default() += 1;
    }
    let blocking_reasons = blocked_counts
        .into_iter()
        .map(|(code, count)| crate::models::TaskExecutionMetricCount { code, count })
        .collect();

    crate::models::TaskExecutionMetrics {
        state_durations,
        waiting_duration_ms,
        review_rounds: execution.review_rounds,
        attempt_count: execution.attempt_no.saturating_add(1),
        validation_failures,
        failures,
        blocking_reasons,
        usage,
    }
}

pub(crate) fn execution_observability(
    conn: &rusqlite::Connection,
    exec_id: &str,
) -> Result<crate::models::TaskExecutionObservability> {
    let detail = execution_detail(conn, exec_id)?;
    let events = crate::db::orchestration::list_execution_events(conn, exec_id)?;
    let metrics = execution_metrics(
        &detail.lineage.execution,
        &events,
        &detail.validation_runs,
        detail.usage,
        chrono::Utc::now(),
    );
    let audit_events = events
        .into_iter()
        .map(|event| crate::models::TaskExecutionAuditEvent {
            id: event.id,
            action: event.action,
            from_status: event.from_status,
            to_status: event.to_status,
            actor_kind: event.actor_kind,
            actor_id: event.actor_id,
            actor_session_id: event.actor_session_id,
            source_message_id: event.source_message_id,
            created_at: event.created_at,
        })
        .collect();
    Ok(crate::models::TaskExecutionObservability {
        lineage: detail.lineage,
        metrics,
        audit_events,
    })
}

pub async fn get_execution_observability(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
) -> Json<ApiResponse<crate::models::TaskExecutionObservability>> {
    match state
        .db
        .with_conn(move |conn| execution_observability(conn, &exec_id))
        .await
    {
        Ok(view) => Json(ApiResponse::ok(view)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            error.to_string(),
        )),
    }
}

pub async fn list_execution_discussion_links(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<crate::models::ExecutionDiscussionLink>>> {
    match state
        .db
        .with_conn(crate::db::orchestration::list_execution_discussion_links)
        .await
    {
        Ok(links) => Json(ApiResponse::ok(links)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            error.to_string(),
        )),
    }
}

#[derive(Deserialize)]
pub struct CancelExecutionRequest {
    #[serde(default = "default_cancel_reason")]
    pub reason: String,
    #[serde(default)]
    pub cleanup_policy: Option<crate::models::CancellationCleanupPolicy>,
}

fn default_cancel_reason() -> String {
    "cancelled by the principal".into()
}

#[derive(Deserialize)]
pub struct ReassignExecutionRequest {
    pub worker: crate::models::CampaignWorkerSelection,
    pub reason: String,
}

fn recovery_view(conn: &rusqlite::Connection, exec_id: &str) -> Result<ExecutionRecoveryView> {
    let execution = crate::db::orchestration::get_task_execution(conn, exec_id)?
        .context("task execution not found")?;
    let recovery = crate::db::orchestration::get_execution_recovery(conn, exec_id)?;
    Ok(ExecutionRecoveryView {
        execution,
        recovery,
        outcome: None,
        cancellation_signal_sent: None,
        termination_confirmed: None,
    })
}

pub async fn get_execution_recovery(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
) -> Json<ApiResponse<ExecutionRecoveryView>> {
    match state
        .db
        .with_conn(move |conn| recovery_view(conn, &exec_id))
        .await
    {
        Ok(view) => Json(ApiResponse::ok(view)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            error.to_string(),
        )),
    }
}

/// Retry the backend-owned apply gate after a dirty parent was cleaned.
///
/// This is deliberately narrower than boot recovery: only an Applying-origin
/// block may enter, the real target branch and cleanliness are re-checked, and
/// the state-machine checkpoint is cleared through `Blocked -> Applying` before
/// any Git operation. No caller can use this path to skip provisioning/review.
async fn resume_blocked_apply(
    db: &Database,
    exec_id: &str,
) -> Result<IntegrationOutcome, ProvisionError> {
    let internal = |error: String| ProvisionError::Internal(error);
    let (execution, run, workspace, project_path) = {
        let id = exec_id.to_string();
        db.with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &id)?
                .context("execution not found")?;
            if execution.status != TaskExecutionStatus::Blocked
                || execution.blocked_from_status != Some(TaskExecutionStatus::Applying)
            {
                bail!("execution is not an Applying-origin Blocked execution");
            }
            let run = crate::db::orchestration::get_orchestration_run(
                conn,
                &execution.orchestration_run_id,
            )?
            .context("orchestration run not found")?;
            let workspace =
                crate::db::discussion_workspaces::get_managed_for_execution(conn, &execution.id)?
                    .context("managed worktree not found")?;
            let project_id = run.project_id.as_deref().context("run has no project")?;
            let project_path = crate::db::projects::get_project(conn, project_id)?
                .context("project not found")?
                .path;
            Ok((execution, run, workspace, project_path))
        })
        .await
        .map_err(|error| internal(error.to_string()))?
    };
    workspace
        .canonical_path
        .as_deref()
        .ok_or_else(|| internal("managed worktree has no canonical path".into()))?;
    let repo = scanner::resolve_host_path(&project_path);
    let target = run
        .target_branch
        .as_deref()
        .ok_or_else(|| ProvisionError::CheckpointRefused("run has no target branch".into()))?;
    let target =
        worktree::resolve_local_branch(&repo, target).map_err(ProvisionError::CheckpointRefused)?;
    let dirty = worktree::worktree_dirty_files(&repo)
        .map_err(|error| ProvisionError::CheckpointRefused(error.to_string()))?;
    if !dirty.is_empty() {
        return Err(ProvisionError::CheckpointRefused(format!(
            "target still has {} uncommitted file(s)",
            dirty.len()
        )));
    }
    let tip =
        worktree::resolve_commit(&repo, &target).map_err(ProvisionError::CheckpointRefused)?;
    let action = match crate::models::saga_resume_action(
        TaskExecutionStatus::Applying,
        execution.candidate_target_sha.as_deref(),
        execution.candidate_merge_sha.as_deref(),
        execution.integrated_sha.as_deref(),
        Some(&tip),
        false,
    ) {
        SagaResumeAction::RebuildCandidate => ExecutionRecoveryAction::RebuildCandidate,
        SagaResumeAction::ApplyFastForward => ExecutionRecoveryAction::ApplyFastForward,
        SagaResumeAction::IdempotentClose => ExecutionRecoveryAction::IdempotentClose,
        other => {
            return Err(ProvisionError::CheckpointRefused(format!(
                "clean apply block has no safe resume action: {other:?}"
            )))
        }
    };
    // Direct apply/idempotent-close paths claim Blocked -> Applying inside
    // `finish_recovered_apply`. Rebuild first returns to Integrating; it needs
    // the origin checkpoint consumed here before rebuilding and later owns
    // Applying through the ordinary Armed checkpoint.
    if action == ExecutionRecoveryAction::RebuildCandidate {
        let id = exec_id.to_string();
        let resumed = db
            .with_conn(move |conn| {
                crate::db::orchestration::transition_execution(
                    conn,
                    &id,
                    TaskExecutionStatus::Applying,
                    &backend_actor(),
                    serde_json::json!({ "recovery": "dirty_target_cleared" }),
                )
            })
            .await
            .map_err(|error| internal(error.to_string()))?;
        if !resumed {
            return Err(ProvisionError::CheckpointRefused(
                "Applying-origin block was already resumed by another caller".into(),
            ));
        }
    }
    let outcome = resume_recovered_integration(db, exec_id, action).await?;
    if let IntegrationOutcome::Refused { reason } = &outcome {
        return Err(ProvisionError::CheckpointRefused(reason.clone()));
    }
    Ok(outcome)
}

pub async fn resume_execution(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
) -> Json<ApiResponse<ExecutionRecoveryView>> {
    let snapshot = {
        let id = exec_id.clone();
        state
            .db
            .with_conn(move |conn| recovery_view(conn, &id))
            .await
    };
    let Ok(snapshot) = snapshot else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "task execution or recovery decision not found",
        ));
    };
    if snapshot.execution.status == TaskExecutionStatus::Interrupted
        && snapshot.execution.interrupted_from_status == Some(TaskExecutionStatus::Blocked)
        && snapshot.execution.blocked_from_status == Some(TaskExecutionStatus::Applying)
    {
        let id = exec_id.clone();
        let restored = state
            .db
            .with_conn(move |conn| {
                crate::db::orchestration::transition_execution(
                    conn,
                    &id,
                    TaskExecutionStatus::Blocked,
                    &backend_actor(),
                    serde_json::json!({ "recovery": "restore_applying_block" }),
                )
            })
            .await;
        match restored {
            Ok(true) => {}
            Ok(false) => {
                return Json(ApiResponse::err_coded(
                    ApiErrorCode::Conflict,
                    "Applying-origin block was already resumed by another caller",
                ));
            }
            Err(error) => {
                return Json(ApiResponse::err_coded(
                    ApiErrorCode::Internal,
                    error.to_string(),
                ));
            }
        }
        let outcome = match resume_blocked_apply(&state.db, &exec_id).await {
            Ok(outcome) => format!("{outcome:?}"),
            Err(error) => {
                let (code, message) = provision_error_parts(&error);
                return Json(ApiResponse::err_coded(code, message));
            }
        };
        let id = exec_id.clone();
        return match state
            .db
            .with_conn(move |conn| {
                let mut view = recovery_view(conn, &id)?;
                view.outcome = Some(outcome);
                Ok(view)
            })
            .await
        {
            Ok(view) => Json(ApiResponse::ok(view)),
            Err(error) => Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                error.to_string(),
            )),
        };
    }
    if snapshot.execution.status == TaskExecutionStatus::Blocked
        && snapshot.execution.blocked_from_status == Some(TaskExecutionStatus::Applying)
    {
        let outcome = match resume_blocked_apply(&state.db, &exec_id).await {
            Ok(outcome) => format!("{outcome:?}"),
            Err(error) => {
                let (code, message) = provision_error_parts(&error);
                return Json(ApiResponse::err_coded(code, message));
            }
        };
        let id = exec_id.clone();
        return match state
            .db
            .with_conn(move |conn| {
                let mut view = recovery_view(conn, &id)?;
                view.outcome = Some(outcome);
                Ok(view)
            })
            .await
        {
            Ok(view) => Json(ApiResponse::ok(view)),
            Err(error) => Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                error.to_string(),
            )),
        };
    }
    if snapshot.execution.status == TaskExecutionStatus::Approved {
        let outcome = match run_integration(&state.db, &exec_id).await {
            Ok(IntegrationOutcome::Refused { reason }) => {
                return Json(ApiResponse::err_coded(ApiErrorCode::Conflict, reason));
            }
            Ok(outcome) => format!("{outcome:?}"),
            Err(error) => {
                let (code, message) = provision_error_parts(&error);
                return Json(ApiResponse::err_coded(code, message));
            }
        };
        let id = exec_id.clone();
        return match state
            .db
            .with_conn(move |conn| {
                let mut view = recovery_view(conn, &id)?;
                view.outcome = Some(outcome);
                Ok(view)
            })
            .await
        {
            Ok(view) => Json(ApiResponse::ok(view)),
            Err(error) => Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                error.to_string(),
            )),
        };
    }
    let Some(recovery) = snapshot.recovery else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            "execution has no pending recovery decision",
        ));
    };
    if !recovery.pending {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            "execution recovery decision was already consumed",
        ));
    }
    if snapshot.execution.status != TaskExecutionStatus::Interrupted {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            format!(
                "execution is {}, not Interrupted",
                snapshot.execution.status.as_str()
            ),
        ));
    }

    let action = recovery.recovery_action;
    let result: Result<String, ProvisionError> = match action {
        ExecutionRecoveryAction::ResumeProvisioning => {
            let target = match worker_target_from_execution(&snapshot.execution) {
                Ok(target) => target,
                Err(error) => {
                    return Json(ApiResponse::err_coded(
                        ApiErrorCode::Conflict,
                        error.to_string(),
                    ))
                }
            };
            let task_reference = {
                let task_id = snapshot.execution.task_id.clone();
                state
                    .db
                    .with_conn(move |conn| {
                        crate::db::planning::get_task(conn, &task_id)?
                            .map(|task| task.summary.reference)
                            .context("planning task vanished")
                    })
                    .await
            };
            match task_reference {
                Ok(task_reference) => resume_provisioning_execution(
                    &state.db,
                    ProvisionInput {
                        task_reference,
                        parent_discussion_id: snapshot.execution.parent_discussion_id.clone(),
                        worker: target,
                        base_rev: snapshot.execution.base_sha.clone(),
                        idempotency_key: snapshot.execution.idempotency_key.clone(),
                    },
                    &exec_id,
                )
                .await
                .map(|execution| format!("provisioning resumed as {}", execution.status.as_str())),
                Err(error) => Err(ProvisionError::Internal(error.to_string())),
            }
        }
        ExecutionRecoveryAction::ResumeWorker => wake_recovered_worker(&state.db, &exec_id)
            .await
            .map_err(|error| ProvisionError::Internal(error.to_string())),
        ExecutionRecoveryAction::AwaitReview => {
            let id = exec_id.clone();
            match state
                .db
                .with_conn(move |conn| {
                    crate::db::orchestration::transition_execution(
                        conn,
                        &id,
                        TaskExecutionStatus::AwaitingReview,
                        &backend_actor(),
                        serde_json::json!({ "recovery": action.as_str() }),
                    )?;
                    Ok(())
                })
                .await
            {
                Ok(()) => wake_recovered_principal(&state.db, &exec_id)
                    .await
                    .map_err(|error| ProvisionError::Internal(error.to_string())),
                Err(error) => Err(ProvisionError::Internal(error.to_string())),
            }
        }
        ExecutionRecoveryAction::AwaitHuman => {
            let id = exec_id.clone();
            state
                .db
                .with_conn(move |conn| {
                    crate::db::orchestration::transition_execution(
                        conn,
                        &id,
                        TaskExecutionStatus::Escalated,
                        &backend_actor(),
                        serde_json::json!({ "recovery": action.as_str() }),
                    )?;
                    Ok("restored human gate".to_string())
                })
                .await
                .map_err(|error| ProvisionError::Internal(error.to_string()))
        }
        ExecutionRecoveryAction::RebuildCandidate
        | ExecutionRecoveryAction::RunValidations
        | ExecutionRecoveryAction::ApplyFastForward
        | ExecutionRecoveryAction::IdempotentClose
        | ExecutionRecoveryAction::BlockDirtyTarget => {
            resume_recovered_integration(&state.db, &exec_id, action)
                .await
                .map(|outcome| format!("{outcome:?}"))
        }
        ExecutionRecoveryAction::BlockMissingWorkspace
        | ExecutionRecoveryAction::BlockMissingDiscussion
        | ExecutionRecoveryAction::BlockAgentUnavailable => {
            let id = exec_id.clone();
            let reason = recovery.recovery_reason.clone();
            state
                .db
                .with_conn(move |conn| {
                    crate::db::orchestration::transition_execution(
                        conn,
                        &id,
                        TaskExecutionStatus::Escalated,
                        &backend_actor(),
                        serde_json::json!({ "recovery": action.as_str(), "reason": reason }),
                    )?;
                    Ok("parked for human repair".to_string())
                })
                .await
                .map_err(|error| ProvisionError::Internal(error.to_string()))
        }
    };
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            let (code, message) = provision_error_parts(&error);
            return Json(ApiResponse::err_coded(code, message));
        }
    };
    let id = exec_id.clone();
    let applied = action.as_str().to_string();
    let refreshed = state
        .db
        .with_conn(move |conn| {
            crate::db::orchestration::clear_execution_recovery(conn, &id, &applied)?;
            let mut view = recovery_view(conn, &id)?;
            view.outcome = Some(outcome);
            Ok(view)
        })
        .await;
    match refreshed {
        Ok(view) => Json(ApiResponse::ok(view)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            error.to_string(),
        )),
    }
}

/// What happens to the managed worktree once its execution is cancelled.
///
/// Split from the handler so the KT-396 regression test can drive it against a
/// real repository and an in-memory database, without an `AppState`.
async fn settle_cancelled_workspace(
    db: &Database,
    exec_id: &str,
    cleanup: crate::models::CancellationCleanupPolicy,
    workspace: Option<crate::db::discussion_workspaces::DiscussionWorkspace>,
    project_path: Option<String>,
) -> String {
    let mut outcome = format!("cancelled; workspace policy={}", cleanup.as_str());
    let Some((workspace, project_path)) = workspace.zip(project_path) else {
        return outcome;
    };
    let Some(path) = workspace.canonical_path.as_deref() else {
        return outcome;
    };
    let repo = scanner::resolve_host_path(&project_path);
    let checkout = scanner::resolve_host_path(path);
    let checkout = checkout.to_string_lossy().to_string();
    // Whether the checkout is still on disk when this settles. Preserve keeps
    // it by policy; RemoveIfClean keeps it whenever the removal is refused.
    let mut survives = true;
    if cleanup == crate::models::CancellationCleanupPolicy::RemoveIfClean {
        match worktree::remove_cancelled_task_worktree(&repo, &checkout, &workspace.branch) {
            Ok(()) => {
                survives = false;
                let id = exec_id.to_string();
                let final_head = workspace.head_sha.clone();
                if let Err(error) = db
                    .with_conn(move |conn| {
                        crate::db::discussion_workspaces::retire_managed_for_execution(
                            conn,
                            &id,
                            final_head.as_deref(),
                        )?;
                        Ok(())
                    })
                    .await
                {
                    outcome.push_str(&format!(
                        "; checkout removed but DB cleanup failed: {error}"
                    ));
                } else {
                    outcome.push_str("; clean checkout removed, provenance preserved");
                }
            }
            Err(error) => outcome.push_str(&format!("; workspace preserved: {error}")),
        }
    }
    if survives {
        // KT-396 — Cancelled is a terminal state, yet this was the one terminal
        // path that never reclaimed the preserved worktree's `target/`: the
        // 39 MiB of task-kt-377-49a08eeb survived its cancellation intact. The
        // sources and the worktree stay, exactly as the policy promises; the
        // same guards as the integration-refusal site apply — liveness alone
        // decides, a refusal never fails the cancellation, every attempt is
        // audited. The DB's canonical_path goes through verbatim: liveness
        // looks the row up by that exact string.
        reclaim_preserved_worktree_artifacts(db, &repo, path).await;
    }
    outcome
}

const CANCELLATION_TERMINATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

async fn wait_for_cancelled_dispatches_to_settle(
    registry: &std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, tokio_util::sync::CancellationToken>>,
    >,
    signalled_keys: &[String],
    timeout: std::time::Duration,
) -> bool {
    if signalled_keys.is_empty() {
        return false;
    }
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let settled = registry
            .lock()
            .map(|entries| signalled_keys.iter().all(|key| !entries.contains_key(key)))
            .unwrap_or(false);
        if settled {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

pub async fn cancel_execution(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
    Json(request): Json<CancelExecutionRequest>,
) -> Json<ApiResponse<ExecutionRecoveryView>> {
    let snapshot = {
        let id = exec_id.clone();
        state
            .db
            .with_conn(move |conn| {
                let execution = crate::db::orchestration::get_task_execution(conn, &id)?
                    .context("task execution not found")?;
                let run = crate::db::orchestration::get_orchestration_run(
                    conn,
                    &execution.orchestration_run_id,
                )?
                .context("orchestration run not found")?;
                let policy = crate::db::orchestration::get_resilience_policy(conn, &run.id)?;
                let workspace =
                    crate::db::discussion_workspaces::get_managed_for_execution(conn, &id)?;
                let project_path = run
                    .project_id
                    .as_deref()
                    .map(|project_id| crate::db::projects::get_project(conn, project_id))
                    .transpose()?
                    .flatten()
                    .map(|project| project.path);
                Ok((execution, policy, workspace, project_path))
            })
            .await
    };
    let Ok((execution, policy, workspace, project_path)) = snapshot else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "task execution not found",
        ));
    };
    let exec_id_log = exec_id.clone();
    let signalled_keys = if let Ok(registry) = state.cancel_registry.lock() {
        let mut keys = Vec::new();
        for key in [
            execution.dispatch_job_id.as_deref(),
            execution.sub_discussion_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(token) = registry.get(key) {
                token.cancel();
                tracing::info!(
                    "Cancellation token signalled for execution {}: registry key {}",
                    exec_id_log,
                    key
                );
                if !keys.iter().any(|existing| existing == key) {
                    keys.push(key.to_string());
                }
            }
        }
        keys
    } else {
        Vec::new()
    };
    let tokens_cancelled = signalled_keys.len();
    let cancellation_signal_sent = tokens_cancelled > 0;
    tracing::info!(
        "Cancellation signal sent for execution {}: {} tokens in registry",
        exec_id_log,
        tokens_cancelled
    );
    let reason = request.reason.trim().to_string();
    let reason_log = reason.clone();
    let id = exec_id.clone();
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            crate::db::orchestration::cancel_execution_tree(conn, &id, &reason, &backend_actor())
        })
        .await
    {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            error.to_string(),
        ));
    }
    tracing::info!(
        "Execution {} marked as Cancelled in durable state (reason: {})",
        exec_id_log,
        reason_log
    );
    let termination_confirmed = wait_for_cancelled_dispatches_to_settle(
        &state.cancel_registry,
        &signalled_keys,
        CANCELLATION_TERMINATION_TIMEOUT,
    )
    .await;
    if termination_confirmed {
        tracing::info!(
            "Execution {} cancellation acknowledged: dispatch process terminated",
            exec_id_log
        );
    } else if cancellation_signal_sent {
        tracing::warn!(
            "Execution {} is durably Cancelled but process termination was not confirmed within {:?}",
            exec_id_log,
            CANCELLATION_TERMINATION_TIMEOUT
        );
    } else {
        tracing::info!(
            "Execution {} is durably Cancelled; no live dispatch token was available to signal",
            exec_id_log
        );
    }
    let cleanup = request
        .cleanup_policy
        .unwrap_or(policy.cancellation_cleanup_policy);
    let outcome =
        settle_cancelled_workspace(&state.db, &exec_id, cleanup, workspace, project_path).await;
    let id = exec_id.clone();
    match state
        .db
        .with_conn(move |conn| {
            let mut view = recovery_view(conn, &id)?;
            view.outcome = Some(outcome);
            view.cancellation_signal_sent = Some(cancellation_signal_sent);
            view.termination_confirmed = Some(termination_confirmed);
            Ok(view)
        })
        .await
    {
        Ok(view) => Json(ApiResponse::ok(view)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            error.to_string(),
        )),
    }
}

/// Reassign a native worker and queue its bounded handoff atomically. Keeping
/// the assignment generation, handoff message and replacement dispatch in one
/// transaction means a quota fallback cannot lose the execution between the
/// provider decision and the actual wake-up.
pub(crate) async fn reassign_native_execution(
    state: &AppState,
    exec_id: &str,
    selection: crate::models::CampaignWorkerSelection,
    reason: &str,
) -> Result<ExecutionRecoveryView> {
    if selection.target.kind == MessageTargetKind::Cli {
        bail!("native reassignment cannot target a CLI session");
    }
    crate::db::orchestration::ensure_task_worker_transport_compatible(&selection.target)?;
    let id = exec_id.to_string();
    let persisted_reason = reason.to_string();
    let (view, replaced_dispatch_id) = state
        .db
        .with_conn(move |conn| {
            let transaction = conn.unchecked_transaction()?;
            let replaced_dispatch_id =
                crate::db::orchestration::get_task_execution(&transaction, &id)?
                    .context("execution vanished before worker reassignment")?
                    .dispatch_job_id;
            let execution = crate::db::orchestration::reassign_execution_worker(
                &transaction,
                &id,
                &selection,
                &persisted_reason,
                &backend_actor(),
            )?;
            let child = execution
                .sub_discussion_id
                .clone()
                .context("execution has no child discussion for a bounded handoff")?;
            let agent = execution
                .worker_agent_type
                .as_deref()
                .context("reassigned execution has no provider")
                .and_then(crate::db::orchestration::agent_type_from_db)?;
            let tier = selection.target.tier.unwrap_or_default();
            anyhow::ensure!(
                crate::db::discussions::update_task_worker_assignment(
                    &transaction,
                    &child,
                    &agent,
                    &tier,
                    selection.model.as_deref(),
                    selection.profile_id.as_deref(),
                )?,
                "reassigned worker child discussion vanished"
            );
            let recovery = crate::db::orchestration::get_execution_recovery(&transaction, &id)?
                .context("reassignment recovery row vanished")?;
            let generation = recovery.assignment_generation;
            let mut handoff = handoff_notice_with_context(
                has_recorded_delivery(&transaction, &id)?,
                Some(generation),
                Some(&transaction),
                Some(&id),
            );
            if !persisted_reason.trim().is_empty() {
                handoff.push_str("\n\n## Consigne du principal pour cette réaffectation\n\n");
                handoff.push_str(persisted_reason.trim());
            }
            let message =
                orchestrator_message(format!("orch-reassign:{}:{}", id, generation), handoff);
            crate::db::discussions::insert_message(&transaction, &child, &message)?;
            let dispatch_id = Uuid::new_v4().to_string();
            let dedupe = format!("orch-reassign:{}:{}", id, recovery.assignment_generation);
            let job = crate::db::agent_dispatch::enqueue_for_latest_user(
                &transaction,
                crate::db::agent_dispatch::NewLatestUserDispatch {
                    id: &dispatch_id,
                    discussion_id: &child,
                    dedupe_key: &dedupe,
                    agent_override: Some(&agent),
                    chain_prompt_ids: &[],
                    batch_item: None,
                    group_id: None,
                    group_concurrency_limit: None,
                },
            )?;
            crate::db::orchestration::attach_execution_dispatch(&transaction, &id, &job.id)?;
            let current = crate::db::orchestration::get_task_execution(&transaction, &id)?
                .context("execution vanished during handoff")?;
            if matches!(
                current.status,
                TaskExecutionStatus::Interrupted
                    | TaskExecutionStatus::ChangesRequested
                    | TaskExecutionStatus::Escalated
            ) {
                crate::db::orchestration::transition_execution(
                    &transaction,
                    &id,
                    TaskExecutionStatus::Working,
                    &backend_actor(),
                    serde_json::json!({ "recovery": "worker_reassigned" }),
                )?;
            }
            crate::db::orchestration::clear_execution_recovery(
                &transaction,
                &id,
                "worker_reassigned",
            )?;
            transaction.commit()?;
            let mut view = recovery_view(conn, &id)?;
            view.outcome = Some(format!(
                "handoff generation {} queued in the existing child discussion",
                recovery.assignment_generation
            ));
            Ok((view, replaced_dispatch_id))
        })
        .await?;
    // The database CAS above makes every late action from the replaced
    // dispatch stale before we stop its process. Cancel only after commit: if
    // the transaction fails, the previous worker remains the durable owner.
    if let (Some(dispatch_id), Ok(registry)) = (replaced_dispatch_id, state.cancel_registry.lock())
    {
        if let Some(token) = registry.get(&dispatch_id) {
            token.cancel();
        }
    }
    state.agent_dispatch_notify.notify_waiters();
    Ok(view)
}

pub async fn reassign_execution(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
    Json(request): Json<ReassignExecutionRequest>,
) -> Json<ApiResponse<ExecutionRecoveryView>> {
    if request.worker.target.kind != MessageTargetKind::Cli {
        return match reassign_native_execution(&state, &exec_id, request.worker, &request.reason)
            .await
        {
            Ok(view) => Json(ApiResponse::ok(view)),
            Err(error) => Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                error.to_string(),
            )),
        };
    }
    let id = exec_id.clone();
    let selection = request.worker.clone();
    let reason = request.reason.trim().to_string();
    let reassigned = state
        .db
        .with_conn(move |conn| {
            crate::db::orchestration::reassign_execution_worker(
                conn,
                &id,
                &selection,
                &reason,
                &backend_actor(),
            )
        })
        .await;
    let Ok(execution) = reassigned else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            reassigned.err().unwrap().to_string(),
        ));
    };
    let Some(child) = execution.sub_discussion_id.clone() else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            "execution has no child discussion for a bounded handoff",
        ));
    };
    if execution.worker_target_kind == Some(MessageTargetKind::Cli) {
        let Some(session_id) = execution.worker_cli_session_id else {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                "CLI reassignment has no exact session identity",
            ));
        };
        let provider = match execution.worker_agent_type.as_deref() {
            Some(value) => match crate::db::orchestration::agent_type_from_db(value) {
                Ok(provider) => provider,
                Err(error) => {
                    return Json(ApiResponse::err_coded(
                        ApiErrorCode::Conflict,
                        error.to_string(),
                    ))
                }
            },
            None => {
                return Json(ApiResponse::err_coded(
                    ApiErrorCode::Conflict,
                    "CLI reassignment has no provider",
                ))
            }
        };
        let id = exec_id.clone();
        let origin = execution.parent_discussion_id.clone();
        let child_for_offer = child.clone();
        let opened = state
            .db
            .with_conn(move |conn| {
                let tx = conn.unchecked_transaction()?;
                let current = crate::db::orchestration::get_task_execution(&tx, &id)?
                    .context("execution vanished before CLI reassignment offer")?;
                if current.status != TaskExecutionStatus::Interrupted {
                    crate::db::orchestration::transition_execution(
                        &tx,
                        &id,
                        TaskExecutionStatus::Interrupted,
                        &backend_actor(),
                        serde_json::json!({ "reason": "cli_reassignment_handoff" }),
                    )?;
                }
                let recovery = crate::db::orchestration::get_execution_recovery(&tx, &id)?
                    .context("reassignment recovery row vanished")?;
                let offer = match crate::db::worker_offers::open_worker_offer(
                    &tx,
                    &crate::db::worker_offers::NewWorkerOffer {
                        id: None,
                        task_execution_id: &id,
                        attempt_no: current.attempt_no,
                        target_cli_session_id: session_id,
                        origin_discussion_id: &origin,
                        child_discussion_id: &child_for_offer,
                        expires_at: None,
                        offer_message_id: None,
                        reason: Some("cli_reassignment"),
                    },
                )? {
                    crate::db::worker_offers::OpenOutcome::Opened(offer) => offer,
                    crate::db::worker_offers::OpenOutcome::SessionCommittedElsewhere {
                        blocking,
                    } => bail!(
                        "CLI session is committed to execution {} attempt {}",
                        blocking.task_execution_id,
                        blocking.attempt_no
                    ),
                };
                if offer.offer_message_id.is_none() {
                    let message = orchestrator_message(
                        format!(
                            "orch-reassign-offer:{}:{}",
                            id, recovery.assignment_generation
                        ),
                        format!(
                            "**Offre de réassignation — génération {}**\n\n\
                             Accepte avec `task_exec_accept_worker_offer({{ offer_id: \"{}\" }})`. \
                             Tu reprendras la même sous-discussion, le même worktree, les manifests, \
                             constats et SHA déjà persistés ; aucun travail validé ne doit être rejoué.",
                            recovery.assignment_generation, offer.id
                        ),
                    );
                    let target = MessageTarget::cli(provider, session_id);
                    crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
                        &tx,
                        &origin,
                        &message,
                        &[target],
                        &[],
                        None,
                    )?;
                    crate::db::worker_offers::set_offer_message(&tx, &offer.id, &message.id)?;
                }
                tx.commit()?;
                let mut view = recovery_view(conn, &id)?;
                view.outcome = Some(format!(
                    "CLI handoff generation {} awaits exact session acceptance",
                    recovery.assignment_generation
                ));
                Ok(view)
            })
            .await;
        return match opened {
            Ok(view) => Json(ApiResponse::ok(view)),
            Err(error) => Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                error.to_string(),
            )),
        };
    }
    Json(ApiResponse::err_coded(
        ApiErrorCode::Conflict,
        "CLI reassignment did not persist a CLI worker identity",
    ))
}

/// Body of `POST /api/orchestration/provision` — the raw launch primitive.
#[derive(Deserialize)]
pub struct ProvisionRequest {
    /// `KT-###` or the task uuid.
    pub task_reference: String,
    /// The principal room the plan lives in.
    pub parent_discussion_id: String,
    /// The chosen worker identity (native, or an exact joined `Cli` session).
    pub worker: MessageTarget,
    #[serde(default)]
    pub base_rev: Option<String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// Principal-owned mechanical gates. Never sourced from the worker manifest.
    #[serde(default)]
    pub validations: Vec<crate::models::ValidationSpec>,
    #[serde(default)]
    pub worker_scope: Option<TaskWorkerScope>,
}

#[derive(Deserialize)]
pub struct TaskExecPrepareRequest {
    pub task_reference: String,
    pub parent_discussion_id: String,
    pub worker: MessageTarget,
    pub source_agent: String,
    pub source_session_id: String,
    #[serde(default)]
    pub worker_scope_intent: Option<TaskWorkerScopeIntent>,
    #[serde(default)]
    pub worker_scope: Option<TaskWorkerScope>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskWorkerScopeIntent {
    Generic,
    Scoped,
}

#[derive(Deserialize)]
pub struct TaskWorkerCatalogueRequest {
    pub parent_discussion_id: String,
    pub source_agent: String,
    pub source_session_id: String,
}

#[derive(Deserialize)]
pub struct TaskExecLaunchRequest {
    pub task_reference: String,
    pub parent_discussion_id: String,
    pub worker: MessageTarget,
    #[serde(default)]
    pub base_rev: Option<String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// Principal-owned mechanical gates persisted on the implicit run.
    #[serde(default)]
    pub validations: Vec<crate::models::ValidationSpec>,
    pub source_agent: String,
    pub source_session_id: String,
    #[serde(default)]
    pub worker_scope_intent: Option<TaskWorkerScopeIntent>,
    #[serde(default)]
    pub worker_scope: Option<TaskWorkerScope>,
}

#[derive(Deserialize)]
pub struct TaskExecCallerRequest {
    pub source_agent: String,
    pub source_session_id: String,
}

#[derive(Deserialize)]
pub struct TaskExecCancelRequest {
    pub source_agent: String,
    pub source_session_id: String,
    #[serde(default = "default_cancel_reason")]
    pub reason: String,
    #[serde(default)]
    pub cleanup_policy: Option<crate::models::CancellationCleanupPolicy>,
}

#[derive(Deserialize)]
pub struct TaskExecReassignRequest {
    pub source_agent: String,
    pub source_session_id: String,
    pub worker: crate::models::CampaignWorkerSelection,
    pub reason: String,
}

fn caller_fields(agent: &str, session_id: &str) -> Option<(String, String)> {
    let agent = agent.trim();
    let session_id = session_id.trim();
    (!agent.is_empty() && !session_id.is_empty())
        .then(|| (agent.to_string(), session_id.to_string()))
}

fn principal_cli_is_authorized(
    conn: &rusqlite::Connection,
    parent_discussion_id: &str,
    source_agent: &str,
    source_session_id: &str,
) -> Result<bool> {
    Ok(
        crate::db::discussion_sessions::find_active_session(conn, source_agent, source_session_id)?
            .is_some_and(|session| session.disc_id == parent_discussion_id),
    )
}

fn execution_party_is_authorized(
    conn: &rusqlite::Connection,
    execution: &TaskExecution,
    source_agent: &str,
    source_session_id: &str,
) -> Result<bool> {
    let Some(session) =
        crate::db::discussion_sessions::find_active_session(conn, source_agent, source_session_id)?
    else {
        return Ok(false);
    };
    Ok(session.disc_id == execution.parent_discussion_id
        || execution.worker_cli_session_id == Some(session.id))
}

pub(crate) fn resolve_task_execution_reference(
    conn: &rusqlite::Connection,
    reference: &str,
) -> Result<Option<TaskExecution>> {
    if let Some(execution) = crate::db::orchestration::get_task_execution(conn, reference)? {
        return Ok(Some(execution));
    }
    let Some(task) = crate::db::planning::get_task(conn, reference)? else {
        return Ok(None);
    };
    if let Some(active) =
        crate::db::orchestration::get_active_execution_for_task(conn, &task.summary.id)?
    {
        return Ok(Some(active));
    }
    crate::db::orchestration::get_latest_execution_for_task(conn, &task.summary.id)
}

fn worker_label(agent: &AgentType) -> &'static str {
    match agent {
        AgentType::ClaudeCode => "Claude Code",
        AgentType::Codex => "Codex",
        AgentType::Vibe => "Vibe",
        AgentType::GeminiCli => "Gemini CLI",
        AgentType::Kiro => "Kiro",
        AgentType::CopilotCli => "GitHub Copilot",
        AgentType::Ollama => "Ollama",
        AgentType::LiteLlm => "LiteLLM",
        AgentType::Nvidia => "NVIDIA",
        AgentType::Custom => "Custom",
    }
}

fn worker_tiers(
    agent: &AgentType,
    model_tiers: &crate::models::ModelTiersConfig,
) -> Vec<crate::models::TaskWorkerTier> {
    [ModelTier::Economy, ModelTier::Default, ModelTier::Reasoning]
        .into_iter()
        .map(|tier| crate::models::TaskWorkerTier {
            tier,
            resolved_model: crate::agents::runner::resolve_model_flag(
                agent,
                tier,
                Some(model_tiers),
            ),
        })
        .collect()
}

fn fixed_worker_reason(code: &str) -> crate::models::CampaignTaskReason {
    let detail = match code {
        "disabled" => "This provider is disabled in Kronn configuration.",
        "not_configured" => "No runnable host runtime or configured HTTP transport is available.",
        "auth_required" => "The authentication required by this provider is not configured.",
        "endpoint_unreachable" => {
            "The HTTP provider did not answer within the bounded discovery probe."
        }
        "model_not_configured" => {
            "No concrete model resolves for this HTTP provider; configure at least one tier."
        }
        "runtime_degraded" => {
            "The detected runtime reports degraded capabilities; inspect Agent settings."
        }
        _ => "The worker is unavailable for a stable, backend-classified reason.",
    };
    preparation_reason(code, detail)
}

fn build_task_worker_catalogue(
    config: &crate::models::AppConfig,
    detections: &[crate::models::AgentDetection],
    joined: &[(
        crate::db::discussion_sessions::DiscussionSession,
        Option<String>,
    )],
    http_reachability: &[(AgentType, bool)],
) -> crate::models::TaskWorkerCatalogue {
    let mut workers = Vec::new();
    let native_agents = [
        AgentType::Ollama,
        AgentType::LiteLlm,
        AgentType::Nvidia,
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::GeminiCli,
        AgentType::Kiro,
        AgentType::CopilotCli,
        AgentType::Vibe,
    ];

    for agent in native_agents {
        let http = crate::agents::runner::is_http_chat_agent(&agent);
        let detection = detections.iter().find(|item| item.agent_type == agent);
        let enabled = detection.is_some_and(|item| item.enabled);
        let runtime_present =
            detection.is_some_and(|item| item.installed || item.runtime_available);
        let auth_ready = if agent == AgentType::Nvidia {
            config
                .tokens
                .active_key_for(crate::api::nvidia::PROVIDER)
                .is_some_and(|key| !key.trim().is_empty())
        } else {
            detection.and_then(|item| item.auth_ready).unwrap_or(true)
        };
        let reachable = if http {
            http_reachability
                .iter()
                .find(|(kind, _)| kind == &agent)
                .is_some_and(|(_, value)| *value)
        } else {
            runtime_present
        };
        let transport_configured = match agent {
            AgentType::Ollama => runtime_present || reachable,
            AgentType::LiteLlm => {
                runtime_present
                    || config
                        .agents
                        .lite_llm
                        .base_url
                        .as_deref()
                        .is_some_and(|v| !v.trim().is_empty())
                    || reachable
            }
            AgentType::Nvidia => true,
            _ => runtime_present,
        };
        let tiers = worker_tiers(&agent, &config.agents.model_tiers);
        let model_configured = !http
            || tiers.iter().any(|tier| {
                tier.resolved_model
                    .as_deref()
                    .is_some_and(|v| !v.trim().is_empty())
            });
        let configured = transport_configured && auth_ready && model_configured;
        let mut reasons = Vec::new();
        if !enabled {
            reasons.push(fixed_worker_reason("disabled"));
        }
        if !transport_configured {
            reasons.push(fixed_worker_reason("not_configured"));
        }
        if !auth_ready {
            reasons.push(fixed_worker_reason("auth_required"));
        }
        if !model_configured {
            reasons.push(fixed_worker_reason("model_not_configured"));
        }
        if http && !reachable {
            reasons.push(fixed_worker_reason("endpoint_unreachable"));
        }
        let worker = if http {
            MessageTarget::discussion_agent(agent.clone()).with_tier(ModelTier::Default)
        } else {
            MessageTarget::agent(agent.clone()).with_tier(ModelTier::Default)
        };
        if let Some(reason) = worker_static_refusal(&worker) {
            reasons.push(reason);
        }
        let warnings = detection
            .and_then(|item| item.runtime_warning.as_ref())
            .map(|_| vec![fixed_worker_reason("runtime_degraded")])
            .unwrap_or_default();
        workers.push(crate::models::TaskWorkerCatalogueEntry {
            worker,
            label: worker_label(&agent).to_string(),
            declared_model: None,
            configured,
            reachable,
            available: enabled && configured && reachable && reasons.is_empty(),
            tiers,
            reasons,
            warnings,
        });
    }

    for (session, alias) in joined {
        let agent = match crate::db::orchestration::agent_type_from_db(&session.agent_type) {
            Ok(agent) => agent,
            Err(_) => continue,
        };
        let worker = MessageTarget::cli(agent.clone(), session.id);
        let mut reasons = Vec::new();
        if let Some(reason) = worker_static_refusal(&worker) {
            reasons.push(reason);
        }
        workers.push(crate::models::TaskWorkerCatalogueEntry {
            worker,
            label: alias
                .clone()
                .unwrap_or_else(|| format!("{} CLI #{}", worker_label(&agent), session.id)),
            declared_model: session.model.clone(),
            configured: true,
            // This is the same durable non-left membership the preflight and
            // launch boundary accept. It means dispatchable by Kronn, not that
            // the external process is generating at this instant.
            reachable: true,
            available: reasons.is_empty(),
            tiers: Vec::new(),
            reasons,
            warnings: Vec::new(),
        });
    }

    crate::models::TaskWorkerCatalogue { workers }
}

async fn bounded_http_worker_reachability(state: &AppState) -> Vec<(AgentType, bool)> {
    let config = state.config.read().await.clone();
    let nvidia_endpoint =
        crate::api::nvidia::resolve_base_url_pub(config.agents.nvidia.base_url.as_deref());
    let nvidia_key = config
        .tokens
        .active_key_for(crate::api::nvidia::PROVIDER)
        .map(str::to_string);
    let timeout = std::time::Duration::from_secs(4);
    let (ollama, lite_llm, nvidia) = tokio::join!(
        tokio::time::timeout(timeout, crate::api::ollama::health(State(state.clone()))),
        tokio::time::timeout(timeout, crate::api::lite_llm::health(State(state.clone()))),
        tokio::time::timeout(
            timeout,
            crate::api::nvidia::probe_catalogue(&nvidia_endpoint, nvidia_key.as_deref())
        ),
    );
    vec![
        (
            AgentType::Ollama,
            ollama
                .ok()
                .and_then(|response| response.0.data)
                .is_some_and(|health| health.status == "online"),
        ),
        (
            AgentType::LiteLlm,
            lite_llm
                .ok()
                .and_then(|response| response.0.data)
                .is_some_and(|health| health.status == "online"),
        ),
        (AgentType::Nvidia, nvidia.is_ok_and(|probe| probe.is_ok())),
    ]
}

pub(crate) async fn task_worker_catalogue_for_discussion(
    state: &AppState,
    parent_discussion_id: &str,
) -> Result<crate::models::TaskWorkerCatalogue> {
    let parent = parent_discussion_id.to_string();
    let joined = state
        .db
        .with_read_conn(move |conn| {
            let sessions = crate::db::discussion_sessions::list_sessions(conn, &parent, false)?;
            sessions
                .into_iter()
                .map(|session| {
                    let (_, alias) =
                        crate::db::discussion_sessions::cli_session_identity(conn, session.id)?;
                    Ok((session, alias))
                })
                .collect::<Result<Vec<_>>>()
        })
        .await?;
    let mut detections = crate::agents::detect_all_cached(false).await;
    let config = state.config.read().await.clone();
    crate::agents::apply_configured_status(&mut detections, &config);
    let reachability = bounded_http_worker_reachability(state).await;
    Ok(build_task_worker_catalogue(
        &config,
        &detections,
        &joined,
        &reachability,
    ))
}

/// MCP-only worker discovery. Caller identity is injected by the bridge and
/// authorized against the principal room before any room/session catalogue is
/// returned.
pub async fn task_worker_catalogue(
    State(state): State<AppState>,
    Json(request): Json<TaskWorkerCatalogueRequest>,
) -> Json<ApiResponse<crate::models::TaskWorkerCatalogue>> {
    let Some((agent, session_id)) =
        caller_fields(&request.source_agent, &request.source_session_id)
    else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "durable source_agent and source_session_id are required",
        ));
    };
    let parent = request.parent_discussion_id.trim().to_string();
    let authorized = {
        let parent = parent.clone();
        state
            .db
            .with_read_conn(move |conn| {
                principal_cli_is_authorized(conn, &parent, &agent, &session_id)
            })
            .await
    };
    if !matches!(authorized, Ok(true)) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "principal discussion not found or caller is not an active member",
        ));
    }
    match task_worker_catalogue_for_discussion(&state, &parent).await {
        Ok(catalogue) => Json(ApiResponse::ok(catalogue)),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

/// MCP-only preflight. Caller identity is injected by the bridge and checked in
/// the backend before task/project/worker details are returned.
pub async fn task_exec_prepare(
    State(state): State<AppState>,
    Json(request): Json<TaskExecPrepareRequest>,
) -> Json<ApiResponse<crate::models::TaskExecutionPreparation>> {
    let Some((agent, session_id)) =
        caller_fields(&request.source_agent, &request.source_session_id)
    else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "durable source_agent and source_session_id are required",
        ));
    };
    if let Some(reason) =
        worker_scope_contract_refusal(request.worker_scope_intent, request.worker_scope.as_ref())
    {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            format!("{}: {}", reason.code, reason.detail),
        ));
    }
    let parent = request.parent_discussion_id.trim().to_string();
    let task = request.task_reference.trim().to_string();
    let worker = request.worker;
    let scope_refusal = worker_scope_refusal(&worker, request.worker_scope.as_ref());
    let result = state
        .db
        .with_conn(move |conn| {
            if !principal_cli_is_authorized(conn, &parent, &agent, &session_id)? {
                bail!("principal discussion not found or caller is not an active member");
            }
            let mut preparation = prepare_task_execution(conn, &task, &parent, &worker)?;
            if let Some(reason) = scope_refusal {
                preparation.launchable = false;
                preparation.reasons.push(reason);
            }
            Ok(preparation)
        })
        .await;
    match result {
        Ok(preparation) => Json(ApiResponse::ok(preparation)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            error.to_string(),
        )),
    }
}

pub async fn task_exec_launch(
    State(state): State<AppState>,
    Json(request): Json<TaskExecLaunchRequest>,
) -> Json<ApiResponse<TaskExecution>> {
    let Some((agent, session_id)) =
        caller_fields(&request.source_agent, &request.source_session_id)
    else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "durable source_agent and source_session_id are required",
        ));
    };
    if let Some(reason) =
        worker_scope_contract_refusal(request.worker_scope_intent, request.worker_scope.as_ref())
    {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            format!("{}: {}", reason.code, reason.detail),
        ));
    }
    let parent = request.parent_discussion_id.trim().to_string();
    let authorized = {
        let parent = parent.clone();
        state
            .db
            .with_conn(move |conn| principal_cli_is_authorized(conn, &parent, &agent, &session_id))
            .await
    };
    if !matches!(authorized, Ok(true)) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "principal discussion not found or caller is not an active member",
        ));
    }
    if let Some(reason) = worker_scope_refusal(&request.worker, request.worker_scope.as_ref()) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            format!("{}: {}", reason.code, reason.detail),
        ));
    }
    let preflight = {
        let task = request.task_reference.trim().to_string();
        let principal = parent.clone();
        let worker = request.worker.clone();
        state
            .db
            .with_conn(move |conn| prepare_task_execution(conn, &task, &principal, &worker))
            .await
    };
    match preflight {
        Ok(preparation) if preparation.launchable => {}
        Ok(preparation) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                format!(
                    "task is not launchable: {}",
                    serde_json::to_string(&preparation.reasons)
                        .unwrap_or_else(|_| "preflight reasons unavailable".into())
                ),
            ));
        }
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                error.to_string(),
            ));
        }
    }
    match provision_single_task_execution_with_scope_and_validations(
        &state.db,
        ProvisionInput {
            task_reference: request.task_reference,
            parent_discussion_id: parent,
            worker: request.worker,
            base_rev: request.base_rev,
            idempotency_key: request.idempotency_key,
        },
        request.worker_scope,
        request.validations,
    )
    .await
    {
        Ok(execution) => Json(ApiResponse::ok(execution)),
        Err(error) => Json(provision_error_to_response(error)),
    }
}

pub async fn task_exec_status(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
    Json(request): Json<TaskExecCallerRequest>,
) -> Json<ApiResponse<crate::models::TaskExecutionDetail>> {
    let Some((agent, session_id)) =
        caller_fields(&request.source_agent, &request.source_session_id)
    else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "durable source_agent and source_session_id are required",
        ));
    };
    let result = state
        .db
        .with_conn(move |conn| {
            let execution = resolve_task_execution_reference(conn, &exec_id)?
                .context("execution not found or caller is not a party")?;
            if !execution_party_is_authorized(conn, &execution, &agent, &session_id)? {
                bail!("execution not found or caller is not a party");
            }
            execution_detail(conn, &execution.id)
        })
        .await;
    match result {
        Ok(detail) => Json(ApiResponse::ok(detail)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            error.to_string(),
        )),
    }
}

pub async fn task_exec_cancel(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
    Json(request): Json<TaskExecCancelRequest>,
) -> Json<ApiResponse<ExecutionRecoveryView>> {
    let Some((agent, session_id)) =
        caller_fields(&request.source_agent, &request.source_session_id)
    else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "durable source_agent and source_session_id are required",
        ));
    };
    let authorized = {
        let id = exec_id.clone();
        state
            .db
            .with_conn(move |conn| {
                let execution = crate::db::orchestration::get_task_execution(conn, &id)?
                    .context("execution not found or caller is not its principal")?;
                principal_cli_is_authorized(
                    conn,
                    &execution.parent_discussion_id,
                    &agent,
                    &session_id,
                )
            })
            .await
    };
    if !matches!(authorized, Ok(true)) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "execution not found or caller is not its principal",
        ));
    }
    cancel_execution(
        State(state),
        Path(exec_id),
        Json(CancelExecutionRequest {
            reason: request.reason,
            cleanup_policy: request.cleanup_policy,
        }),
    )
    .await
}

pub async fn task_exec_reassign(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
    Json(request): Json<TaskExecReassignRequest>,
) -> Json<ApiResponse<ExecutionRecoveryView>> {
    let Some((agent, session_id)) =
        caller_fields(&request.source_agent, &request.source_session_id)
    else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "durable source_agent and source_session_id are required",
        ));
    };
    let authorized = {
        let id = exec_id.clone();
        state
            .db
            .with_conn(move |conn| {
                let execution = crate::db::orchestration::get_task_execution(conn, &id)?
                    .context("execution not found or caller is not its principal")?;
                principal_cli_is_authorized(
                    conn,
                    &execution.parent_discussion_id,
                    &agent,
                    &session_id,
                )
            })
            .await
    };
    if !matches!(authorized, Ok(true)) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "execution not found or caller is not its principal",
        ));
    }
    reassign_execution(
        State(state),
        Path(exec_id),
        Json(ReassignExecutionRequest {
            worker: request.worker,
            reason: request.reason,
        }),
    )
    .await
}

/// Map a launch-saga refusal/failure to a stable `(code, message)`. `ProvisionError` has
/// no `Display` on purpose (each variant carries structured fields), so both handlers
/// share this. A mid-saga workspace failure is `Conflict` (resumable), never a bare 500 —
/// DoD-6's "no silent orphan" surfaced to the caller.
pub(crate) fn provision_error_parts(error: &ProvisionError) -> (ApiErrorCode, String) {
    match error {
        ProvisionError::TaskNotFound => (ApiErrorCode::NotFound, "task not found".to_string()),
        ProvisionError::NotLaunchable(reason) => (ApiErrorCode::Validation, reason.clone()),
        ProvisionError::WorkspaceFailed {
            reason,
            compensated,
        } => (
            ApiErrorCode::Conflict,
            format!("workspace step failed (compensated={compensated}, resumable): {reason}"),
        ),
        ProvisionError::CheckpointRefused(reason) => (
            ApiErrorCode::Conflict,
            format!("checkpoint refused (resumable): {reason}"),
        ),
        ProvisionError::Internal(reason) => (ApiErrorCode::Internal, reason.clone()),
    }
}

fn provision_error_to_response(error: ProvisionError) -> ApiResponse<TaskExecution> {
    let (code, message) = provision_error_parts(&error);
    ApiResponse::err_coded(code, message)
}

/// `POST /api/orchestration/provision` — launch one ready task into a fresh
/// sub-discussion + SHA-pinned worktree. A native worker drives to `Working`; a `Cli`
/// worker opens a durable control offer and parks `Blocked` awaiting acceptance (KT-328).
pub async fn provision(
    State(state): State<AppState>,
    Json(request): Json<ProvisionRequest>,
) -> Json<ApiResponse<TaskExecution>> {
    let input = ProvisionInput {
        task_reference: request.task_reference,
        parent_discussion_id: request.parent_discussion_id,
        worker: request.worker,
        base_rev: request.base_rev,
        idempotency_key: request.idempotency_key,
    };
    match provision_single_task_execution_with_scope_and_validations(
        &state.db,
        input,
        request.worker_scope,
        request.validations,
    )
    .await
    {
        Ok(execution) => Json(ApiResponse::ok(execution)),
        Err(error) => Json(provision_error_to_response(error)),
    }
}

/// Body of `POST /api/orchestration/accept-offer`. The caller passes ONLY the opaque
/// `offer_id`; the bridge auto-fills both the live exact-session identity and the
/// separate durable room-binding identity. The model supplies neither.
#[derive(Deserialize)]
pub struct AcceptOfferRequest {
    pub offer_id: String,
    #[serde(default)]
    pub source_agent: Option<String>,
    #[serde(default)]
    pub source_session_id: Option<String>,
    #[serde(default)]
    pub source_binding_session_id: Option<String>,
}

/// The attach payload the bridge needs to rebind: the child room to follow.
#[derive(Serialize)]
pub struct AcceptOfferResponse {
    pub child_discussion_id: String,
    pub execution: TaskExecution,
}

/// Map the accept outcome to an HTTP response. Pure, so the anti-oracle fusion is unit
/// tested without an `AppState`: an unknown offer and a wrong caller collapse into ONE
/// opaque refusal, so a non-target session cannot enumerate live offer ids or probe
/// which session an offer targets. The other refusals are only reachable AFTER the
/// caller == target check, so naming the real state there is not an oracle.
fn accept_outcome_to_response(outcome: AcceptAttachOutcome) -> ApiResponse<AcceptOfferResponse> {
    match outcome {
        AcceptAttachOutcome::Attached {
            child_discussion_id,
            execution,
        } => ApiResponse::ok(AcceptOfferResponse {
            child_discussion_id,
            execution,
        }),
        AcceptAttachOutcome::NotFound | AcceptAttachOutcome::WrongAcceptor => {
            ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "offer not found or not addressed to this session".to_string(),
            )
        }
        AcceptAttachOutcome::BindingMismatch => ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            "this CLI session is not durably bound to the offer room; reconnect or explicitly transfer the session, then retry"
                .to_string(),
        ),
        AcceptAttachOutcome::NotAcceptable { status } => ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            format!("offer no longer acceptable ({})", status.as_str()),
        ),
        AcceptAttachOutcome::Expired => {
            ApiResponse::err_coded(ApiErrorCode::Conflict, "offer expired".to_string())
        }
        AcceptAttachOutcome::CheckpointRefused(reason) => ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            format!("final checkpoint refused (resumable): {reason}"),
        ),
    }
}

/// `POST /api/orchestration/accept-offer` — the exact targeted CLI session accepts its
/// control offer and is attached to the sub-discussion. Identity is derived server-side
/// from the bridge-supplied live pair, then moves the bridge-supplied durable
/// room binding; the model passes only `offer_id`.
pub async fn accept_offer(
    State(state): State<AppState>,
    Json(request): Json<AcceptOfferRequest>,
) -> Json<ApiResponse<AcceptOfferResponse>> {
    let clean = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let (source_agent, source_session_id, source_binding_session_id) = match (
        clean(&request.source_agent),
        clean(&request.source_session_id),
        clean(&request.source_binding_session_id),
    ) {
        (Some(agent), Some(session), Some(binding_session)) => (agent, session, binding_session),
        _ => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                "source_agent, source_session_id, and source_binding_session_id are required"
                    .to_string(),
            ))
        }
    };
    let offer_id = request.offer_id.trim().to_string();
    if offer_id.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "offer_id is required".to_string(),
        ));
    }
    match accept_worker_offer_and_attach(
        &state.db,
        &offer_id,
        &source_agent,
        &source_session_id,
        &source_binding_session_id,
    )
    .await
    {
        Ok(outcome) => Json(accept_outcome_to_response(outcome)),
        Err(error) => {
            let (code, message) = provision_error_parts(&error);
            Json(ApiResponse::err_coded(code, message))
        }
    }
}

/// Out-of-band runner identity shared by the spawned-worker commit and delivery
/// boundaries. The bridge injects it from `KRONN_TASK_WORKER_CONTEXT`; it is
/// deliberately absent from both model-visible MCP schemas.
#[derive(Deserialize)]
pub struct SpawnedAgentCaller {
    pub discussion_id: String,
    pub agent_type: String,
    pub source_message_id: String,
}

fn spawned_native_caller(
    caller: &SpawnedAgentCaller,
) -> Result<(AgentType, &str, &str), &'static str> {
    let discussion_id = caller.discussion_id.trim();
    let source_message_id = caller.source_message_id.trim();
    let agent_type = crate::db::orchestration::agent_type_from_db(caller.agent_type.trim())
        .map_err(|_| "spawned agent context is incomplete")?;
    if discussion_id.is_empty() || source_message_id.is_empty() {
        return Err("spawned agent context is incomplete");
    }
    Ok((agent_type, discussion_id, source_message_id))
}

/// Body of `POST /api/orchestration/worker-commit`. The model supplies only an
/// explicit file inventory and message. Execution, provider, dispatch, branch
/// and worktree are runner-owned authority and cannot be redirected by args.
#[derive(Deserialize)]
pub struct SpawnedWorkerCommitRequest {
    pub task_execution_id: String,
    pub files: Vec<String>,
    pub message: String,
    pub spawned_agent: SpawnedAgentCaller,
}

/// `POST /api/orchestration/worker-commit` — commit an exact spawned worker's
/// explicit paths without granting the model access to shared Git objects or
/// refs. There is intentionally no amend, push, branch, ref or path-to-repo
/// parameter.
pub async fn commit_spawned_worker(
    State(state): State<AppState>,
    Json(request): Json<SpawnedWorkerCommitRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let exec_id = request.task_execution_id.trim().to_string();
    if exec_id.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "task_execution_id is required".to_string(),
        ));
    }
    let (agent_type, discussion_id, source_message_id) =
        match spawned_native_caller(&request.spawned_agent) {
            Ok(caller) => caller,
            Err(message) => {
                return Json(ApiResponse::err_coded(
                    ApiErrorCode::Validation,
                    message.to_string(),
                ))
            }
        };
    let alias = crate::db::orchestration::agent_type_to_db(&agent_type);
    let execution = match native_worker_execution_for_caller(
        &state.db,
        &exec_id,
        NativeExecutionCaller {
            discussion_id,
            agent_type: &agent_type,
            source_message_id: Some(source_message_id),
            alias: &alias,
            actor_session_id: Some(source_message_id),
        },
    )
    .await
    {
        Ok(Some(execution)) => execution,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "execution not found or not addressed to this worker".to_string(),
            ))
        }
        Err(error) => {
            let (code, message) = provision_error_parts(&error);
            return Json(ApiResponse::err_coded(code, message));
        }
    };
    if execution.status != TaskExecutionStatus::Working {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            format!(
                "execution cannot commit in state {}",
                execution.status.as_str()
            ),
        ));
    }

    let execution_id = execution.id.clone();
    let workspace = match state
        .db
        .with_read_conn(move |conn| {
            crate::db::discussion_workspaces::get_managed_for_execution(conn, &execution_id)
        })
        .await
    {
        Ok(Some(workspace)) => workspace,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                "execution has no attached managed worktree".to_string(),
            ))
        }
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                format!("unable to resolve worker worktree: {error}"),
            ))
        }
    };
    let exact_workspace = execution.workspace_id.as_deref() == Some(workspace.id.as_str())
        && workspace.task_execution_id.as_deref() == Some(execution.id.as_str())
        && workspace.disc_id == discussion_id
        && workspace.ownership == "managed"
        && workspace.state == "attached";
    if !exact_workspace {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            "execution worktree authority is stale or mismatched".to_string(),
        ));
    }
    let Some(stored_path) = workspace.canonical_path else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            "execution worktree has no canonical path".to_string(),
        ));
    };
    let root_is_real_directory = std::fs::symlink_metadata(&stored_path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink());
    let root = match (root_is_real_directory, std::fs::canonicalize(&stored_path)) {
        (true, Ok(root)) => root,
        _ => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                "execution worktree canonical path drifted".to_string(),
            ))
        }
    };
    let files = request.files;
    let message = request.message;
    let committed = tokio::task::spawn_blocking(move || {
        crate::api::agent_workspace_tools::git_commit_payload(&root, &files, &message)
    })
    .await;
    match committed {
        Ok(Ok(payload)) => Json(ApiResponse::ok(payload)),
        Ok(Err(message)) => Json(ApiResponse::err_coded(ApiErrorCode::Validation, message)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("worker commit task failed: {error}"),
        )),
    }
}

/// Body of `POST /api/orchestration/deliver`. The worker names its `task_execution_id` and
/// submits the `manifest`; the bridge auto-fills the durable `(source_agent,
/// source_session_id)` from which the SERVER derives the session — the model never supplies
/// a session id, and only the execution's exact worker is authorized.
#[derive(Deserialize)]
pub struct DeliverRequest {
    pub task_execution_id: String,
    /// The DeliveryManifest v1 as JSON (validated against the contract before persistence).
    pub manifest: serde_json::Value,
    #[serde(default)]
    pub source_agent: Option<String>,
    #[serde(default)]
    pub source_session_id: Option<String>,
    /// Out-of-band identity injected by Kronn when a discussion agent is
    /// backed by a host CLI. This field is absent from the MCP tool schema:
    /// the bridge copies it from the runner environment, never from model
    /// arguments, then the native authorization path revalidates every value.
    #[serde(default)]
    pub spawned_agent: Option<SpawnedAgentCaller>,
}

/// The delivery receipt: the parent room the review request landed in + the execution now
/// `AwaitingReview`.
#[derive(Serialize)]
pub struct DeliverResponse {
    pub review_discussion_id: String,
    pub execution: TaskExecution,
}

/// Map the deliver outcome to an HTTP response. Pure, so the anti-oracle fusion is unit
/// tested without an `AppState`: an unknown execution and a wrong caller collapse into ONE
/// opaque refusal. `NotDeliverable`/`InvalidManifest` are reachable only AFTER the caller
/// == worker check, so naming the real state there is not an oracle.
pub(crate) fn deliver_outcome_to_response(outcome: DeliverOutcome) -> ApiResponse<DeliverResponse> {
    match outcome {
        DeliverOutcome::Delivered {
            review_discussion_id,
            execution,
        } => ApiResponse::ok(DeliverResponse {
            review_discussion_id,
            execution,
        }),
        DeliverOutcome::NotAddressed => ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "execution not found or not addressed to this session".to_string(),
        ),
        DeliverOutcome::NotDeliverable { status } => ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            format!("execution not deliverable ({})", status.as_str()),
        ),
        DeliverOutcome::InvalidManifest(detail) => {
            ApiResponse::err_coded(ApiErrorCode::Validation, detail)
        }
    }
}

/// `POST /api/orchestration/deliver` — a worker submits its DeliveryManifest for review.
/// Identity is derived server-side from the bridge-supplied durable pair; the caller
/// supplies only `task_execution_id` + `manifest`.
pub async fn deliver(
    State(state): State<AppState>,
    Json(request): Json<DeliverRequest>,
) -> Json<ApiResponse<DeliverResponse>> {
    let clean = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let exec_id = request.task_execution_id.trim().to_string();
    if exec_id.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "task_execution_id is required".to_string(),
        ));
    }
    let manifest_json = match serde_json::to_string(&request.manifest) {
        Ok(s) => s,
        Err(e) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                format!("manifest is not serializable JSON: {e}"),
            ))
        }
    };
    let outcome = if let Some(caller) = request.spawned_agent {
        if clean(&request.source_agent).is_some() || clean(&request.source_session_id).is_some() {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                "choose exactly one delivery identity mode".to_string(),
            ));
        }
        let (agent_type, discussion_id, source_message_id) = match spawned_native_caller(&caller) {
            Ok(caller) => caller,
            Err(_) => {
                return Json(ApiResponse::err_coded(
                    ApiErrorCode::Validation,
                    "spawned agent delivery context is incomplete".to_string(),
                ))
            }
        };
        let alias = crate::db::orchestration::agent_type_to_db(&agent_type);
        deliver_native_worker_manifest(
            &state.db,
            &exec_id,
            NativeExecutionCaller {
                discussion_id,
                agent_type: &agent_type,
                source_message_id: Some(source_message_id),
                alias: &alias,
                actor_session_id: Some(source_message_id),
            },
            &manifest_json,
        )
        .await
    } else {
        let (source_agent, source_session_id) = match (
            clean(&request.source_agent),
            clean(&request.source_session_id),
        ) {
            (Some(agent), Some(session)) => (agent, session),
            _ => {
                return Json(ApiResponse::err_coded(
                    ApiErrorCode::Validation,
                    "source_agent and source_session_id are required".to_string(),
                ))
            }
        };
        deliver_worker_manifest(
            &state.db,
            &exec_id,
            &source_agent,
            &source_session_id,
            &manifest_json,
        )
        .await
    };
    match outcome {
        Ok(outcome) => Json(deliver_outcome_to_response(outcome)),
        Err(error) => {
            let (code, message) = provision_error_parts(&error);
            Json(ApiResponse::err_coded(code, message))
        }
    }
}

/// Validate + parse a submitted DeliveryManifest against the v1 contract (KT-319
/// DoD-1). A shape error surfaces as `Err`, which the deliver handler (tranche 2)
/// turns into a typed refusal — never a silent accept. Pure: no I/O.
pub fn parse_delivery_manifest(manifest_json: &str) -> Result<DeliveryManifestV1> {
    crate::workflows::template::validate_envelope_against_schema(
        manifest_json,
        &crate::models::delivery_manifest_v1_schema(),
    )
    .map_err(|e| anyhow::anyhow!("DeliveryManifest v1 invalide : {e}"))?;
    serde_json::from_str(manifest_json)
        .context("DeliveryManifest v1 : structure JSON non conforme au contrat typé")
}

/// Validate + parse a submitted ReviewDecision against the v1 contract (KT-319).
/// Beyond the JSON-subset schema, this enforces the one rule the subset cannot
/// express: `request_changes` MUST carry a non-empty `comment` (a change request
/// with no reason is not actionable by the worker — DoD-4). Pure: no I/O.
pub fn parse_review_decision(decision_json: &str) -> Result<ReviewDecisionV1> {
    crate::workflows::template::validate_envelope_against_schema(
        decision_json,
        &crate::models::review_decision_v1_schema(),
    )
    .map_err(|e| anyhow::anyhow!("ReviewDecision v1 invalide : {e}"))?;
    let decision: ReviewDecisionV1 = serde_json::from_str(decision_json)
        .context("ReviewDecision v1 : structure JSON non conforme au contrat typé")?;
    if decision.decision == ReviewVerdict::RequestChanges
        && decision
            .comment
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
    {
        bail!("ReviewDecision v1 : `request_changes` exige un `comment` non vide");
    }
    let mut verification_ids = std::collections::HashSet::new();
    for verification in &decision.dod_verifications {
        if !verification_ids.insert(verification.dod_id.as_str()) {
            bail!(
                "ReviewDecision v1 : preuve DoD dupliquée pour `{}`",
                verification.dod_id
            );
        }
        if verification.evidence.trim().is_empty() {
            bail!(
                "ReviewDecision v1 : la preuve DoD `{}` doit être non vide",
                verification.dod_id
            );
        }
    }
    Ok(decision)
}

/// Body of `POST /api/orchestration/review`. The principal names its `task_execution_id` and
/// submits the `decision`; the bridge auto-fills the durable `(source_agent,
/// source_session_id)` from which the SERVER derives the deciding identity — the model never
/// supplies a session id, and only a party to the execution is authorized.
#[derive(Deserialize)]
pub struct ReviewRequest {
    pub task_execution_id: String,
    /// The ReviewDecision v1 as JSON (validated against the contract AFTER authz).
    pub decision: serde_json::Value,
    #[serde(default)]
    pub source_agent: Option<String>,
    #[serde(default)]
    pub source_session_id: Option<String>,
}

#[derive(Deserialize)]
pub struct HumanReviewRequest {
    pub decision: serde_json::Value,
}

/// The review receipt: the applied verdict + the execution now `Approved` / `ChangesRequested`.
#[derive(Serialize)]
pub struct ReviewResponse {
    /// "approve" | "request_changes".
    pub verdict: String,
    pub execution: TaskExecution,
}

/// Human-readable, actionable cause for an approve refusal (DoD-5). Only ever shown to an
/// authorized principal, so precision is help, not an oracle.
fn approve_block_message(reason: &ApproveBlockReason) -> String {
    match reason {
        ApproveBlockReason::NoManifest => {
            "no delivery manifest for the current attempt".to_string()
        }
        ApproveBlockReason::ManifestClaimsInvalid(detail) => {
            format!("delivery manifest claims are invalid: {detail}")
        }
        ApproveBlockReason::ReviewedHeadMismatch { reviewed, delivered } => format!(
            "reviewed_head_sha does not identify the delivered commit (reviewed {reviewed}, delivered {delivered})"
        ),
        ApproveBlockReason::ReviewEvidenceInvalid(detail) => {
            format!("review DoD evidence is invalid: {detail}")
        }
        ApproveBlockReason::HeadDrifted { delivered, current } => {
            format!("worktree HEAD drifted since delivery (delivered {delivered}, now {current})")
        }
        ApproveBlockReason::DodNotMet { unmet } => {
            format!("{} DoD item(s) not met: {}", unmet.len(), unmet.join(", "))
        }
        ApproveBlockReason::WorktreeUnavailable(detail) => {
            format!("cannot confirm the worktree HEAD: {detail}")
        }
        ApproveBlockReason::WorktreeDirty { files } => format!(
            "worktree contains uncommitted changes; commit or discard them before approval: {}",
            files.join(", ")
        ),
        ApproveBlockReason::ManifestDiffMismatch(detail) => {
            format!("delivery manifest does not match the committed diff: {detail}")
        }
    }
}

/// Map the review outcome to an HTTP response. Pure, so the anti-oracle fusion is unit tested
/// without an `AppState`: an unknown execution and a caller who is neither worker nor principal
/// collapse into ONE opaque refusal. `SelfReviewForbidden` / `NotReviewable` / `ApproveBlocked`
/// / `InvalidDecision` are reachable only AFTER the caller is established as a party, so naming
/// the real reason there is not an oracle.
pub(crate) fn review_outcome_to_response(outcome: ReviewOutcome) -> ApiResponse<ReviewResponse> {
    match outcome {
        ReviewOutcome::Reviewed { verdict, execution } => ApiResponse::ok(ReviewResponse {
            verdict: match verdict {
                ReviewVerdict::Approve => "approve".to_string(),
                ReviewVerdict::RequestChanges => "request_changes".to_string(),
            },
            execution,
        }),
        ReviewOutcome::NotAddressed => ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "execution not found or not addressed to this session".to_string(),
        ),
        ReviewOutcome::SelfReviewForbidden => ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            "the worker cannot decide its own review (self-review is not enabled for this run)"
                .to_string(),
        ),
        ReviewOutcome::NotReviewable { status } => ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            format!("execution not reviewable ({})", status.as_str()),
        ),
        ReviewOutcome::ApproveBlocked { reason } => ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            format!("approve refused: {}", approve_block_message(&reason)),
        ),
        // A successful request_changes that exhausted the review budget: the decision applied and
        // the execution is now `Escalated` (its `status` carries the signal). Returned `ok` — it
        // is a valid outcome, not a refusal — so the principal reads `execution.status`.
        ReviewOutcome::Escalated { execution } => ApiResponse::ok(ReviewResponse {
            verdict: "request_changes".to_string(),
            execution,
        }),
        ReviewOutcome::InvalidDecision(detail) => {
            ApiResponse::err_coded(ApiErrorCode::Validation, detail)
        }
    }
}

/// `POST /api/orchestration/review` — the principal decides a delivered attempt (approve |
/// request_changes). Identity is derived server-side from the bridge-supplied durable pair;
/// the caller supplies only `task_execution_id` + `decision`.
pub async fn review(
    State(state): State<AppState>,
    Json(request): Json<ReviewRequest>,
) -> Json<ApiResponse<ReviewResponse>> {
    let clean = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let (source_agent, source_session_id) = match (
        clean(&request.source_agent),
        clean(&request.source_session_id),
    ) {
        (Some(agent), Some(session)) => (agent, session),
        _ => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                "source_agent and source_session_id are required".to_string(),
            ))
        }
    };
    let exec_id = request.task_execution_id.trim().to_string();
    if exec_id.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "task_execution_id is required".to_string(),
        ));
    }
    let decision_json = match serde_json::to_string(&request.decision) {
        Ok(s) => s,
        Err(e) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                format!("decision is not serializable JSON: {e}"),
            ))
        }
    };
    match decide_review(
        &state.db,
        &exec_id,
        &decision_json,
        &source_agent,
        &source_session_id,
    )
    .await
    {
        Ok(outcome) => match continue_approved_review(&state.db, outcome).await {
            Ok(outcome) => Json(review_outcome_to_response(outcome)),
            Err(error) => {
                let (code, message) = provision_error_parts(&error);
                Json(ApiResponse::err_coded(code, message))
            }
        },
        Err(error) => {
            let (code, message) = provision_error_parts(&error);
            Json(ApiResponse::err_coded(code, message))
        }
    }
}

/// Authenticated web-UI review. Unlike the agent/CLI endpoint, the actor is the
/// human operating Kronn, so there is no model-controlled session identity to
/// resolve and no self-review ambiguity.
pub async fn human_review(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
    Json(request): Json<HumanReviewRequest>,
) -> Json<ApiResponse<ReviewResponse>> {
    let decision_json = match serde_json::to_string(&request.decision) {
        Ok(value) => value,
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                format!("decision is not serializable JSON: {error}"),
            ))
        }
    };
    let execution = {
        let id = exec_id.clone();
        state
            .db
            .with_conn(move |conn| crate::db::orchestration::get_task_execution(conn, &id))
            .await
    };
    let execution = match execution {
        Ok(Some(execution)) => execution,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "task execution not found",
            ))
        }
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                error.to_string(),
            ))
        }
    };
    match decide_authorized_review(&state.db, execution, "Human", None, true, &decision_json).await
    {
        Ok(outcome) => match continue_approved_review(&state.db, outcome).await {
            Ok(outcome) => Json(review_outcome_to_response(outcome)),
            Err(error) => {
                let (code, message) = provision_error_parts(&error);
                Json(ApiResponse::err_coded(code, message))
            }
        },
        Err(error) => {
            let (code, message) = provision_error_parts(&error);
            Json(ApiResponse::err_coded(code, message))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AgentType, AiAuditStatus, AiConfigStatus, CreatePlanningDodItem, CreatePlanningTaskRequest,
        PlanningTaskStatus, Project, ValidationSpec,
    };
    use std::path::Path;
    use std::process::Command;

    #[test]
    fn recovered_apply_must_win_its_durable_claim_before_touching_git() {
        assert!(require_recovered_apply_claim(true).is_ok());

        let error = require_recovered_apply_claim(false)
            .expect_err("a lost CAS must stop before the shared parent checkout is touched");
        let ProvisionError::CheckpointRefused(reason) = error else {
            panic!("lost claim returned the wrong error: {error:?}");
        };
        assert!(reason.contains("already claimed"));
    }

    #[test]
    fn boot_agent_availability_requires_runtime_enablement_and_auth() {
        let detection = |agent_type, installed, runtime_available, enabled, auth_ready| {
            crate::models::AgentDetection {
                name: format!("{agent_type:?}"),
                agent_type,
                installed,
                enabled,
                path: None,
                version: None,
                latest_version: None,
                origin: "test".into(),
                install_command: None,
                host_managed: false,
                host_label: None,
                runtime_available,
                auth_ready: Some(auth_ready),
                auth_setup_command: None,
                rtk_available: false,
                rtk_hook_configured: false,
                runtime_warning: None,
            }
        };
        let available = available_agent_types(vec![
            detection(AgentType::Codex, true, false, true, true),
            detection(AgentType::ClaudeCode, true, false, false, true),
            detection(AgentType::Vibe, false, true, true, false),
            detection(AgentType::Ollama, false, true, true, true),
        ]);
        assert_eq!(available, vec![AgentType::Codex, AgentType::Ollama]);
    }

    #[test]
    fn worker_catalogue_unifies_ollama_joined_cli_and_unavailable_provider() {
        let detection =
            |agent_type, installed, runtime_available, enabled| crate::models::AgentDetection {
                name: format!("{agent_type:?}"),
                agent_type,
                installed,
                enabled,
                path: None,
                version: None,
                latest_version: None,
                origin: "test".into(),
                install_command: None,
                host_managed: false,
                host_label: None,
                runtime_available,
                auth_ready: Some(true),
                auth_setup_command: None,
                rtk_available: false,
                rtk_hook_configured: false,
                runtime_warning: None,
            };
        let detections = vec![
            detection(AgentType::Ollama, true, true, true),
            detection(AgentType::LiteLlm, false, false, true),
            detection(AgentType::Nvidia, true, true, true),
            detection(AgentType::Codex, true, true, true),
        ];
        let joined = vec![(
            crate::db::discussion_sessions::DiscussionSession {
                id: 77,
                disc_id: "d-parent".into(),
                agent_type: "Codex".into(),
                session_id: Some("codex-session".into()),
                role: "peer".into(),
                status: "active".into(),
                joined_at: "now".into(),
                left_at: None,
                last_seen: None,
                activity: None,
                model: Some("gpt-5.6-sol".into()),
                conversation_id: None,
            },
            Some("@codex-cli".into()),
        )];
        let catalogue = build_task_worker_catalogue(
            &crate::core::config::default_config(),
            &detections,
            &joined,
            &[
                (AgentType::Ollama, true),
                (AgentType::LiteLlm, false),
                (AgentType::Nvidia, false),
            ],
        );

        for entry in &catalogue.workers {
            assert!(
                !entry.available || (entry.configured && entry.reachable),
                "available worker violates the catalogue invariant: {entry:#?}"
            );
        }

        let ollama = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::Ollama)
            .expect("Ollama entry");
        assert!(ollama.available);
        assert!(ollama.configured && ollama.reachable);
        assert_eq!(ollama.worker.kind, MessageTargetKind::DiscussionAgent);
        assert_eq!(ollama.worker.tier, Some(ModelTier::Default));
        assert!(ollama.tiers.iter().any(|tier| {
            tier.tier == ModelTier::Default && tier.resolved_model.as_deref() == Some("qwen3:8b")
        }));
        assert!(worker_static_refusal(&ollama.worker).is_none());

        let cli = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.cli_session_id == Some(77))
            .expect("exact joined CLI entry");
        assert!(cli.available);
        assert_eq!(cli.worker.kind, MessageTargetKind::Cli);
        assert_eq!(cli.label, "@codex-cli");
        assert_eq!(cli.declared_model.as_deref(), Some("gpt-5.6-sol"));
        assert!(cli.tiers.is_empty(), "joined CLIs ignore tier overrides");

        let nvidia = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::Nvidia)
            .expect("NVIDIA entry");
        assert!(!nvidia.available);
        assert!(!nvidia.configured);
        assert!(!nvidia.reachable);
        let codes = nvidia
            .reasons
            .iter()
            .map(|reason| reason.code.as_str())
            .collect::<Vec<_>>();
        assert!(codes.contains(&"auth_required"));
        assert!(codes.contains(&"model_not_configured"));
        assert!(codes.contains(&"endpoint_unreachable"));
        for reason in &nvidia.reasons {
            assert!(!reason.detail.contains("http://"));
            assert!(!reason.detail.contains("https://"));
        }
    }

    // ── KT-319 tranche 1 — delivery/review contracts + brief (pure). ──

    #[test]
    fn delivery_manifest_v1_accepts_a_complete_manifest() {
        let json = serde_json::json!({
            "version": "1", "task_ref": "KT-319", "head_sha": "abcdef1234567",
            "files_touched": [{ "path": "backend/src/x.rs", "kind": "modified" }],
            "tests": [{ "name": "cargo test --lib x", "status": "pass", "evidence": "exit 0" }],
            "dod_status": [{ "dod_id": "d1", "met": true, "evidence": "x.rs:1" }],
            "docs": [], "migrations": [], "risks": [], "limitations": [],
            "summary": "did the thing"
        })
        .to_string();
        let m = parse_delivery_manifest(&json).expect("a complete manifest must parse");
        assert_eq!(m.version, "1");
        assert_eq!(
            m.files_touched[0].kind,
            crate::models::FileChangeKind::Modified
        );
        assert_eq!(m.tests[0].status, crate::models::TestStatus::Pass);
        assert!(m.dod_status[0].met);
    }

    #[test]
    fn delivery_manifest_v1_rejects_a_missing_required_field() {
        // head_sha dropped — DoD-1 requires it and DoD-5 compares against it.
        // Differential: the error must NAME the missing field, not just "is_err".
        let json = serde_json::json!({
            "version": "1", "task_ref": "KT-319",
            "files_touched": [], "tests": [], "dod_status": [],
            "docs": [], "migrations": [], "risks": [], "limitations": [],
            "summary": "x"
        })
        .to_string();
        let err = parse_delivery_manifest(&json).unwrap_err().to_string();
        assert!(
            err.contains("head_sha"),
            "must name the missing field, got: {err}"
        );
    }

    #[test]
    fn delivery_manifest_v1_rejects_an_unknown_file_kind() {
        let json = serde_json::json!({
            "version": "1", "task_ref": "KT-319", "head_sha": "abcdef1234567",
            "files_touched": [{ "path": "x", "kind": "renamed" }],
            "tests": [], "dod_status": [],
            "docs": [], "migrations": [], "risks": [], "limitations": [], "summary": "x"
        })
        .to_string();
        assert!(
            parse_delivery_manifest(&json).is_err(),
            "an unknown file kind must be rejected"
        );
    }

    #[test]
    fn review_decision_v1_accepts_approve_without_comment() {
        let json =
            serde_json::json!({ "version": "1", "task_ref": "KT-319", "decision": "approve" })
                .to_string();
        let d = parse_review_decision(&json).expect("approve without comment is valid");
        assert_eq!(d.decision, crate::models::ReviewVerdict::Approve);
        assert!(d.findings.is_empty());
        assert!(d.reviewed_head_sha.is_none(), "legacy rows remain readable");
    }

    #[test]
    fn manifest_claims_require_exact_dod_coverage_and_evidence() {
        let dod = vec![PlanningDodItem {
            id: "d1".into(),
            sentence: "tests pass".into(),
            completed: false,
            position: 0,
        }];
        let complete = parse_delivery_manifest(&manifest_json("abcdef1234567")).unwrap();
        assert!(validate_manifest_claims(&dod, &complete).is_ok());

        let mut empty = complete.clone();
        empty.dod_status.clear();
        assert!(validate_manifest_claims(&dod, &empty)
            .unwrap_err()
            .contains("missing: d1"));

        let mut duplicate = complete.clone();
        duplicate.dod_status.push(duplicate.dod_status[0].clone());
        assert!(validate_manifest_claims(&dod, &duplicate)
            .unwrap_err()
            .contains("duplicate"));

        let mut unknown = complete.clone();
        unknown.dod_status[0].dod_id = "foreign".into();
        assert!(validate_manifest_claims(&dod, &unknown)
            .unwrap_err()
            .contains("unknown DoD id `foreign`"));

        let mut unproved = complete;
        unproved.dod_status[0].evidence = Some("  ".into());
        assert!(validate_manifest_claims(&dod, &unproved)
            .unwrap_err()
            .contains("has no non-empty evidence"));
    }

    #[test]
    fn review_decision_rejects_duplicate_or_blank_dod_evidence() {
        let duplicate = serde_json::json!({
            "version": "1", "task_ref": "KT-407", "decision": "approve",
            "reviewed_head_sha": "abcdef1234567",
            "dod_verifications": [
                { "dod_id": "d1", "met": true, "evidence": "exit 0" },
                { "dod_id": "d1", "met": true, "evidence": "another run" }
            ]
        })
        .to_string();
        assert!(parse_review_decision(&duplicate)
            .unwrap_err()
            .to_string()
            .contains("dupliquée"));

        let blank = serde_json::json!({
            "version": "1", "task_ref": "KT-407", "decision": "approve",
            "reviewed_head_sha": "abcdef1234567",
            "dod_verifications": [
                { "dod_id": "d1", "met": true, "evidence": "  " }
            ]
        })
        .to_string();
        assert!(parse_review_decision(&blank)
            .unwrap_err()
            .to_string()
            .contains("doit être non vide"));
    }

    #[test]
    fn review_decision_v1_rejects_request_changes_without_comment() {
        // The one rule the JSON subset cannot express (DoD-4): a change request
        // with no reason is not actionable.
        let json = serde_json::json!({
            "version": "1", "task_ref": "KT-319", "decision": "request_changes",
            "findings": [{ "issue": "bug here" }]
        })
        .to_string();
        let err = parse_review_decision(&json).unwrap_err().to_string();
        assert!(
            err.contains("comment"),
            "must name the missing comment, got: {err}"
        );
    }

    #[test]
    fn review_decision_v1_accepts_request_changes_with_comment_and_findings() {
        let json = serde_json::json!({
            "version": "1", "task_ref": "KT-319", "decision": "request_changes",
            "comment": "fix the drift check",
            "findings": [{ "path": "x.rs", "line": 12, "issue": "off by one" }]
        })
        .to_string();
        let d = parse_review_decision(&json).expect("a well-formed request_changes must parse");
        assert_eq!(d.decision, crate::models::ReviewVerdict::RequestChanges);
        assert_eq!(d.findings[0].line, Some(12));
    }

    #[test]
    fn delivery_manifest_v1_rejects_an_unknown_contract_version() {
        // Finding 🟠 2: a refusal that isn't tested doesn't exist. The version enum
        // is pinned by DELIVERY_CONTRACT_VERSION; a future "2" must be refused until
        // a v2 schema exists — differential on the `version` field, not `is_err`.
        let json = serde_json::json!({
            "version": "2", "task_ref": "KT-319", "head_sha": "abcdef1234567",
            "files_touched": [], "tests": [], "dod_status": [],
            "docs": [], "migrations": [], "risks": [], "limitations": [], "summary": "x"
        })
        .to_string();
        let err = parse_delivery_manifest(&json).unwrap_err().to_string();
        assert!(
            err.contains("version"),
            "must name the version field, got: {err}"
        );
        assert!(
            err.contains("not in allowed enum"),
            "must reject on the pinned version enum, got: {err}"
        );
    }

    #[test]
    fn review_decision_v1_rejects_an_unknown_contract_version() {
        // Same guard on the review contract: it pins REVIEW_CONTRACT_VERSION
        // separately, so its own literal must not drift unpinned either.
        let json =
            serde_json::json!({ "version": "2", "task_ref": "KT-319", "decision": "approve" })
                .to_string();
        let err = parse_review_decision(&json).unwrap_err().to_string();
        assert!(
            err.contains("version"),
            "must name the version field, got: {err}"
        );
        assert!(
            err.contains("not in allowed enum"),
            "must reject on the pinned version enum, got: {err}"
        );
    }

    #[test]
    fn worker_brief_names_the_delivery_contract_and_kt318_sections() {
        // KT-318 DoD-6: the brief carries objectif, DoD, décisions/scope,
        // constraints, tests, workspace AND the concrete delivery format — not a
        // prose "summarize your changes".
        let brief = worker_brief_markdown(
            "KT-319",
            "Titre de la tâche",
            "Objectif de la tâche",
            &[PlanningDodItem {
                id: "dod-1".into(),
                sentence: "Faire X".into(),
                completed: false,
                position: 0,
            }],
            "/wt/kt319",
            "kronn/task/KT-319",
            "abc1234",
            true,
            false,
            None,
        );
        for needle in [
            "## Objectif",
            "## Definition of Done",
            "## Décisions & périmètre",
            "## Méthode",
            "## Contraintes",
            "## Commence maintenant",
            "## Tests",
            "## Workspace",
            "## Format de livraison",
            "task_exec_deliver",
            "DeliveryManifest v1",
            "head_sha",
            "dod_status",
            "files_touched",
            "`dod-1` — Faire X",
        ] {
            assert!(brief.contains(needle), "brief must mention `{needle}`");
        }
        assert!(
            !brief.contains("résume les changements et signale"),
            "the old prose delivery instruction must be gone"
        );
        assert!(brief.contains("outils natifs de ton CLI"), "{brief}");
        assert!(!brief.contains("Premier appel : `search_text`"), "{brief}");
    }

    #[test]
    fn http_worker_brief_never_claims_shell_validation_capability() {
        let brief = worker_brief_markdown(
            "KT-407",
            "Sous-tâche Ollama",
            "Modifier une fonction bornée",
            &[],
            "/wt/kt407",
            "kronn/task/KT-407",
            "abc1234",
            false,
            true,
            None,
        );
        assert!(brief.contains("Tu n'as pas de shell"), "{brief}");
        assert!(brief.contains("`status: skipped`"), "{brief}");
        assert!(brief.contains("Le principal exécutera"), "{brief}");
        assert!(brief.contains("Premier appel : `search_text`"), "{brief}");
        assert!(brief.contains("`edit_lines`"), "{brief}");
        assert!(brief.contains("appelle `git_commit`"), "{brief}");
        assert!(!brief.contains("`task_exec_commit`"), "{brief}");
        assert!(
            brief.contains("exactement un `{ met, evidence }`"),
            "{brief}"
        );
        assert!(brief.contains("Kronn injecte lui-même"), "{brief}");
        assert!(!brief.contains("`head_sha` : le HEAD exact"), "{brief}");
        assert!(!brief.contains("`dod_id, met, evidence`"), "{brief}");
        assert!(
            !brief.contains("Exécute les commandes de validation"),
            "{brief}"
        );
    }

    #[test]
    fn spawned_host_cli_brief_keeps_shell_method_but_projects_delivery() {
        let brief = worker_brief_markdown(
            "KT-436",
            "Sous-tâche Claude Code spawnée",
            "Modifier une fonction bornée",
            &[PlanningDodItem {
                id: "opaque-dod-id".into(),
                sentence: "Faire X".into(),
                completed: false,
                position: 0,
            }],
            "/wt/kt436",
            "kronn/task/KT-436",
            "abc1234",
            true,
            true,
            None,
        );
        assert!(brief.contains("outils natifs de ton CLI"), "{brief}");
        assert!(
            brief.contains("Exécute les commandes de validation"),
            "{brief}"
        );
        assert!(
            brief.contains("exactement un `{ met, evidence }`"),
            "{brief}"
        );
        assert!(brief.contains("1. [ ] Faire X"), "{brief}");
        assert!(brief.contains("appelle `task_exec_commit`"), "{brief}");
        assert!(brief.contains("N'utilise pas `git commit`"), "{brief}");
        assert!(!brief.contains("opaque-dod-id"), "{brief}");
        assert!(!brief.contains("`head_sha` : le HEAD exact"), "{brief}");
    }

    #[test]
    fn prelocalized_http_worker_brief_forbids_exploration_and_names_exact_target() {
        let scope = TaskWorkerScope::PrelocalizedEdit {
            path: "backend/src/lib.rs".into(),
            start_line: 40,
            end_line: 44,
        };
        let brief = worker_brief_markdown(
            "KT-435",
            "Sous-tâche Ollama prélocalisée",
            "Modifier uniquement la cible fournie",
            &[],
            "/wt/kt435",
            "kronn/task/KT-435",
            "abc1234",
            false,
            true,
            Some(&scope),
        );
        for needle in [
            "Cible mécanique prélocalisée",
            "`backend/src/lib.rs`",
            "`40..=44`",
            "un seul `read_file` contraint",
            "retire définitivement les outils de lecture",
            "Ne cherche pas ailleurs",
        ] {
            assert!(
                brief.contains(needle),
                "brief must mention `{needle}`\n{brief}"
            );
        }
        assert!(!brief.contains("Premier appel : `search_text`"), "{brief}");
        assert!(!brief.contains("## Workspace"), "{brief}");
        assert!(!brief.contains("## Décisions & périmètre"), "{brief}");
        assert!(
            brief.len() < 2_500,
            "specialized brief is too large: {}",
            brief.len()
        );
    }

    #[test]
    fn http_turn_usage_sums_rework_and_exposes_peak_and_phase_breakdown() {
        let event = |id: &str, dispatch: &str, turns: serde_json::Value| {
            crate::models::TaskExecutionEvent {
                id: id.into(),
                task_execution_id: "exec-1".into(),
                action: "http_turn_telemetry".into(),
                from_status: None,
                to_status: None,
                actor_kind: crate::models::PlanningActorKind::Backend,
                actor_id: Some("http-agent-runner".into()),
                actor_session_id: Some(dispatch.into()),
                changes: serde_json::json!({"version": 1, "turns": turns}),
                source_message_id: None,
                created_at: chrono::Utc::now(),
            }
        };
        let turn =
            |number: u32, provider: &str, phase: &str, prompt: u64, eval: u64, duration: u64| {
                serde_json::json!({
                    "turn": number,
                    "provider": provider,
                    "phase": phase,
                    "prompt_tokens": prompt,
                    "eval_tokens": eval,
                    "duration_ms": duration,
                    "provider_ok": true,
                    "requested_tools": [],
                    "executed_tools": []
                })
            };
        let events = vec![
            event(
                "e1",
                "dispatch-1",
                serde_json::json!([
                    turn(1, "ollama", "read", 100, 10, 1_000),
                    turn(2, "ollama", "mutation", 200, 20, 2_000)
                ]),
            ),
            event(
                "e2",
                "dispatch-2",
                serde_json::json!([turn(1, "nvidia", "delivery", 300, 30, 3_000)]),
            ),
        ];

        let usage = summarize_http_turn_usage(&events).unwrap();
        assert_eq!(usage.turns, 3);
        assert_eq!(usage.prompt_tokens, 600);
        assert_eq!(usage.eval_tokens, 60);
        assert_eq!(usage.traffic_tokens, 660);
        assert_eq!(usage.peak_context_tokens, 300);
        assert_eq!(usage.duration_ms, 6_000);
        assert_eq!(usage.phases.len(), 3);
        assert_eq!(
            usage.recent_turns[0].dispatch_id.as_deref(),
            Some("dispatch-1")
        );
        assert_eq!(
            usage.recent_turns[2].dispatch_id.as_deref(),
            Some("dispatch-2")
        );
    }

    // ── HTTP mapping (KT-328 tranche 2, commit 3) — pure, no AppState needed. ──

    #[test]
    fn accept_outcome_fuses_unknown_and_wrong_acceptor_and_keeps_others_distinct() {
        // Anti-oracle: an unknown offer and a wrong caller MUST be indistinguishable, so
        // a non-target session cannot enumerate live offer ids or probe target identity.
        let unknown = accept_outcome_to_response(AcceptAttachOutcome::NotFound);
        let wrong = accept_outcome_to_response(AcceptAttachOutcome::WrongAcceptor);
        assert!(!unknown.success && !wrong.success);
        assert_eq!(unknown.error_code.as_deref(), Some("not_found"));
        assert_eq!(wrong.error_code.as_deref(), Some("not_found"));
        assert_eq!(
            unknown.error, wrong.error,
            "the two refusals must be byte-identical (no oracle)"
        );
        assert_eq!(
            unknown.error.as_deref(),
            Some("offer not found or not addressed to this session")
        );

        // Legitimate, actionable outcomes stay distinct + informative (each only reachable
        // AFTER caller == target, so naming the real state is not an oracle).
        let busy = accept_outcome_to_response(AcceptAttachOutcome::NotAcceptable {
            status: crate::models::WorkerOfferStatus::Accepted,
        });
        assert_eq!(busy.error_code.as_deref(), Some("conflict"));
        assert!(busy
            .error
            .as_deref()
            .unwrap()
            .contains(crate::models::WorkerOfferStatus::Accepted.as_str()));

        let binding = accept_outcome_to_response(AcceptAttachOutcome::BindingMismatch);
        assert_eq!(binding.error_code.as_deref(), Some("conflict"));
        let binding_error = binding.error.as_deref().unwrap();
        assert!(binding_error.contains("durably bound"));
        assert!(binding_error.contains("retry"));
        assert!(!binding_error.contains("discussion"));

        let expired = accept_outcome_to_response(AcceptAttachOutcome::Expired);
        assert_eq!(expired.error_code.as_deref(), Some("conflict"));
        assert!(expired.error.as_deref().unwrap().contains("expired"));

        let refused =
            accept_outcome_to_response(AcceptAttachOutcome::CheckpointRefused("task raced".into()));
        assert_eq!(refused.error_code.as_deref(), Some("conflict"));
        assert!(refused.error.as_deref().unwrap().contains("resumable"));
    }

    #[test]
    fn provision_error_maps_to_stable_codes() {
        assert_eq!(
            provision_error_to_response(ProvisionError::TaskNotFound)
                .error_code
                .as_deref(),
            Some("not_found")
        );
        assert_eq!(
            provision_error_to_response(ProvisionError::NotLaunchable("nope".into()))
                .error_code
                .as_deref(),
            Some("validation")
        );
        let failed = provision_error_to_response(ProvisionError::WorkspaceFailed {
            reason: "disk full".into(),
            compensated: true,
        });
        assert_eq!(failed.error_code.as_deref(), Some("conflict"));
        assert!(failed
            .error
            .as_deref()
            .unwrap()
            .contains("compensated=true"));
        assert_eq!(
            provision_error_to_response(ProvisionError::Internal("boom".into()))
                .error_code
                .as_deref(),
            Some("internal")
        );
    }

    fn git(repo: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap()
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::Builder::new()
            .prefix("kronn-t3-")
            .tempdir()
            .unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "t@t.com"]);
        git(dir.path(), &["config", "user.name", "T"]);
        std::fs::write(dir.path().join("README.md"), "# t").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "init"]);
        dir
    }

    fn git_rev(repo: &Path, rev: &str) -> String {
        let out = git(repo, &["rev-parse", rev]);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn test_actor() -> PlanningActor {
        PlanningActor {
            kind: PlanningActorKind::Backend,
            id: Some("test".into()),
            session_id: None,
            source_message_id: None,
        }
    }

    fn native_worker() -> MessageTarget {
        MessageTarget::agent(AgentType::ClaudeCode)
    }

    fn test_project(id: &str, path: &str) -> Project {
        let now = chrono::Utc::now();
        Project {
            id: id.into(),
            name: "proj".into(),
            path: path.into(),
            repo_url: None,
            token_override: None,
            ai_config: AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: AiAuditStatus::NoTemplate,
            ai_todo_count: 0,
            tech_debt_count: 0,
            needs_docs_migration: false,
            path_exists: true,
            default_skill_ids: vec![],
            default_profile_id: None,
            briefing_notes: None,
            linked_repos: vec![],
            created_at: now,
            updated_at: now,
        }
    }

    fn plain_discussion(id: &str, project_id: &str) -> Discussion {
        let now = chrono::Utc::now();
        Discussion {
            awaiting_agent: false,
            agent_running: false,
            id: id.into(),
            project_id: Some(project_id.into()),
            title: "Principal".into(),
            agent: AgentType::ClaudeCode,
            language: "fr".into(),
            participants: vec![AgentType::ClaudeCode],
            messages: vec![],
            message_count: 0,
            non_system_message_count: 0,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            tier: ModelTier::default(),
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
            workflow_run_id: None,
            test_mode_restore_branch: None,
            test_mode_stash_ref: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn create_todo_task(
        conn: &rusqlite::Connection,
        project_id: &str,
        title: &str,
    ) -> crate::models::PlanningTaskDetail {
        crate::db::planning::create_task(
            conn,
            &CreatePlanningTaskRequest {
                title: title.into(),
                discussion_id: None,
                idempotency_key: None,
                description: "Implémenter le module.".into(),
                status: PlanningTaskStatus::Todo,
                priority: Default::default(),
                parent_id: None,
                project_ids: vec![project_id.into()],
                tags: vec![],
                definition_of_done: vec![CreatePlanningDodItem {
                    id: None,
                    sentence: "Le module compile et les tests passent.".into(),
                    completed: false,
                }],
                links: vec![],
                actor: test_actor(),
            },
        )
        .unwrap()
    }

    /// Seed a project pointing at `repo`, a parent discussion, and one Todo task.
    /// Returns `(task_reference, parent_discussion_id, project_id)`.
    async fn seed(db: &Database, repo: &Path) -> (String, String, String) {
        let project_id = "proj-1".to_string();
        let parent_id = "parent-1".to_string();
        let proj = test_project(&project_id, &repo.to_string_lossy());
        let parent = plain_discussion(&parent_id, &project_id);
        let pid = project_id.clone();
        let reference = db
            .with_conn(move |conn| {
                crate::db::projects::insert_project(conn, &proj)?;
                crate::db::discussions::insert_discussion(conn, &parent)?;
                Ok(create_todo_task(conn, &pid, "Faire la chose")
                    .summary
                    .reference)
            })
            .await
            .unwrap();
        (reference, parent_id, project_id)
    }

    async fn count(db: &Database, sql: &'static str) -> i64 {
        db.with_conn(move |conn| Ok(conn.query_row(sql, [], |r| r.get::<_, i64>(0))?))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn provisions_native_task_end_to_end() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _pid) = seed(&db, repo.path()).await;
        let base_sha = git_rev(repo.path(), "HEAD");

        let exec = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref.clone(),
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("run-1".into()),
            },
        )
        .await
        .expect("provision should succeed");

        assert_eq!(exec.status, TaskExecutionStatus::Working);
        assert!(exec.sub_discussion_id.is_some(), "sub-disc linked");
        assert!(exec.workspace_id.is_some(), "workspace linked");
        assert!(exec.dispatch_job_id.is_some(), "dispatch attached");
        assert_eq!(exec.attempt_no, 0);

        // Task flipped to InProgress — the LAST checkpoint (DoD-8).
        let tref = task_ref.clone();
        let task = db
            .with_conn(move |conn| crate::db::planning::get_task(conn, &tref))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(task.summary.status, PlanningTaskStatus::InProgress);

        // Worktree exists at the pinned SHA (DoD-2/8 — pinned before branch).
        let (wt, _branch) =
            worktree::task_worktree_layout(repo.path(), &task_ref, exec_short(&exec.id)).unwrap();
        assert!(wt.exists(), "child worktree should exist");
        worktree::verify_worktree_head(&wt, &base_sha).expect("worktree HEAD == pinned base");

        // Breadcrumb queryable in one lineage read (DoD-4).
        let eid = exec.id.clone();
        let lineage = db
            .with_conn(move |conn| crate::db::orchestration::get_execution_lineage(conn, &eid))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(lineage.sub_discussion_id, exec.sub_discussion_id);
        assert!(lineage.workspace_canonical_path.is_some());
        assert_eq!(lineage.task_reference, task_ref);

        let child_id = exec.sub_discussion_id.clone().unwrap();
        let child = db
            .with_conn(move |conn| {
                crate::db::discussions::get_discussion(conn, &child_id)?
                    .context("provisioned worker room missing")
            })
            .await
            .unwrap();
        assert!(
            child.pin_first_message,
            "the worker brief must survive prompt truncation on every resume"
        );

        // The frontend detail projection resolves durable state, DoD, metrics
        // and the current (still empty) semantic attempt in one read.
        let eid = exec.id.clone();
        let detail = db
            .with_conn(move |conn| execution_detail(conn, &eid))
            .await
            .unwrap();
        assert_eq!(detail.target_branch.as_deref(), Some("main"));
        assert_eq!(detail.definition_of_done.len(), 1);
        assert_eq!(detail.attempts.len(), 1);
        assert_eq!(detail.attempts[0].attempt_no, 0);
        assert!(detail.attempts[0].delivery.is_none());
        assert_eq!(detail.usage.cli_traffic_tokens, None);
        assert!(!detail.usage.in_app_cost_is_partial);

        let links = db
            .with_conn(crate::db::orchestration::list_execution_discussion_links)
            .await
            .unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].execution_id, exec.id);
        assert_eq!(links[0].parent_discussion_id, exec.parent_discussion_id);
        assert_eq!(
            Some(links[0].sub_discussion_id.as_str()),
            exec.sub_discussion_id.as_deref()
        );

        // Exactly one brief + one dispatch became visible on commit.
        assert_eq!(count(&db, "SELECT COUNT(*) FROM messages").await, 1);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM agent_dispatch_jobs").await,
            1
        );
    }

    /// KT-410 — the CLI bridge already threaded `ValidationSpec`, but the native
    /// MCP catalogue neither declared it nor read it, so `task_exec_launch`
    /// silently provisioned an ungated run. Exercises the real
    /// `KronnToolExecutor` path (not `provision_single_task_execution_with_validations`
    /// directly) so a regression in the catalogue/handler wiring itself fails
    /// this test, not just the lower-level function.
    #[tokio::test]
    async fn native_principal_launch_persists_exact_validations_via_mcp_catalogue() {
        use crate::agents::tools::ToolCall;

        let repo = init_repo();
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let (task_ref, parent_id, _pid) = seed(&db, repo.path()).await;
        {
            let task_ref = task_ref.clone();
            let parent_id = parent_id.clone();
            db.with_conn(move |conn| {
                crate::db::planning::link_discussion(
                    conn,
                    &task_ref,
                    &crate::models::LinkPlanningDiscussionRequest {
                        discussion_id: parent_id,
                        placement: Default::default(),
                        is_primary: true,
                        position: None,
                        actor: test_actor(),
                    },
                )
            })
            .await
            .unwrap();
        }
        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let exec = crate::api::agent_tools::KronnToolExecutor::arc(
            state,
            Some(parent_id),
            AgentType::ClaudeCode,
            None,
            None,
        );

        let launch_tool = exec
            .catalogue()
            .into_iter()
            .find(|tool| tool["function"]["name"] == "task_exec_launch")
            .expect("task_exec_launch must be in the native catalogue");
        let items = &launch_tool["function"]["parameters"]["properties"]["validations"]["items"];
        assert_eq!(
            items["properties"]["command"]["minLength"], 1,
            "the published schema must match the enforced non-empty command: {launch_tool}"
        );
        assert_eq!(
            items["properties"]["timeout_secs"]["minimum"], 1,
            "the published schema must match the enforced positive timeout: {launch_tool}"
        );
        assert_eq!(
            items["required"],
            serde_json::json!(["command"]),
            "the published schema must mark command required: {launch_tool}"
        );
        assert_eq!(
            items["additionalProperties"], false,
            "the published schema must reject unknown fields, matching ValidationSpec's deny_unknown_fields: {launch_tool}"
        );

        let validations = serde_json::json!([
            {"command": "cargo build", "timeout_secs": 120},
            {"command": "cargo test", "quick_exec_id": "qe-1"}
        ]);
        let call = ToolCall {
            id: "c1".into(),
            name: "task_exec_launch".into(),
            arguments: serde_json::json!({
                "task_reference": task_ref,
                "worker": serde_json::to_value(native_worker()).unwrap(),
                "worker_scope_intent": "generic",
                "base_rev": "main",
                "idempotency_key": "kt-410-mcp-launch",
                "validations": validations,
            }),
        };
        let outcome = exec.execute(&call).await;
        assert!(outcome.ok, "launch failed: {:?}", outcome.content);
        let run_id = outcome.content["orchestration_run_id"]
            .as_str()
            .expect("execution must carry its run id")
            .to_string();
        let run = db
            .with_conn(move |conn| crate::db::orchestration::get_orchestration_run(conn, &run_id))
            .await
            .unwrap()
            .expect("orchestration run must exist");
        assert_eq!(
            serde_json::to_value(&run.validations).unwrap(),
            validations,
            "the exact principal validations must be persisted, not a subset or []"
        );
    }

    #[tokio::test]
    async fn native_principal_launch_persists_and_dispatches_exact_prelocalized_scope() {
        use crate::agents::tools::{ToolCall, ToolRunMode};

        let repo = init_repo();
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let (task_ref, parent_id, _pid) = seed(&db, repo.path()).await;
        {
            let task_ref = task_ref.clone();
            let parent_id = parent_id.clone();
            db.with_conn(move |conn| {
                crate::db::planning::link_discussion(
                    conn,
                    &task_ref,
                    &crate::models::LinkPlanningDiscussionRequest {
                        discussion_id: parent_id,
                        placement: Default::default(),
                        is_primary: true,
                        position: None,
                        actor: test_actor(),
                    },
                )
            })
            .await
            .unwrap();
        }
        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let principal = crate::api::agent_tools::KronnToolExecutor::arc(
            state.clone(),
            Some(parent_id),
            AgentType::ClaudeCode,
            None,
            None,
        );
        let scope = TaskWorkerScope::PrelocalizedEdit {
            path: "README.md".into(),
            start_line: 1,
            end_line: 1,
        };
        let launch_tool = principal
            .catalogue()
            .into_iter()
            .find(|tool| tool["function"]["name"] == "task_exec_launch")
            .expect("task_exec_launch schema");
        assert_eq!(
            launch_tool["function"]["parameters"]["properties"]["worker_scope"]["properties"]
                ["mode"]["enum"],
            serde_json::json!(["prelocalized_edit", "prelocalized_insert_after"])
        );

        let outcome = principal
            .execute(&ToolCall {
                id: "scope-launch".into(),
                name: "task_exec_launch".into(),
                arguments: serde_json::json!({
                    "task_reference": task_ref,
                    "worker": serde_json::to_value(
                        MessageTarget::discussion_agent(AgentType::Ollama)
                    ).unwrap(),
                    "worker_scope_intent": "scoped",
                    "base_rev": "main",
                    "idempotency_key": "kt-435-native-scope",
                    "worker_scope": serde_json::to_value(&scope).unwrap(),
                }),
            })
            .await;
        assert!(outcome.ok, "scope launch failed: {:?}", outcome.content);
        let execution: TaskExecution = serde_json::from_value(outcome.content).unwrap();
        assert_eq!(execution.worker_scope, Some(scope.clone()));
        let child_id = execution.sub_discussion_id.expect("worker child room");

        let worker = crate::api::discussions::streaming::native_http_tools_for_discussion(
            &state,
            &child_id,
            &AgentType::Ollama,
            None,
            execution.dispatch_job_id,
            false,
        )
        .await
        .unwrap()
        .expect("native worker executor");
        assert_eq!(worker.run_mode(), ToolRunMode::Worker);
        assert_eq!(worker.worker_scope(), Some(scope));
    }

    /// KT-410 — a present-and-malformed `validations` payload must be refused
    /// explicitly, never silently coerced to an empty (ungated) list.
    #[tokio::test]
    async fn native_principal_launch_refuses_malformed_validations_without_silent_fallback() {
        use crate::agents::tools::ToolCall;

        let repo = init_repo();
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let (task_ref, parent_id, _pid) = seed(&db, repo.path()).await;
        {
            let task_ref = task_ref.clone();
            let parent_id = parent_id.clone();
            db.with_conn(move |conn| {
                crate::db::planning::link_discussion(
                    conn,
                    &task_ref,
                    &crate::models::LinkPlanningDiscussionRequest {
                        discussion_id: parent_id,
                        placement: Default::default(),
                        is_primary: true,
                        position: None,
                        actor: test_actor(),
                    },
                )
            })
            .await
            .unwrap();
        }
        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let exec = crate::api::agent_tools::KronnToolExecutor::arc(
            state,
            Some(parent_id),
            AgentType::ClaudeCode,
            None,
            None,
        );

        // Each case is malformed for a DIFFERENT reason; a shared fixture that
        // mixed them (e.g. an unknown field on an item missing `command`)
        // would pass even if only one of the two checks actually worked.
        let cases: &[(&str, serde_json::Value)] = &[
            (
                "missing required command",
                serde_json::json!([{"quick_exec_id": "qe-1"}]),
            ),
            (
                "valid command plus an unknown field",
                serde_json::json!([{"command": "cargo build", "unexpected_field": true}]),
            ),
            ("explicit null instead of an array", serde_json::Value::Null),
            (
                "structurally valid but semantically empty command",
                serde_json::json!([{"command": "   "}]),
            ),
            (
                "structurally valid but zero timeout",
                serde_json::json!([{"command": "cargo build", "timeout_secs": 0}]),
            ),
        ];
        for (label, validations) in cases {
            let call = ToolCall {
                id: "c1".into(),
                name: "task_exec_launch".into(),
                arguments: serde_json::json!({
                    "task_reference": task_ref,
                    "worker": serde_json::to_value(native_worker()).unwrap(),
                    "worker_scope_intent": "generic",
                    "base_rev": "main",
                    "idempotency_key": format!("kt-410-mcp-launch-malformed-{label}"),
                    "validations": validations,
                }),
            };
            let outcome = exec.execute(&call).await;
            assert!(
                !outcome.ok,
                "{label} must be refused, not silently accepted: {:?}",
                outcome.content
            );
        }
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM orchestration_runs").await,
            0,
            "no refused launch may provision an ungated run behind its error"
        );
    }

    #[tokio::test]
    async fn observability_is_bounded_correlated_and_redacted() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _project_id) = seed(&db, repo.path()).await;
        let exec = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("observability".into()),
            },
        )
        .await
        .unwrap();

        let exec_id = exec.id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "DELETE FROM task_execution_events WHERE task_execution_id = ?1",
                [&exec_id],
            )?;
            conn.execute(
                "UPDATE task_executions SET status = 'Failed', review_rounds = 2, \
                        attempt_no = 2, created_at = '2026-08-21T00:00:00Z', \
                        updated_at = '2026-08-21T00:00:10Z', \
                        finished_at = '2026-08-21T00:00:10Z' WHERE id = ?1",
                [&exec_id],
            )?;
            let transitions = [
                (
                    "Pending",
                    "Provisioning",
                    "2026-08-21T00:00:01Z",
                    serde_json::json!({}),
                ),
                (
                    "Provisioning",
                    "Blocked",
                    "2026-08-21T00:00:02Z",
                    serde_json::json!({
                        "code": "awaiting_worker_acceptance",
                        "reason": "super-secret-agent-prose"
                    }),
                ),
                (
                    "Blocked",
                    "Provisioning",
                    "2026-08-21T00:00:04Z",
                    serde_json::json!({}),
                ),
                (
                    "Provisioning",
                    "Working",
                    "2026-08-21T00:00:05Z",
                    serde_json::json!({}),
                ),
                (
                    "Working",
                    "AwaitingReview",
                    "2026-08-21T00:00:07Z",
                    serde_json::json!({}),
                ),
                (
                    "AwaitingReview",
                    "Escalated",
                    "2026-08-21T00:00:09Z",
                    serde_json::json!({ "timeout_kind": "review" }),
                ),
                (
                    "Escalated",
                    "Failed",
                    "2026-08-21T00:00:10Z",
                    serde_json::json!({}),
                ),
            ];
            for (index, (from, to, at, changes)) in transitions.into_iter().enumerate() {
                conn.execute(
                    "INSERT INTO task_execution_events \
                     (id, task_execution_id, action, from_status, to_status, actor_kind, \
                      changes_json, created_at) VALUES (?1, ?2, 'transition', ?3, ?4, \
                      'backend', ?5, ?6)",
                    rusqlite::params![
                        format!("event-{index}"),
                        exec_id,
                        from,
                        to,
                        changes.to_string(),
                        at
                    ],
                )?;
            }
            crate::db::orchestration::record_validation_run(
                conn,
                &exec_id,
                Some("candidate"),
                &ValidationSpec {
                    command: "test".into(),
                    quick_exec_id: None,
                    timeout_secs: None,
                },
                Some(1),
                Some(50),
                Some("super-secret-validation-output"),
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let exec_id = exec.id.clone();
        let view = db
            .with_conn(move |conn| execution_observability(conn, &exec_id))
            .await
            .unwrap();
        assert_eq!(view.lineage.execution.id, exec.id);
        assert_eq!(view.metrics.usage.duration_ms, 10_000);
        assert_eq!(view.metrics.waiting_duration_ms, 6_000);
        assert_eq!(view.metrics.review_rounds, 2);
        assert_eq!(view.metrics.attempt_count, 3);
        assert_eq!(view.metrics.validation_failures, 1);
        assert_eq!(view.audit_events.len(), 7);
        assert_eq!(
            view.metrics
                .state_durations
                .iter()
                .find(|metric| metric.status == TaskExecutionStatus::Provisioning)
                .map(|metric| metric.duration_ms),
            Some(2_000)
        );
        assert!(view
            .metrics
            .blocking_reasons
            .iter()
            .any(|metric| { metric.code == "awaiting_worker_acceptance" && metric.count == 1 }));
        assert!(view
            .metrics
            .blocking_reasons
            .iter()
            .any(|metric| metric.code == "timeout_review" && metric.count == 1));

        let public_json = serde_json::to_string(&view).unwrap();
        assert!(!public_json.contains("super-secret-agent-prose"));
        assert!(!public_json.contains("super-secret-validation-output"));
        assert!(!public_json.contains("changes"));
    }

    /// The whole saga on a real repo: the child does work, the parent moves on, and
    /// the integration has to reconcile the two without ever rewriting the parent.
    #[tokio::test]
    async fn integrates_an_approved_execution_by_fast_forward() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, project_id) = seed(&db, repo.path()).await;

        let exec = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref.clone(),
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("integ-1".into()),
            },
        )
        .await
        .expect("provision should succeed");

        // KT-321 will pin the target at launch; until then the engine is handed one.
        let rid = exec.orchestration_run_id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE orchestration_runs SET target_branch = 'main' WHERE id = ?1",
                rusqlite::params![rid],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // The worker commits in its own worktree.
        let (child, branch) =
            worktree::task_worktree_layout(repo.path(), &task_ref, exec_short(&exec.id)).unwrap();
        std::fs::write(child.join("worker.txt"), "done").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-m", "worker work"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&child)
                .output()
                .unwrap();
        }
        let candidate_before = git_rev(&child, "HEAD");

        // Meanwhile the parent moves on, so this is a real reconciliation.
        std::fs::write(repo.path().join("parent.txt"), "moved").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-m", "parent moves"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(repo.path())
                .output()
                .unwrap();
        }
        let parent_tip = git_rev(repo.path(), "HEAD");

        let eid = exec.id.clone();
        let delivered = candidate_before.clone();
        db.with_conn(move |conn| {
            crate::db::worker_deliveries::upsert_delivery(conn, &eid, 0, &delivered, "{}")?;
            for to in [
                TaskExecutionStatus::AwaitingReview,
                TaskExecutionStatus::Approved,
            ] {
                crate::db::orchestration::transition_execution(
                    conn,
                    &eid,
                    to,
                    &backend_actor(),
                    serde_json::json!({}),
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

        let outcome = run_integration(&db, &exec.id)
            .await
            .expect("integration runs");
        let IntegrationOutcome::Integrated { sha } = outcome else {
            panic!("expected the integration to land, got {outcome:?}");
        };

        // The parent advanced onto the candidate, and only forward.
        assert_eq!(git_rev(repo.path(), "HEAD"), sha);
        assert_ne!(sha, parent_tip, "the parent must have moved");
        assert_ne!(
            sha, candidate_before,
            "the candidate absorbed the parent tip"
        );
        let contains = std::process::Command::new("git")
            .args(["merge-base", "--is-ancestor", &parent_tip, &sha])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(
            contains.status.success(),
            "the parent's own work must survive"
        );
        assert!(repo.path().join("parent.txt").exists());
        assert!(repo.path().join("worker.txt").exists());

        // The checkpoints describe what happened, and the backup ref points back.
        let eid = exec.id.clone();
        let (target, merge, integrated, backup) = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT candidate_target_sha, candidate_merge_sha, integrated_sha, backup_ref \
                       FROM task_executions WHERE id = ?1",
                    rusqlite::params![eid],
                    |r| {
                        Ok((
                            r.get::<_, Option<String>>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, Option<String>>(3)?,
                        ))
                    },
                )?)
            })
            .await
            .unwrap();
        assert_eq!(target.as_deref(), Some(parent_tip.as_str()));
        assert_eq!(merge.as_deref(), Some(sha.as_str()));
        assert_eq!(integrated.as_deref(), Some(sha.as_str()));
        let backup = backup.expect("a backup ref must have been armed");
        assert_eq!(
            worktree::resolve_commit(repo.path(), &backup).unwrap(),
            parent_tip
        );

        // The terminal checkpoint is cross-aggregate: execution + integrated SHA
        // and the plan task's Done status commit together.
        let (eid, tref) = (exec.id.clone(), task_ref.clone());
        let (terminal, task_status, retained_workspace, child_workspace) = db
            .with_conn(move |conn| {
                let terminal = crate::db::orchestration::get_task_execution(conn, &eid)?
                    .expect("execution survives terminal cleanup");
                let task_status = crate::db::planning::get_task(conn, &tref)?
                    .expect("task survives integration")
                    .summary
                    .status;
                let retained_workspace: (
                    String,
                    Option<String>,
                    Option<String>,
                    String,
                    Option<String>,
                ) = conn.query_row(
                    "SELECT state, workspace_path, canonical_path, branch, head_sha
                       FROM discussion_workspaces WHERE task_execution_id = ?1",
                    [&eid],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )?;
                let child_workspace: (Option<String>, Option<String>) = conn.query_row(
                    "SELECT workspace_path, worktree_branch FROM discussions WHERE id = ?1",
                    [terminal.sub_discussion_id.as_deref().unwrap()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                Ok((terminal, task_status, retained_workspace, child_workspace))
            })
            .await
            .unwrap();
        assert_eq!(terminal.status, TaskExecutionStatus::Done);
        assert_eq!(terminal.integrated_sha.as_deref(), Some(sha.as_str()));
        assert!(
            terminal.workspace_id.is_some(),
            "workspace provenance survives cleanup"
        );
        assert_eq!(task_status, PlanningTaskStatus::Done);
        assert_eq!(
            retained_workspace,
            (
                "detached".to_string(),
                Some(child.to_string_lossy().to_string()),
                None,
                branch.clone(),
                Some(candidate_before.clone()),
            ),
            "managed ownership evidence survives physical cleanup without claiming the removed path"
        );
        assert_eq!(
            child_workspace,
            (None, None),
            "child no longer points at a removed checkout"
        );
        assert!(!child.exists(), "integrated child worktree was removed");
        assert!(
            worktree::resolve_commit(repo.path(), &branch).is_err(),
            "integrated task branch was removed with its worktree"
        );

        // Crash-window replay: model the terminal DB checkpoint having landed
        // while its managed checkout/intent still exists. A Done replay must
        // finish cleanup instead of returning early forever.
        let recreated =
            worktree::create_task_worktree(repo.path(), &task_ref, exec_short(&exec.id), &sha)
                .unwrap();
        let eid = exec.id.clone();
        let child_id = exec.sub_discussion_id.clone().unwrap();
        let parent = exec.parent_discussion_id.clone();
        let task_id = exec.task_id.clone();
        let pid = project_id.clone();
        let path = recreated.path.clone();
        let branch_for_db = recreated.branch.clone();
        let integrated_for_db = sha.clone();
        db.with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let ws = crate::db::discussion_workspaces::upsert_managed(
                &tx,
                &eid,
                &child_id,
                &parent,
                Some(&task_id),
                &pid,
                &path,
                &path,
                &branch_for_db,
                &integrated_for_db,
                &integrated_for_db,
            )?;
            crate::db::orchestration::set_execution_workspace(
                &tx,
                &eid,
                &ws.id,
                &integrated_for_db,
                &branch_for_db,
            )?;
            crate::db::discussions::update_discussion_workspace(
                &tx,
                &child_id,
                &path,
                &branch_for_db,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
        let replay = run_integration(&db, &exec.id).await.unwrap();
        assert!(matches!(
            replay,
            IntegrationOutcome::NotIntegrable {
                status: TaskExecutionStatus::Done
            }
        ));
        assert!(!std::path::Path::new(&recreated.path).exists());
        let eid = exec.id.clone();
        let replayed_workspace = db
            .with_conn(move |conn| {
                crate::db::discussion_workspaces::get_managed_for_execution(conn, &eid)
            })
            .await
            .unwrap()
            .expect("Done replay keeps durable provenance");
        assert_eq!(replayed_workspace.state, "detached");
        assert_eq!(replayed_workspace.canonical_path, None);
    }

    async fn approved_execution_with_commit(
        db: &Database,
        repo: &Path,
        key: &str,
        file: &str,
        content: &str,
        validations: Vec<ValidationSpec>,
    ) -> (TaskExecution, String, std::path::PathBuf) {
        let (task_ref, parent_id, _project_id) = seed(db, repo).await;
        let execution = provision_single_task_execution(
            db,
            ProvisionInput {
                task_reference: task_ref.clone(),
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some(key.into()),
            },
        )
        .await
        .unwrap();
        let run_id = execution.orchestration_run_id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE orchestration_runs SET target_branch = 'main', validation_json = ?2 \
                 WHERE id = ?1",
                rusqlite::params![run_id, serde_json::to_string(&validations)?],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let (child, _) =
            worktree::task_worktree_layout(repo, &task_ref, exec_short(&execution.id)).unwrap();
        std::fs::write(child.join(file), content).unwrap();
        for args in [vec!["add", "."], vec!["commit", "-m", "worker work"]] {
            assert!(git(&child, &args).status.success());
        }
        let execution_id = execution.id.clone();
        db.with_conn(move |conn| {
            for to in [
                TaskExecutionStatus::AwaitingReview,
                TaskExecutionStatus::Approved,
            ] {
                crate::db::orchestration::transition_execution(
                    conn,
                    &execution_id,
                    to,
                    &backend_actor(),
                    serde_json::json!({}),
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
        (execution, task_ref, child)
    }

    #[tokio::test]
    async fn dirty_apply_block_resumes_after_the_parent_is_cleaned() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (execution, _task_ref, child) = approved_execution_with_commit(
            &db,
            repo.path(),
            "dirty-apply-resume",
            "worker.txt",
            "done",
            vec![],
        )
        .await;
        let target_sha = git_rev(repo.path(), "main");
        checkpoint(
            &db,
            &execution.id,
            CheckpointStep::Anchored(target_sha.clone()),
        )
        .await
        .unwrap();
        let merge_sha = match worktree::build_candidate(&child, &target_sha).unwrap() {
            worktree::CandidateOutcome::Built { sha } => sha,
            other => panic!("expected a candidate, got {other:?}"),
        };
        checkpoint(&db, &execution.id, CheckpointStep::Built(merge_sha.clone()))
            .await
            .unwrap();
        checkpoint(&db, &execution.id, CheckpointStep::Validating)
            .await
            .unwrap();
        let backup = worktree::write_backup_ref(
            repo.path(),
            &format!("{}-{}", execution.task_id, exec_short(&execution.id)),
            &target_sha,
        )
        .unwrap();
        checkpoint(&db, &execution.id, CheckpointStep::Armed(backup))
            .await
            .unwrap();

        let execution_id = execution.id.clone();
        let (applying, run) = db
            .with_conn(move |conn| {
                let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                    .expect("execution");
                let run = crate::db::orchestration::get_orchestration_run(
                    conn,
                    &execution.orchestration_run_id,
                )?
                .expect("run");
                Ok((execution, run))
            })
            .await
            .unwrap();
        assert_eq!(applying.status, TaskExecutionStatus::Applying);

        let dirty_path = repo.path().join("uncommitted.txt");
        std::fs::write(&dirty_path, "keep me").unwrap();
        let blocked = finish_recovered_apply(
            &db,
            &applying,
            &run,
            repo.path(),
            child.to_str().unwrap(),
            true,
        )
        .await
        .unwrap();
        assert!(matches!(blocked, IntegrationOutcome::Refused { .. }));
        let execution_id = execution.id.clone();
        let parked = db
            .with_conn(move |conn| {
                crate::db::orchestration::get_task_execution(conn, &execution_id)
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(parked.status, TaskExecutionStatus::Blocked);
        assert_eq!(
            parked.blocked_from_status,
            Some(TaskExecutionStatus::Applying)
        );

        // Reproduce the two-stage restart that stranded the real 0.11.0
        // pilot: a Blocked(Applying) row is interrupted while an older
        // ResumeProvisioning decision is still pending. Classification must
        // replace that stale decision from the nested durable checkpoint.
        let execution_id = execution.id.clone();
        db.with_conn(move |conn| {
            crate::db::orchestration::transition_execution(
                conn,
                &execution_id,
                TaskExecutionStatus::Interrupted,
                &backend_actor(),
                serde_json::json!({ "reason": "test_restart" }),
            )?;
            let interrupted = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                .context("interrupted execution")?;
            let run = crate::db::orchestration::get_orchestration_run(
                conn,
                &interrupted.orchestration_run_id,
            )?
            .context("orchestration run")?;
            crate::db::orchestration::set_execution_recovery(
                conn,
                &interrupted,
                &run,
                ExecutionRecoveryAction::ResumeProvisioning,
                "stale decision from an earlier boot",
            )?;
            Ok(())
        })
        .await
        .unwrap();
        classify_interrupted_execution(&db, &execution.id, &[AgentType::ClaudeCode])
            .await
            .unwrap();
        let execution_id = execution.id.clone();
        let (interrupted, refreshed) = db
            .with_conn(move |conn| {
                Ok((
                    crate::db::orchestration::get_task_execution(conn, &execution_id)?
                        .context("interrupted execution")?,
                    crate::db::orchestration::get_execution_recovery(conn, &execution_id)?
                        .context("refreshed recovery")?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(
            interrupted.interrupted_from_status,
            Some(TaskExecutionStatus::Blocked)
        );
        assert_eq!(
            refreshed.recovery_action,
            ExecutionRecoveryAction::BlockDirtyTarget,
            "the nested Applying origin wins over the stale provisioning decision"
        );
        let execution_id = execution.id.clone();
        db.with_conn(move |conn| {
            let restored = crate::db::orchestration::transition_execution(
                conn,
                &execution_id,
                TaskExecutionStatus::Blocked,
                &backend_actor(),
                serde_json::json!({ "recovery": "restore_applying_block" }),
            )?;
            assert!(restored);
            Ok(())
        })
        .await
        .unwrap();

        assert!(git(repo.path(), &["add", "uncommitted.txt"])
            .status
            .success());
        assert!(git(
            repo.path(),
            &["commit", "-m", "parent advanced while blocked"]
        )
        .status
        .success());
        let advanced_target = git_rev(repo.path(), "HEAD");
        let resumed = resume_blocked_apply(&db, &execution.id).await.unwrap();
        let IntegrationOutcome::Integrated { sha: resumed_sha } = resumed else {
            panic!("expected rebuilt integration");
        };
        assert_ne!(resumed_sha, merge_sha, "the stale candidate was rebuilt");
        let execution_id = execution.id.clone();
        let done = db
            .with_conn(move |conn| {
                crate::db::orchestration::get_task_execution(conn, &execution_id)
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(done.status, TaskExecutionStatus::Done);
        assert_eq!(
            done.candidate_target_sha.as_deref(),
            Some(advanced_target.as_str())
        );
        assert_eq!(done.integrated_sha.as_deref(), Some(resumed_sha.as_str()));
        assert_eq!(git_rev(repo.path(), "HEAD"), resumed_sha);
        assert!(dirty_path.exists(), "the committed parent change survives");
        assert!(repo.path().join("worker.txt").exists());
    }

    #[tokio::test]
    async fn integration_conflict_returns_to_worker_without_losing_lineage() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (execution, task_ref, child) = approved_execution_with_commit(
            &db,
            repo.path(),
            "conflict-saga",
            "README.md",
            "worker version",
            vec![],
        )
        .await;
        std::fs::write(repo.path().join("README.md"), "parent version").unwrap();
        assert!(git(repo.path(), &["add", "."]).status.success());
        assert!(git(repo.path(), &["commit", "-m", "parent conflict"])
            .status
            .success());

        let outcome = run_integration(&db, &execution.id).await.unwrap();
        assert!(matches!(
            outcome,
            IntegrationOutcome::SentBack { ref reason } if reason.contains("conflict")
        ));
        let execution_id = execution.id.clone();
        let after = db
            .with_conn(move |conn| {
                crate::db::orchestration::get_task_execution(conn, &execution_id)
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, TaskExecutionStatus::ChangesRequested);
        assert_eq!(after.sub_discussion_id, execution.sub_discussion_id);
        assert_eq!(after.workspace_id, execution.workspace_id);
        assert!(after.integrated_sha.is_none());
        assert!(
            child.exists(),
            "the worker checkout remains available for repair"
        );
        assert!(git(&child, &["status", "--porcelain"]).stdout.is_empty());
        let task = db
            .with_conn(move |conn| crate::db::planning::get_task(conn, &task_ref))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(task.summary.status, PlanningTaskStatus::InProgress);
    }

    #[tokio::test]
    async fn failed_validation_returns_to_worker_and_records_the_exact_candidate() {
        let repo = init_repo();
        let parent_before = git_rev(repo.path(), "HEAD");
        let db = Database::open_in_memory().unwrap();
        let validation = ValidationSpec {
            command: "false".into(),
            quick_exec_id: None,
            timeout_secs: Some(5),
        };
        let (execution, _task_ref, child) = approved_execution_with_commit(
            &db,
            repo.path(),
            "validation-saga",
            "worker.txt",
            "done",
            vec![validation.clone()],
        )
        .await;

        let outcome = run_integration(&db, &execution.id).await.unwrap();
        let IntegrationOutcome::SentBack { reason } = outcome else {
            panic!("expected failed validation to return work, got {outcome:?}");
        };
        assert!(reason.contains("validation failed"), "{reason}");
        let execution_id = execution.id.clone();
        let (after, validations) = db
            .with_conn(move |conn| {
                Ok((
                    crate::db::orchestration::get_task_execution(conn, &execution_id)?
                        .expect("execution"),
                    crate::db::orchestration::list_validation_runs(conn, &execution_id)?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(after.status, TaskExecutionStatus::ChangesRequested);
        assert!(after.integrated_sha.is_none());
        assert_eq!(validations.len(), 1);
        assert_eq!(validations[0].command, validation.command);
        assert_eq!(
            validations[0].candidate_merge_sha, after.candidate_merge_sha,
            "the failure belongs to the candidate that was actually tested"
        );
        assert!(!validations[0].passed());
        assert_eq!(git_rev(repo.path(), "HEAD"), parent_before);
        assert!(
            child.exists(),
            "failed validation preserves the worker checkout"
        );
    }

    /// An unpinned target is the case where the engine would have to guess which
    /// history to advance. It refuses instead.
    #[tokio::test]
    async fn recovered_apply_refuses_when_the_target_branch_disappears() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _project_id) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("recovery-missing-target".into()),
            },
        )
        .await
        .unwrap();
        let target_sha = git_rev(repo.path(), "main");
        let child_path = {
            let id = execution.id.clone();
            db.with_conn(move |conn| {
                crate::db::discussion_workspaces::get_managed_for_execution(conn, &id)?
                    .and_then(|workspace| workspace.canonical_path)
                    .context("managed recovery worktree has no path")
            })
            .await
            .unwrap()
        };
        let merge_sha = git_rev(Path::new(&child_path), "HEAD");
        let id = execution.id.clone();
        let run_id = execution.orchestration_run_id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE orchestration_runs SET target_branch = 'main' WHERE id = ?1",
                rusqlite::params![run_id],
            )?;
            for status in [
                TaskExecutionStatus::AwaitingReview,
                TaskExecutionStatus::Approved,
                TaskExecutionStatus::Integrating,
                TaskExecutionStatus::Validating,
                TaskExecutionStatus::Applying,
                TaskExecutionStatus::Interrupted,
            ] {
                crate::db::orchestration::transition_execution(
                    conn,
                    &id,
                    status,
                    &backend_actor(),
                    serde_json::json!({}),
                )?;
            }
            conn.execute(
                "UPDATE task_executions SET candidate_target_sha = ?2, \
                        candidate_merge_sha = ?3 WHERE id = ?1",
                rusqlite::params![&id, &target_sha, &merge_sha],
            )?;
            let execution = crate::db::orchestration::get_task_execution(conn, &id)?
                .context("execution exists")?;
            let run = crate::db::orchestration::get_orchestration_run(
                conn,
                &execution.orchestration_run_id,
            )?
            .context("run exists")?;
            crate::db::orchestration::set_execution_recovery(
                conn,
                &execution,
                &run,
                ExecutionRecoveryAction::ApplyFastForward,
                "candidate was ready before crash",
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let deleted = Command::new("git")
            .args(["update-ref", "-d", "refs/heads/main"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(deleted.status.success());
        assert!(worktree::resolve_commit(repo.path(), "main").is_err());

        let result = resume_recovered_integration(
            &db,
            &execution.id,
            ExecutionRecoveryAction::ApplyFastForward,
        )
        .await;
        assert!(
            result.is_err(),
            "a missing target branch must not apply Git"
        );
        let id = execution.id.clone();
        let (status, recovery) = db
            .with_conn(move |conn| {
                Ok((
                    crate::db::orchestration::get_task_execution(conn, &id)?
                        .context("execution remains inspectable")?
                        .status,
                    crate::db::orchestration::get_execution_recovery(conn, &id)?
                        .context("recovery decision remains inspectable")?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(status, TaskExecutionStatus::Interrupted);
        assert!(recovery.pending);
        assert_eq!(
            recovery.recovery_action,
            ExecutionRecoveryAction::AwaitHuman
        );
    }

    #[tokio::test]
    async fn refuses_to_integrate_without_a_pinned_target() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _pid) = seed(&db, repo.path()).await;

        let exec = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("integ-2".into()),
            },
        )
        .await
        .unwrap();

        let eid = exec.id.clone();
        let run_id = exec.orchestration_run_id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE orchestration_runs SET target_branch = NULL WHERE id = ?1",
                rusqlite::params![run_id],
            )?;
            for to in [
                TaskExecutionStatus::AwaitingReview,
                TaskExecutionStatus::Approved,
            ] {
                crate::db::orchestration::transition_execution(
                    conn,
                    &eid,
                    to,
                    &backend_actor(),
                    serde_json::json!({}),
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

        let outcome = run_integration(&db, &exec.id).await.unwrap();
        assert!(
            matches!(outcome, IntegrationOutcome::Refused { .. }),
            "got {outcome:?}"
        );
        // Nothing was pinned, so nothing was attempted.
        let eid = exec.id.clone();
        let anchor: Option<String> = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT candidate_target_sha FROM task_executions WHERE id = ?1",
                    rusqlite::params![eid],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(anchor, None);
    }

    #[tokio::test]
    async fn refuses_a_non_todo_task() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _pid) = seed(&db, repo.path()).await;
        // Flip the task out of Todo (resolve its id first, never guess a column).
        let tref = task_ref.clone();
        db.with_conn(move |conn| {
            let id = crate::db::planning::get_task(conn, &tref)?
                .unwrap()
                .summary
                .id;
            conn.execute(
                "UPDATE planning_tasks SET status='in_progress' WHERE id=?1",
                [id],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let err = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("run-x".into()),
            },
        )
        .await
        .expect_err("a non-Todo task must be refused");
        assert!(matches!(err, ProvisionError::NotLaunchable(_)), "{err:?}");
        // Nothing created.
        assert_eq!(count(&db, "SELECT COUNT(*) FROM task_executions").await, 0);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM messages").await, 0);
    }

    #[tokio::test]
    async fn refuses_a_task_without_a_project() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        // A task with NO project.
        let parent_id = "parent-1".to_string();
        let parent = plain_discussion(&parent_id, "proj-1");
        let proj = test_project("proj-1", &repo.path().to_string_lossy());
        let task_ref = db
            .with_conn(move |conn| {
                crate::db::projects::insert_project(conn, &proj)?;
                crate::db::discussions::insert_discussion(conn, &parent)?;
                let detail = crate::db::planning::create_task(
                    conn,
                    &CreatePlanningTaskRequest {
                        title: "No project".into(),
                        discussion_id: None,
                        idempotency_key: None,
                        description: "x".into(),
                        status: PlanningTaskStatus::Todo,
                        priority: Default::default(),
                        parent_id: None,
                        project_ids: vec![],
                        tags: vec![],
                        definition_of_done: vec![CreatePlanningDodItem {
                            id: None,
                            sentence: "done".into(),
                            completed: false,
                        }],
                        links: vec![],
                        actor: test_actor(),
                    },
                )?;
                Ok(detail.summary.reference)
            })
            .await
            .unwrap();

        let err = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: None,
            },
        )
        .await
        .expect_err("a project-less task must be refused");
        assert!(matches!(err, ProvisionError::NotLaunchable(_)), "{err:?}");
    }

    /// Seed a joined, active CLI session (ClaudeCode) in `disc_id` with an explicit
    /// PK so a `MessageTarget::cli(_, pk)` worker resolves to a real session row.
    async fn seed_cli_session(db: &Database, id: i64, disc_id: &str, session_id: &str) {
        let disc = disc_id.to_string();
        let sid = session_id.to_string();
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO discussion_sessions \
                 (id, disc_id, agent_type, session_id, role, status, joined_at) \
                 VALUES (?1, ?2, 'ClaudeCode', ?3, 'peer', 'active', ?4)",
                rusqlite::params![id, disc, sid, chrono::Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn shared_launch_boundary_rejects_missing_or_mismatched_cli_before_side_effects() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _pid) = seed(&db, repo.path()).await;

        let missing = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref.clone(),
                parent_discussion_id: parent_id.clone(),
                worker: MessageTarget::cli(AgentType::ClaudeCode, 404),
                base_rev: Some("main".into()),
                idempotency_key: Some("missing-cli".into()),
            },
        )
        .await
        .expect_err("an unknown exact CLI session must fail closed");
        assert!(
            matches!(
                &missing,
                ProvisionError::NotLaunchable(reason) if reason.contains("worker_unavailable")
            ),
            "{missing:?}"
        );

        seed_cli_session(&db, 101, &parent_id, "claude-session").await;
        let mismatched = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref.clone(),
                parent_discussion_id: parent_id,
                worker: MessageTarget::cli(AgentType::Codex, 101),
                base_rev: Some("main".into()),
                idempotency_key: Some("provider-mismatch".into()),
            },
        )
        .await
        .expect_err("a session from another provider must never be substituted");
        assert!(
            matches!(
                &mismatched,
                ProvisionError::NotLaunchable(reason) if reason.contains("worker_unavailable")
            ),
            "{mismatched:?}"
        );

        let reference = task_ref.clone();
        let (execution_count, task_status) = db
            .with_conn(move |conn| {
                let execution_count =
                    conn.query_row("SELECT COUNT(*) FROM task_executions", [], |row| {
                        row.get::<_, i64>(0)
                    })?;
                let task = crate::db::planning::get_task(conn, &reference)?
                    .context("task remains inspectable")?;
                Ok((execution_count, task.summary.status))
            })
            .await
            .unwrap();
        assert_eq!(execution_count, 0, "no durable execution was created");
        assert_eq!(task_status, PlanningTaskStatus::Todo);
        assert!(
            !repo.path().join(".kronn/worktrees").exists(),
            "no managed worktree directory was created"
        );
    }

    #[tokio::test]
    async fn cli_worker_opens_a_control_offer_and_parks_blocked() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _pid) = seed(&db, repo.path()).await;
        seed_cli_session(&db, 101, &parent_id, "sess-a").await;

        let exec = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref.clone(),
                parent_discussion_id: parent_id.clone(),
                worker: MessageTarget::cli(AgentType::ClaudeCode, 101),
                base_rev: Some("main".into()),
                idempotency_key: Some("cli-run".into()),
            },
        )
        .await
        .expect("a CLI worker provisions A–D then parks awaiting acceptance");

        // Parked Blocked(awaiting_worker_acceptance), NOT Working; task stays Todo.
        assert_eq!(exec.status, TaskExecutionStatus::Blocked);
        assert!(
            exec.blocked_reason
                .as_deref()
                .unwrap_or_default()
                .contains("awaiting_worker_acceptance"),
            "reason = {:?}",
            exec.blocked_reason
        );
        // The hold is discriminated by structured CODE, not prose (KT-334 branches
        // on this): the awaiting-acceptance park is the NORMAL case.
        assert_eq!(
            exec.blocked_reason_code,
            Some(crate::models::BlockedReasonCode::AwaitingWorkerAcceptance),
        );
        assert!(
            exec.dispatch_job_id.is_none(),
            "no native dispatch for a CLI worker"
        );
        // A–D still ran: sub-disc + workspace are provisioned.
        assert!(exec.sub_discussion_id.is_some(), "sub-disc provisioned");
        assert!(exec.workspace_id.is_some(), "workspace provisioned");

        let tref = task_ref.clone();
        let task = db
            .with_conn(move |conn| crate::db::planning::get_task(conn, &tref))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            task.summary.status,
            PlanningTaskStatus::Todo,
            "task stays Todo until acceptance (DoD-8)"
        );

        // Exactly one live offer for this attempt, targeting the session, wired to a
        // control message as provenance.
        let eid = exec.id.clone();
        let offer = db
            .with_conn(move |conn| {
                crate::db::worker_offers::get_active_offer_for_attempt(conn, &eid, 0)
            })
            .await
            .unwrap()
            .expect("a pending offer exists for the attempt");
        assert_eq!(offer.target_cli_session_id, 101);
        assert_eq!(offer.origin_discussion_id, parent_id);
        assert_eq!(
            offer.child_discussion_id,
            exec.sub_discussion_id.clone().unwrap()
        );
        assert!(matches!(
            offer.status,
            crate::models::WorkerOfferStatus::Pending
        ));
        let msg_id = offer
            .offer_message_id
            .clone()
            .expect("provenance message wired");

        // The control message lives in the ORIGIN room, targeted to the EXACT session;
        // no native dispatch was enqueued, and it is the only message.
        let mid = msg_id.clone();
        let (disc_of_msg, targets) = db
            .with_conn(move |conn| {
                let disc: String = conn.query_row(
                    "SELECT discussion_id FROM messages WHERE id = ?1",
                    [&mid],
                    |r| r.get(0),
                )?;
                let targets = crate::db::discussions::list_message_targets(conn, &mid)?;
                Ok((disc, targets))
            })
            .await
            .unwrap();
        assert_eq!(
            disc_of_msg, parent_id,
            "control offer posted in the ORIGIN room"
        );
        assert_eq!(targets.len(), 1, "exactly one typed target");
        assert_eq!(targets[0].kind, MessageTargetKind::Cli);
        assert_eq!(targets[0].cli_session_id, Some(101));
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM agent_dispatch_jobs").await,
            0
        );
        assert_eq!(count(&db, "SELECT COUNT(*) FROM messages").await, 1);
    }

    #[tokio::test]
    async fn cli_worker_session_busy_parks_blocked_without_a_second_offer() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_a, parent_id, project_id) = seed(&db, repo.path()).await;
        let pid = project_id.clone();
        let task_b = db
            .with_conn(move |conn| {
                Ok(create_todo_task(conn, &pid, "Autre tâche")
                    .summary
                    .reference)
            })
            .await
            .unwrap();
        seed_cli_session(&db, 101, &parent_id, "sess-a").await;

        let mk = |task: String, key: &str| ProvisionInput {
            task_reference: task,
            parent_discussion_id: parent_id.clone(),
            worker: MessageTarget::cli(AgentType::ClaudeCode, 101),
            base_rev: Some("main".into()),
            idempotency_key: Some(key.to_string()),
        };

        // A takes the session → its offer is live (normal awaiting-acceptance park).
        let ea = provision_single_task_execution(&db, mk(task_a, "a"))
            .await
            .unwrap();
        assert_eq!(ea.status, TaskExecutionStatus::Blocked);
        assert_eq!(
            ea.blocked_reason_code,
            Some(crate::models::BlockedReasonCode::AwaitingWorkerAcceptance),
        );

        // B targets the SAME session → SessionCommittedElsewhere → Blocked naming A,
        // with NO second offer and NO extra control message (never a Failed).
        let eb = provision_single_task_execution(&db, mk(task_b, "b"))
            .await
            .unwrap();
        assert_eq!(
            eb.status,
            TaskExecutionStatus::Blocked,
            "B parks Blocked, not Failed"
        );
        assert!(
            eb.blocked_reason
                .as_deref()
                .unwrap_or_default()
                .contains(ea.id.as_str()),
            "reason names the holder {}, got {:?}",
            ea.id,
            eb.blocked_reason
        );
        // Differential proof: the two Blocked states carry DISTINCT codes — this one
        // needs a human decision (re-offer / native), NOT prose-matching (KT-334).
        assert_eq!(
            eb.blocked_reason_code,
            Some(crate::models::BlockedReasonCode::WorkerSessionCommittedElsewhere),
        );
        assert!(eb.dispatch_job_id.is_none());

        // Only ONE offer on the session (A's); B opened none.
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM task_execution_worker_offers").await,
            1,
            "no second offer for B"
        );
        let ebid = eb.id.clone();
        let for_b = db
            .with_conn(move |conn| crate::db::worker_offers::list_offers_for_execution(conn, &ebid))
            .await
            .unwrap();
        assert!(for_b.is_empty(), "B has no offer of its own");
        // One control message total (A's); B posted none.
        assert_eq!(count(&db, "SELECT COUNT(*) FROM messages").await, 1);
    }

    /// Seed a parked CLI worker + a durable origin binding, returning the ids the
    /// acceptance flow needs. Mirrors what a real `disc_join` + provisioning leave behind.
    async fn parked_cli_worker(db: &Database, repo: &Path) -> (String, String, String, String) {
        let (task_ref, parent_id, _pid) = seed(db, repo).await;
        seed_cli_session(db, 101, &parent_id, "sess-a").await;
        // A real disc_join binds the session to the origin room durably; the acceptance
        // transfer needs that expected source to move from.
        {
            let p = parent_id.clone();
            db.with_conn(move |conn| {
                crate::db::disc_source::bind_to_source(conn, &p, "ClaudeCode", "sess-a")
            })
            .await
            .unwrap();
        }
        let exec = provision_single_task_execution(
            db,
            ProvisionInput {
                task_reference: task_ref.clone(),
                parent_discussion_id: parent_id.clone(),
                worker: MessageTarget::cli(AgentType::ClaudeCode, 101),
                base_rev: Some("main".into()),
                idempotency_key: Some("cli-run".into()),
            },
        )
        .await
        .unwrap();
        let child_id = exec.sub_discussion_id.clone().unwrap();
        let eid = exec.id.clone();
        let offer_id = db
            .with_conn(move |conn| {
                crate::db::worker_offers::get_active_offer_for_attempt(conn, &eid, 0)
            })
            .await
            .unwrap()
            .unwrap()
            .id;
        (task_ref, parent_id, child_id, offer_id)
    }

    /// Full CLI handshake (KT-328 tranche 2): a parked worker accepts its offer → the
    /// session moves to the child, the brief lands there targeted (no dispatch), the
    /// execution is Working, the task InProgress, the offer accepted, and the origin
    /// carries a durable attach notice.
    #[tokio::test]
    async fn cli_worker_accepts_and_attaches_end_to_end() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, _parent_id, child_id, offer_id) = parked_cli_worker(&db, repo.path()).await;

        // A real MCP reload rotates the active bridge identity while preserving
        // the durable room binding. The accept boundary must authorize the
        // exact LIVE row and move the separate DURABLE binding.
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE discussion_sessions SET session_id = 'live-sess-a' WHERE id = 101",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // Before acceptance: the child has NO brief yet (DoD-3).
        let c = child_id.clone();
        let before = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM messages WHERE discussion_id = ?1",
                    [&c],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(before, 0, "no brief in the child before acceptance");

        let outcome =
            accept_worker_offer_and_attach(&db, &offer_id, "ClaudeCode", "live-sess-a", "sess-a")
                .await
                .unwrap();
        let (attached_child, exec_id) = match outcome {
            AcceptAttachOutcome::Attached {
                child_discussion_id,
                execution,
            } => {
                assert_eq!(
                    execution.status,
                    TaskExecutionStatus::Working,
                    "execution is Working after acceptance"
                );
                assert_eq!(
                    execution.blocked_reason, None,
                    "a resumed execution cannot expose the former acceptance hold as active"
                );
                assert_eq!(execution.blocked_reason_code, None);
                (child_discussion_id, execution.id)
            }
            other => panic!("expected Attached, got {other:?}"),
        };
        assert_eq!(attached_child, child_id);

        // Task flipped Todo → InProgress (the sole anti-race authority).
        let tref = task_ref.clone();
        let task = db
            .with_conn(move |conn| crate::db::planning::get_task(conn, &tref))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(task.summary.status, PlanningTaskStatus::InProgress);

        // Offer settled `accepted`; session moved to the child (binding + membership).
        let (offer, bound_disc, session_disc) = {
            let oid = offer_id.clone();
            db.with_conn(move |conn| {
                let offer = crate::db::worker_offers::get_worker_offer(conn, &oid)?.unwrap();
                let bound = crate::db::disc_source::find_disc_by_source_session(
                    conn,
                    "ClaudeCode",
                    "sess-a",
                )?;
                let sdisc: String = conn.query_row(
                    "SELECT disc_id FROM discussion_sessions WHERE id = 101",
                    [],
                    |r| r.get(0),
                )?;
                Ok((offer, bound, sdisc))
            })
            .await
            .unwrap()
        };
        assert_eq!(offer.status, crate::models::WorkerOfferStatus::Accepted);
        assert_eq!(
            bound_disc.as_deref(),
            Some(child_id.as_str()),
            "durable binding moved to the child"
        );
        assert_eq!(
            session_disc, child_id,
            "session membership moved to the child"
        );

        // The work brief is in the CHILD (the only message there), Cli-targeted, no dispatch.
        let c = child_id.clone();
        let (child_msg_count, targets, dispatches) = db
            .with_conn(move |conn| {
                let n: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM messages WHERE discussion_id = ?1",
                    [&c],
                    |r| r.get(0),
                )?;
                let brief_id: String = conn.query_row(
                    "SELECT id FROM messages WHERE discussion_id = ?1 LIMIT 1",
                    [&c],
                    |r| r.get(0),
                )?;
                let targets = crate::db::discussions::list_message_targets(conn, &brief_id)?;
                let d: i64 =
                    conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |r| r.get(0))?;
                Ok((n, targets, d))
            })
            .await
            .unwrap();
        assert_eq!(child_msg_count, 1, "exactly the brief in the child");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, MessageTargetKind::Cli);
        assert_eq!(targets[0].cli_session_id, Some(101));
        assert_eq!(dispatches, 0, "a CLI worker enqueues zero native dispatch");

        // The origin carries a durable attach notice naming the child (never silent).
        let notice = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT content FROM messages WHERE id = ?1",
                    [format!("orch-attach:{exec_id}:0")],
                    |r| r.get::<_, String>(0),
                )?)
            })
            .await
            .unwrap();
        assert!(
            notice.contains(&child_id),
            "attach notice names the child room, got {notice:?}"
        );
    }

    /// KT-425 — reproduce the real interruption boundary: phase 1 committed
    /// `pending → accepting`, but neither durable binding nor live session moved and the
    /// final checkpoint never ran. The exact target retry must resume the idempotent saga;
    /// another same-provider session remains opaque and cannot steal it.
    #[tokio::test]
    async fn accepting_offer_resumes_after_crash_before_transfer() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, child_id, offer_id) = parked_cli_worker(&db, repo.path()).await;
        seed_cli_session(&db, 102, &parent_id, "sess-b").await;

        let oid = offer_id.clone();
        db.with_conn(move |conn| {
            assert!(crate::db::worker_offers::transition_offer_status(
                conn,
                &oid,
                crate::models::WorkerOfferStatus::Pending,
                crate::models::WorkerOfferStatus::Accepting,
                None,
            )?);
            Ok(())
        })
        .await
        .unwrap();

        let wrong =
            accept_worker_offer_and_attach(&db, &offer_id, "ClaudeCode", "sess-b", "sess-a")
                .await
                .unwrap();
        assert!(
            matches!(wrong, AcceptAttachOutcome::WrongAcceptor),
            "same-provider non-target must not resume an accepting offer: {wrong:?}"
        );

        let resumed =
            accept_worker_offer_and_attach(&db, &offer_id, "ClaudeCode", "sess-a", "sess-a")
                .await
                .unwrap();
        let execution = match resumed {
            AcceptAttachOutcome::Attached {
                child_discussion_id,
                execution,
            } => {
                assert_eq!(child_discussion_id, child_id);
                execution
            }
            other => panic!("accepting retry must converge to Attached, got {other:?}"),
        };
        assert_eq!(execution.status, TaskExecutionStatus::Working);

        let oid = offer_id.clone();
        let tref = task_ref.clone();
        let (offer_status, session_disc, task_status) = db
            .with_conn(move |conn| {
                let offer = crate::db::worker_offers::get_worker_offer(conn, &oid)?
                    .context("offer vanished after resumed checkpoint")?;
                let session_disc: String = conn.query_row(
                    "SELECT disc_id FROM discussion_sessions WHERE id = 101",
                    [],
                    |row| row.get(0),
                )?;
                let task = crate::db::planning::get_task(conn, &tref)?
                    .context("task vanished after resumed checkpoint")?;
                Ok((offer.status, session_disc, task.summary.status))
            })
            .await
            .unwrap();
        assert_eq!(offer_status, crate::models::WorkerOfferStatus::Accepted);
        assert_eq!(session_disc, child_id);
        assert_eq!(task_status, PlanningTaskStatus::InProgress);
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM messages WHERE id LIKE 'orch-brief:%'"
            )
            .await,
            1,
            "resumed checkpoint posts one brief"
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM messages WHERE id LIKE 'orch-attach:%'"
            )
            .await,
            1,
            "resumed checkpoint posts one origin notice"
        );
    }

    /// A crash between the accept and the checkpoint resumes: a second acceptance by the
    /// same session converges to Attached and never double-posts the brief or notice.
    #[tokio::test]
    async fn accept_and_attach_is_idempotent_on_resume() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_task_ref, _parent_id, _child_id, offer_id) =
            parked_cli_worker(&db, repo.path()).await;

        let first =
            accept_worker_offer_and_attach(&db, &offer_id, "ClaudeCode", "sess-a", "sess-a")
                .await
                .unwrap();
        assert!(matches!(first, AcceptAttachOutcome::Attached { .. }));

        // Replay (same session, same offer) — idempotent: still Attached, nothing doubled.
        let again =
            accept_worker_offer_and_attach(&db, &offer_id, "ClaudeCode", "sess-a", "sess-a")
                .await
                .unwrap();
        assert!(
            matches!(again, AcceptAttachOutcome::Attached { .. }),
            "resume converges to Attached, got {again:?}"
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM messages WHERE id LIKE 'orch-brief:%'"
            )
            .await,
            1,
            "exactly one brief after a resumed acceptance"
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM messages WHERE id LIKE 'orch-attach:%'"
            )
            .await,
            1,
            "exactly one attach notice after a resumed acceptance"
        );
    }

    /// A different session (SAME provider) cannot accept another session's offer — typed
    /// WrongAcceptor, and nothing moves.
    #[tokio::test]
    async fn a_wrong_session_cannot_accept_and_nothing_moves() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_task_ref, parent_id, child_id, offer_id) = parked_cli_worker(&db, repo.path()).await;
        // A second joined ClaudeCode session in the origin room — same provider, NOT the
        // offer's target.
        seed_cli_session(&db, 102, &parent_id, "sess-b").await;

        // Even naming the target's real durable binding cannot compensate for
        // a different live session: the exact target-PK check runs first.
        let outcome =
            accept_worker_offer_and_attach(&db, &offer_id, "ClaudeCode", "sess-b", "sess-a")
                .await
                .unwrap();
        assert!(
            matches!(outcome, AcceptAttachOutcome::WrongAcceptor),
            "a non-target same-provider session is refused, got {outcome:?}"
        );

        // Nothing moved: offer still pending, target session still in origin, child empty.
        let c = child_id.clone();
        let (offer_status, session_disc, child_msgs) = db
            .with_conn(move |conn| {
                let status: String = conn.query_row(
                    "SELECT status FROM task_execution_worker_offers WHERE id = ?1",
                    [&offer_id],
                    |r| r.get(0),
                )?;
                let sdisc: String = conn.query_row(
                    "SELECT disc_id FROM discussion_sessions WHERE id = 101",
                    [],
                    |r| r.get(0),
                )?;
                let n: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM messages WHERE discussion_id = ?1",
                    [&c],
                    |r| r.get(0),
                )?;
                Ok((status, sdisc, n))
            })
            .await
            .unwrap();
        assert_eq!(offer_status, "pending", "offer untouched");
        assert_eq!(
            session_disc, parent_id,
            "target session still in the origin room"
        );
        assert_eq!(child_msgs, 0, "no brief posted in the child");
    }

    #[tokio::test]
    async fn idempotent_relaunch_returns_the_same_execution() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _pid) = seed(&db, repo.path()).await;

        let mk = |key: &str| ProvisionInput {
            task_reference: task_ref.clone(),
            parent_discussion_id: parent_id.clone(),
            worker: native_worker(),
            base_rev: Some("main".into()),
            idempotency_key: Some(key.to_string()),
        };
        let first = provision_single_task_execution(&db, mk("run-1"))
            .await
            .unwrap();
        let second = provision_single_task_execution(&db, mk("run-1"))
            .await
            .unwrap();

        assert_eq!(first.id, second.id, "same key returns the same execution");
        // No duplication: one execution, one brief, one dispatch.
        assert_eq!(count(&db, "SELECT COUNT(*) FROM task_executions").await, 1);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM messages").await, 1);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM agent_dispatch_jobs").await,
            1
        );
    }

    #[tokio::test]
    async fn refuses_an_immutable_revision_as_the_integration_target() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _pid) = seed(&db, repo.path()).await;
        let pinned = git_rev(repo.path(), "HEAD");
        // The branch moves AFTER we decide to build on `pinned`.
        std::fs::write(repo.path().join("later.txt"), "later").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "second"]);
        let moved = git_rev(repo.path(), "main");
        assert_ne!(pinned, moved);

        let error = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref.clone(),
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some(pinned.clone()),
                idempotency_key: None,
            },
        )
        .await
        .expect_err("a SHA cannot be the parent integration target");

        assert!(matches!(
            error,
            ProvisionError::NotLaunchable(ref reason)
                if reason.contains("not a local branch")
        ));
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM task_executions").await,
            0,
            "the refusal happens before a durable execution is created"
        );
        assert_eq!(git_rev(repo.path(), "main"), moved);
    }

    #[tokio::test]
    async fn distinct_tasks_get_distinct_sub_discussions() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_a, parent_id, project_id) = seed(&db, repo.path()).await;
        let pid = project_id.clone();
        let task_b = db
            .with_conn(move |conn| {
                Ok(create_todo_task(conn, &pid, "Autre tâche")
                    .summary
                    .reference)
            })
            .await
            .unwrap();

        let ea = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_a,
                parent_discussion_id: parent_id.clone(),
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("a".into()),
            },
        )
        .await
        .unwrap();
        let eb = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_b,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("b".into()),
            },
        )
        .await
        .unwrap();

        assert_ne!(ea.id, eb.id);
        assert_ne!(
            ea.sub_discussion_id, eb.sub_discussion_id,
            "each execution owns a fresh sub-discussion (1:1, ADR §1)"
        );
    }

    #[tokio::test]
    async fn compensates_a_worktree_failure_then_resumes() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _pid) = seed(&db, repo.path()).await;

        // Inject a Phase-D failure INDEPENDENT of the execution id: make the
        // worktrees base un-creatable by planting a FILE where the dir must go.
        std::fs::create_dir_all(repo.path().join(".kronn")).unwrap();
        std::fs::write(repo.path().join(".kronn/worktrees"), "block").unwrap();

        let input = |key: &str| ProvisionInput {
            task_reference: task_ref.clone(),
            parent_discussion_id: parent_id.clone(),
            worker: native_worker(),
            base_rev: Some("main".into()),
            idempotency_key: Some(key.to_string()),
        };

        let err = provision_single_task_execution(&db, input("resume-me"))
            .await
            .expect_err("worktree creation must fail");
        assert!(
            matches!(
                err,
                ProvisionError::WorkspaceFailed {
                    compensated: true,
                    ..
                }
            ),
            "{err:?}"
        );

        // The execution is Blocked + resumable; its managed intent row was
        // compensated away; no orphan worktree, no brief/job leaked.
        let tref = task_ref.clone();
        let blocked = db
            .with_conn(move |conn| {
                let id = crate::db::planning::get_task(conn, &tref)?
                    .unwrap()
                    .summary
                    .id;
                crate::db::orchestration::get_active_execution_for_task(conn, &id)
            })
            .await
            .unwrap()
            .expect("a resumable execution remains");
        assert_eq!(blocked.status, TaskExecutionStatus::Blocked);
        assert!(blocked.blocked_reason.is_some());
        assert!(
            blocked.sub_discussion_id.is_some(),
            "sub-disc created pre-D"
        );
        let eid = blocked.id.clone();
        let ws = db
            .with_conn(move |conn| {
                crate::db::discussion_workspaces::get_managed_for_execution(conn, &eid)
            })
            .await
            .unwrap();
        assert!(ws.is_none(), "managed intent row was compensated");
        assert_eq!(count(&db, "SELECT COUNT(*) FROM messages").await, 0);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM agent_dispatch_jobs").await,
            0
        );

        // Clear the injected fault and resume with the SAME key — it must reuse its
        // own sub-disc and drive to Working.
        std::fs::remove_file(repo.path().join(".kronn/worktrees")).unwrap();
        let resumed = provision_single_task_execution(&db, input("resume-me"))
            .await
            .expect("resume should succeed");
        assert_eq!(resumed.id, blocked.id, "resume reuses the same execution");
        assert_eq!(resumed.status, TaskExecutionStatus::Working);
        assert_eq!(
            resumed.sub_discussion_id, blocked.sub_discussion_id,
            "resume reuses its own sub-discussion (keyed by the execution)"
        );
    }

    // ── Direct sync test of the atomic checkpoint's rollback integrality ──
    #[test]
    fn checkpoint_rolls_back_when_task_is_not_todo() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();

        // Seed project, parent + sub discussions, and a task we then move out of Todo.
        let repo = "/tmp/whatever";
        crate::db::projects::insert_project(&conn, &test_project("p", repo)).unwrap();
        crate::db::discussions::insert_discussion(&conn, &plain_discussion("parent", "p")).unwrap();
        crate::db::discussions::insert_discussion(&conn, &plain_discussion("sub", "p")).unwrap();
        let task = create_todo_task(&conn, "p", "Task");
        conn.execute(
            "UPDATE planning_tasks SET status='in_progress' WHERE id=?1",
            [&task.summary.id],
        )
        .unwrap();

        // A Provisioning execution.
        let mut launch = crate::models::LaunchSingleTaskInput::new(&task.summary.id, "parent");
        launch.project_id = Some("p".into());
        launch.worker_target_kind = Some(MessageTargetKind::Agent);
        launch.worker_agent_type = Some("ClaudeCode".into());
        let outcome =
            crate::db::orchestration::launch_single_task(&conn, &launch, &test_actor()).unwrap();
        let execution = advance_to_provisioning(&conn, outcome.execution, &test_actor()).unwrap();
        assert_eq!(execution.status, TaskExecutionStatus::Provisioning);

        let prepared = Prepared {
            execution: execution.clone(),
            project_id: "p".into(),
            repo_path: repo.into(),
            task_reference: task.summary.reference.clone(),
            task_title: task.summary.title.clone(),
            task_description: "x".into(),
            dod: task.definition_of_done.clone(),
            already_launched: false,
        };
        let brief = build_brief(&prepared, "/tmp/wt", "kronn/task/kt-x", "deadbeef");
        let target = native_worker();

        let outcome = crate::db::orchestration::commit_provisioning_checkpoint(
            &conn,
            &ProvisioningCheckpoint {
                exec_id: &execution.id,
                sub_discussion_id: "sub",
                task_reference: &task.summary.reference,
                attempt_no: 0,
                brief: &brief,
                target: &target,
                actor: &test_actor(),
            },
        )
        .unwrap();

        assert!(
            matches!(
                outcome,
                CheckpointOutcome::TaskNotStarted(StartTaskCheckpoint::NotTodo)
            ),
            "a non-Todo task must roll the checkpoint back"
        );
        // Rollback integrality: execution stays Provisioning, task unchanged, and
        // NOTHING became visible (no brief, no dispatch job).
        let after = crate::db::orchestration::get_task_execution(&conn, &execution.id)
            .unwrap()
            .unwrap();
        assert_eq!(after.status, TaskExecutionStatus::Provisioning);
        assert!(after.dispatch_job_id.is_none());
        let msgs: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        let jobs: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(msgs, 0, "brief must not be visible after rollback");
        assert_eq!(jobs, 0, "dispatch must not be visible after rollback");
    }

    // ── KT-319 tranche 2 — the deliver path (worker → AwaitingReview + review request). ──

    /// Drive a CLI worker to `Working` via the full KT-328 handshake, returning the handles
    /// the deliver tests act on.
    async fn attached_cli_worker(db: &Database, repo: &Path) -> (String, String, String, String) {
        let (task_ref, parent_id, child_id, offer_id) = parked_cli_worker(db, repo).await;
        let outcome =
            accept_worker_offer_and_attach(db, &offer_id, "ClaudeCode", "sess-a", "sess-a")
                .await
                .unwrap();
        let exec_id = match outcome {
            AcceptAttachOutcome::Attached { execution, .. } => {
                assert_eq!(execution.status, TaskExecutionStatus::Working);
                execution.id
            }
            other => panic!("expected Attached, got {other:?}"),
        };
        (task_ref, parent_id, child_id, exec_id)
    }

    /// KT-320 DoD-9: terminality is the return boundary for a joined CLI.
    /// Done, Cancelled and Failed all leave a durable message in both rooms and
    /// restore both the source binding and the live session membership.
    #[tokio::test]
    async fn every_terminal_state_returns_the_cli_worker_to_its_origin() {
        async fn exercise(terminal: TaskExecutionStatus) {
            let repo = init_repo();
            let db = Database::open_in_memory().unwrap();
            let (parent_id, child_id, exec_id) = if terminal == TaskExecutionStatus::Failed {
                // Failed is reachable from Provisioning. A parked CLI already
                // owns its child room but has not moved its session yet.
                let (_task, parent, child, offer_id) = parked_cli_worker(&db, repo.path()).await;
                let exec_id = db
                    .with_conn(move |conn| {
                        Ok(crate::db::worker_offers::get_worker_offer(conn, &offer_id)?
                            .expect("offer")
                            .task_execution_id)
                    })
                    .await
                    .unwrap();
                (parent, child, exec_id)
            } else {
                let (_task, parent, child, exec_id) = attached_cli_worker(&db, repo.path()).await;
                (parent, child, exec_id)
            };

            let eid = exec_id.clone();
            db.with_conn(move |conn| {
                let path: &[TaskExecutionStatus] = match terminal {
                    TaskExecutionStatus::Done => &[
                        TaskExecutionStatus::AwaitingReview,
                        TaskExecutionStatus::Approved,
                        TaskExecutionStatus::Integrating,
                        TaskExecutionStatus::Validating,
                        TaskExecutionStatus::Applying,
                    ],
                    TaskExecutionStatus::Cancelled => &[TaskExecutionStatus::Cancelled],
                    TaskExecutionStatus::Failed => &[
                        TaskExecutionStatus::Provisioning,
                        TaskExecutionStatus::Failed,
                    ],
                    _ => unreachable!(),
                };
                for &to in path {
                    assert!(crate::db::orchestration::transition_execution(
                        conn,
                        &eid,
                        to,
                        &backend_actor(),
                        serde_json::json!({ "test": "terminal_return" }),
                    )?);
                }
                if terminal == TaskExecutionStatus::Done {
                    const MERGE: &str = "dddddddddddddddddddddddddddddddddddddddd";
                    conn.execute(
                        "UPDATE task_executions SET candidate_merge_sha = ?2 WHERE id = ?1",
                        rusqlite::params![eid, MERGE],
                    )?;
                    assert_eq!(
                        crate::db::orchestration::commit_integration_checkpoint(
                            conn,
                            &eid,
                            crate::db::orchestration::IntegrationStep::Integrated {
                                integrated_sha: MERGE,
                            },
                            &backend_actor(),
                        )?,
                        crate::db::orchestration::IntegrationCheckpointOutcome::Committed {
                            status: TaskExecutionStatus::Done
                        }
                    );
                }
                Ok(())
            })
            .await
            .unwrap();

            let (eid, parent, child) = (exec_id.clone(), parent_id.clone(), child_id.clone());
            let (binding, session_disc, child_trace, origin_trace, status) = db
                .with_conn(move |conn| {
                    let binding = crate::db::disc_source::find_disc_by_source_session(
                        conn,
                        "ClaudeCode",
                        "sess-a",
                    )?;
                    let session_disc: String = conn.query_row(
                        "SELECT disc_id FROM discussion_sessions WHERE id = 101",
                        [],
                        |row| row.get(0),
                    )?;
                    let child_trace: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM messages WHERE discussion_id = ?1 AND id = ?2",
                        rusqlite::params![
                            child,
                            format!("orch-return-child:{eid}:{}", terminal.as_str())
                        ],
                        |row| row.get(0),
                    )?;
                    let origin_trace: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM messages WHERE discussion_id = ?1 AND id = ?2",
                        rusqlite::params![
                            parent,
                            format!("orch-return-origin:{eid}:{}", terminal.as_str())
                        ],
                        |row| row.get(0),
                    )?;
                    let status = crate::db::orchestration::get_task_execution(conn, &eid)?
                        .expect("execution")
                        .status;
                    Ok((binding, session_disc, child_trace, origin_trace, status))
                })
                .await
                .unwrap();
            assert_eq!(status, terminal);
            assert_eq!(binding.as_deref(), Some(parent_id.as_str()));
            assert_eq!(session_disc, parent_id);
            assert_eq!(
                child_trace, 1,
                "child terminal trace missing for {terminal:?}"
            );
            assert_eq!(
                origin_trace, 1,
                "origin terminal trace missing for {terminal:?}"
            );
        }

        for terminal in [
            TaskExecutionStatus::Done,
            TaskExecutionStatus::Cancelled,
            TaskExecutionStatus::Failed,
        ] {
            exercise(terminal).await;
        }
    }

    fn manifest_json(head_sha: &str) -> String {
        manifest_json_with_files_for_dod(head_sha, serde_json::json!([]), "d1", true)
    }

    fn manifest_json_with_files_for_dod(
        head_sha: &str,
        files_touched: serde_json::Value,
        dod_id: &str,
        met: bool,
    ) -> String {
        serde_json::json!({
            "version": "1", "task_ref": "KT-1", "head_sha": head_sha,
            "files_touched": files_touched,
            "tests": [{ "name": "cargo test --lib x", "status": "pass", "evidence": "exit 0" }],
            "dod_status": [{ "dod_id": dod_id, "met": met, "evidence": if met { "x.rs:1" } else { "principal must validate" } }],
            "docs": [], "migrations": [], "risks": [], "limitations": [],
            "summary": "did the thing"
        })
        .to_string()
    }

    async fn dod_id_for_execution(db: &Database, exec_id: &str) -> String {
        let execution_id = exec_id.to_string();
        db.with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                .context("execution")?;
            let task = crate::db::planning::get_task(conn, &execution.task_id)?.context("task")?;
            Ok(task.definition_of_done[0].id.clone())
        })
        .await
        .unwrap()
    }

    async fn clean_manifest_for_execution(db: &Database, exec_id: &str) -> String {
        let path = managed_worktree_path(db, exec_id).await;
        let dod_id = dod_id_for_execution(db, exec_id).await;
        manifest_json_with_files_for_dod(
            &git_rev(Path::new(&path), "HEAD"),
            serde_json::json!([]),
            &dod_id,
            true,
        )
    }

    async fn projected_manifest_for_execution(db: &Database, exec_id: &str) -> String {
        let execution_id = exec_id.to_string();
        let dod_count = db
            .with_conn(move |conn| {
                let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                    .context("execution")?;
                let task =
                    crate::db::planning::get_task(conn, &execution.task_id)?.context("task")?;
                Ok(task.definition_of_done.len())
            })
            .await
            .unwrap();
        serde_json::json!({
            "tests": [{ "name": "principal validation", "status": "skipped", "evidence": "no shell; principal must run" }],
            "dod_status": (0..dod_count).map(|_| serde_json::json!({
                "met": true,
                "evidence": "implemented in the committed worktree"
            })).collect::<Vec<_>>(),
            "docs": [],
            "migrations": [],
            "risks": [],
            "limitations": [],
            "summary": "did the thing"
        })
        .to_string()
    }

    #[tokio::test]
    async fn native_worker_delivery_uses_child_room_and_exact_provider_identity() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("native-delivery".into()),
            },
        )
        .await
        .unwrap();
        let child = execution.sub_discussion_id.clone().unwrap();
        let dispatch_trigger = {
            let dispatch_job_id = execution.dispatch_job_id.clone().unwrap();
            db.with_conn(move |conn| {
                Ok(crate::db::agent_dispatch::get(conn, &dispatch_job_id)?
                    .unwrap()
                    .trigger_message_id)
            })
            .await
            .unwrap()
        };

        let manifest = projected_manifest_for_execution(&db, &execution.id).await;
        for (discussion, provider, source_message_id) in [
            (child.as_str(), AgentType::Ollama, dispatch_trigger.as_str()),
            (
                "foreign-child",
                AgentType::ClaudeCode,
                dispatch_trigger.as_str(),
            ),
            (
                child.as_str(),
                AgentType::ClaudeCode,
                "another-same-provider-run",
            ),
        ] {
            let outcome = deliver_native_worker_manifest(
                &db,
                &execution.id,
                NativeExecutionCaller {
                    discussion_id: discussion,
                    agent_type: &provider,
                    source_message_id: Some(source_message_id),
                    alias: "native worker",
                    actor_session_id: Some("native-delivery-turn"),
                },
                &manifest,
            )
            .await
            .unwrap();
            assert!(matches!(outcome, DeliverOutcome::NotAddressed));
        }

        let outcome = deliver_native_worker_manifest(
            &db,
            &execution.id,
            NativeExecutionCaller {
                discussion_id: &child,
                agent_type: &AgentType::ClaudeCode,
                source_message_id: Some(&dispatch_trigger),
                alias: "Claude Code",
                actor_session_id: Some("native-delivery-turn"),
            },
            &manifest,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, DeliverOutcome::Delivered { .. }));

        let expected_head = git_rev(
            Path::new(&managed_worktree_path(&db, &execution.id).await),
            "HEAD",
        );
        let expected_dod_id = dod_id_for_execution(&db, &execution.id).await;
        assert_eq!(
            execution.worker_dod_ids.as_deref(),
            Some(&[expected_dod_id.clone()][..])
        );
        let execution_id = execution.id.clone();
        let detail = db
            .with_conn(move |conn| execution_detail(conn, &execution_id))
            .await
            .unwrap();
        let persisted = detail.attempts[0]
            .delivery
            .as_ref()
            .expect("normalized native delivery persisted");
        assert_eq!(persisted.version, DELIVERY_CONTRACT_VERSION);
        assert_eq!(persisted.task_ref, "KT-1");
        assert_eq!(persisted.head_sha, expected_head);
        assert!(persisted.files_touched.is_empty());
        assert_eq!(persisted.dod_status[0].dod_id, expected_dod_id);
    }

    #[tokio::test]
    async fn native_worker_delivery_refuses_wrong_dod_count_without_mutating_state() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("native-delivery-wrong-dod-count".into()),
            },
        )
        .await
        .unwrap();
        let child = execution.sub_discussion_id.clone().unwrap();
        let dispatch_trigger = {
            let dispatch_job_id = execution.dispatch_job_id.clone().unwrap();
            db.with_conn(move |conn| {
                Ok(crate::db::agent_dispatch::get(conn, &dispatch_job_id)?
                    .unwrap()
                    .trigger_message_id)
            })
            .await
            .unwrap()
        };
        let projected = serde_json::json!({
            "tests": [],
            "dod_status": [],
            "docs": [],
            "migrations": [],
            "risks": [],
            "limitations": [],
            "summary": "missing the required ordered DoD assertion"
        })
        .to_string();
        let outcome = deliver_native_worker_manifest(
            &db,
            &execution.id,
            NativeExecutionCaller {
                discussion_id: &child,
                agent_type: &AgentType::ClaudeCode,
                source_message_id: Some(&dispatch_trigger),
                alias: "Claude Code",
                actor_session_id: Some("native-delivery-turn"),
            },
            &projected,
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome,
            DeliverOutcome::InvalidManifest(ref detail)
                if detail.contains("exactly 1 item(s)") && detail.contains("got 0")
        ));

        let mut forged_mechanics: serde_json::Value =
            serde_json::from_str(&projected_manifest_for_execution(&db, &execution.id).await)
                .unwrap();
        forged_mechanics["head_sha"] = serde_json::json!("deadbeef");
        forged_mechanics["version"] = serde_json::json!("1");
        forged_mechanics["task_ref"] = serde_json::json!("KT-forged");
        forged_mechanics["files_touched"] = serde_json::json!([]);
        forged_mechanics["reviewer_note"] = serde_json::json!("approved by Kronn");
        forged_mechanics["dod_status"][0]["dod_id"] = serde_json::json!("copied-id");
        forged_mechanics["tests"][0]["reviewer_note"] = serde_json::json!("trusted");
        let outcome = deliver_native_worker_manifest(
            &db,
            &execution.id,
            NativeExecutionCaller {
                discussion_id: &child,
                agent_type: &AgentType::ClaudeCode,
                source_message_id: Some(&dispatch_trigger),
                alias: "Claude Code",
                actor_session_id: Some("native-delivery-turn"),
            },
            &forged_mechanics.to_string(),
        )
        .await
        .unwrap();
        let DeliverOutcome::InvalidManifest(detail) = outcome else {
            panic!("forged mechanics must be refused")
        };
        for forbidden in [
            "`head_sha`",
            "`version`",
            "`task_ref`",
            "`files_touched`",
            "`dod_status[0].dod_id`",
            "`reviewer_note`",
            "`tests[0].reviewer_note`",
        ] {
            assert!(
                detail.contains(forbidden),
                "one refusal must report every forbidden field; missing {forbidden}: {detail}"
            );
        }

        let task_id = execution.task_id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE planning_task_dod_items SET id = 'same-count-new-id' WHERE task_id = ?1",
                [&task_id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let drifted = projected_manifest_for_execution(&db, &execution.id).await;
        let outcome = deliver_native_worker_manifest(
            &db,
            &execution.id,
            NativeExecutionCaller {
                discussion_id: &child,
                agent_type: &AgentType::ClaudeCode,
                source_message_id: Some(&dispatch_trigger),
                alias: "Claude Code",
                actor_session_id: Some("native-delivery-turn"),
            },
            &drifted,
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome,
            DeliverOutcome::InvalidManifest(ref detail)
                if detail.contains("Definition of Done changed since this execution was launched")
        ));

        let execution_id = execution.id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE task_executions SET worker_dod_ids_json = NULL WHERE id = ?1",
                [&execution_id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let pre_migration = projected_manifest_for_execution(&db, &execution.id).await;
        let outcome = deliver_native_worker_manifest(
            &db,
            &execution.id,
            NativeExecutionCaller {
                discussion_id: &child,
                agent_type: &AgentType::ClaudeCode,
                source_message_id: Some(&dispatch_trigger),
                alias: "Claude Code",
                actor_session_id: Some("native-delivery-turn"),
            },
            &pre_migration,
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome,
            DeliverOutcome::InvalidManifest(ref detail)
                if detail.contains("no launch-time Definition of Done snapshot")
        ));
        let execution_id = execution.id.clone();
        let stored = db
            .with_conn(move |conn| {
                crate::db::orchestration::get_task_execution(conn, &execution_id)
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, TaskExecutionStatus::Working);
    }

    #[tokio::test]
    async fn spawned_worker_commit_is_server_mediated_and_exactly_scoped() {
        let repo = init_repo();
        let base_sha = git_rev(repo.path(), "HEAD");
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("runner-scoped-cli-commit".into()),
            },
        )
        .await
        .unwrap();
        let child = execution.sub_discussion_id.clone().unwrap();
        let dispatch_trigger = {
            let dispatch_job_id = execution.dispatch_job_id.clone().unwrap();
            db.with_read_conn(move |conn| {
                Ok(crate::db::agent_dispatch::get(conn, &dispatch_job_id)?
                    .unwrap()
                    .trigger_message_id)
            })
            .await
            .unwrap()
        };
        let worktree = managed_worktree_path(&db, &execution.id).await;
        std::fs::write(Path::new(&worktree).join("README.md"), "# mediated\n").unwrap();
        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let request = |discussion_id: String, files: Vec<String>| SpawnedWorkerCommitRequest {
            task_execution_id: execution.id.clone(),
            files,
            message: "test: mediated worker commit".into(),
            spawned_agent: SpawnedAgentCaller {
                discussion_id,
                agent_type: "ClaudeCode".into(),
                source_message_id: dispatch_trigger.clone(),
            },
        };

        let wrong_identity = commit_spawned_worker(
            State(state.clone()),
            Json(request("another-child".into(), vec!["README.md".into()])),
        )
        .await
        .0;
        assert!(!wrong_identity.success);
        assert_eq!(wrong_identity.error_code.as_deref(), Some("not_found"));
        assert_eq!(git_rev(Path::new(&worktree), "HEAD"), base_sha);

        let outside = commit_spawned_worker(
            State(state.clone()),
            Json(request(child.clone(), vec!["../README.md".into()])),
        )
        .await
        .0;
        assert!(!outside.success);
        assert_eq!(outside.error_code.as_deref(), Some("validation"));
        assert_eq!(git_rev(Path::new(&worktree), "HEAD"), base_sha);

        let committed = commit_spawned_worker(
            State(state.clone()),
            Json(request(child.clone(), vec!["README.md".into()])),
        )
        .await
        .0;
        assert!(committed.success, "exact runner capability must commit");
        let payload = committed.data.unwrap();
        assert_eq!(payload["files"], serde_json::json!(["README.md"]));
        assert_ne!(git_rev(Path::new(&worktree), "HEAD"), base_sha);
        assert_eq!(
            git_rev(repo.path(), "HEAD"),
            base_sha,
            "the mediated commit must advance only the worker branch"
        );

        let execution_id = execution.id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE task_executions SET status = 'AwaitingReview' WHERE id = ?1",
                [&execution_id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        std::fs::write(Path::new(&worktree).join("README.md"), "# too late\n").unwrap();
        let not_working = commit_spawned_worker(
            State(state.clone()),
            Json(request(child.clone(), vec!["README.md".into()])),
        )
        .await
        .0;
        assert!(!not_working.success);
        assert_eq!(not_working.error_code.as_deref(), Some("conflict"));

        let workspace_id = execution.workspace_id.clone().unwrap();
        let execution_id = execution.id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE task_executions SET status = 'Working' WHERE id = ?1",
                [&execution_id],
            )?;
            conn.execute(
                "UPDATE discussion_workspaces SET state = 'detached' WHERE id = ?1",
                [&workspace_id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        std::fs::write(Path::new(&worktree).join("README.md"), "# stale\n").unwrap();
        let stale =
            commit_spawned_worker(State(state), Json(request(child, vec!["README.md".into()])))
                .await
                .0;
        assert!(!stale.success);
        assert_eq!(stale.error_code.as_deref(), Some("conflict"));
    }

    #[tokio::test]
    async fn deliver_handler_reuses_native_checks_for_runner_scoped_cli_agents() {
        let repo = init_repo();
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("runner-scoped-cli-delivery".into()),
            },
        )
        .await
        .unwrap();
        let child = execution.sub_discussion_id.clone().unwrap();
        let dispatch_trigger = {
            let dispatch_job_id = execution.dispatch_job_id.clone().unwrap();
            db.with_conn(move |conn| {
                Ok(crate::db::agent_dispatch::get(conn, &dispatch_job_id)?
                    .unwrap()
                    .trigger_message_id)
            })
            .await
            .unwrap()
        };
        let manifest: serde_json::Value =
            serde_json::from_str(&projected_manifest_for_execution(&db, &execution.id).await)
                .unwrap();
        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );

        let refused = deliver(
            State(state.clone()),
            Json(DeliverRequest {
                task_execution_id: execution.id.clone(),
                manifest: manifest.clone(),
                source_agent: None,
                source_session_id: None,
                spawned_agent: Some(SpawnedAgentCaller {
                    discussion_id: "another-child".into(),
                    agent_type: "ClaudeCode".into(),
                    source_message_id: dispatch_trigger.clone(),
                }),
            }),
        )
        .await
        .0;
        assert!(!refused.success);
        assert_eq!(refused.error_code.as_deref(), Some("not_found"));

        let delivered = deliver(
            State(state),
            Json(DeliverRequest {
                task_execution_id: execution.id,
                manifest,
                source_agent: None,
                source_session_id: None,
                spawned_agent: Some(SpawnedAgentCaller {
                    discussion_id: child,
                    agent_type: "ClaudeCode".into(),
                    source_message_id: dispatch_trigger,
                }),
            }),
        )
        .await
        .0;
        assert!(delivered.success, "exact runner capability must deliver");
        assert_eq!(
            delivered.data.unwrap().execution.status,
            TaskExecutionStatus::AwaitingReview
        );
    }

    #[tokio::test]
    async fn status_reference_recovers_the_execution_after_a_lost_chat_cursor() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref.clone(),
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("reconnect-status".into()),
            },
        )
        .await
        .unwrap();
        let recovered = db
            .with_conn(move |conn| resolve_task_execution_reference(conn, &task_ref))
            .await
            .unwrap()
            .expect("task reference must recover its active execution");
        assert_eq!(recovered.id, execution.id);
    }

    #[tokio::test]
    async fn preflight_explicitly_refuses_a_native_runtime_without_typed_delivery() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let task_for_link = task_ref.clone();
        let parent_for_link = parent_id.clone();
        db.with_conn(move |conn| {
            crate::db::planning::link_discussion(
                conn,
                &task_for_link,
                &crate::models::LinkPlanningDiscussionRequest {
                    discussion_id: parent_for_link,
                    placement: crate::models::PlanningPlacement::Active,
                    is_primary: false,
                    position: None,
                    actor: test_actor(),
                },
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let preparation = db
            .with_conn(move |conn| {
                prepare_task_execution(
                    conn,
                    &task_ref,
                    &parent_id,
                    &MessageTarget::agent(AgentType::Vibe),
                )
            })
            .await
            .unwrap();
        assert!(!preparation.launchable);
        assert!(preparation
            .reasons
            .iter()
            .any(|reason| reason.code == "worker_capability"));
    }

    #[tokio::test]
    async fn preflight_and_launch_refuse_cross_transport_workers_before_mutation() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let task_for_checks = task_ref.clone();
        let parent_for_checks = parent_id.clone();
        let (bad_host, bad_http, good_host, good_http) = db
            .with_conn(move |conn| {
                crate::db::planning::link_discussion(
                    conn,
                    &task_for_checks,
                    &crate::models::LinkPlanningDiscussionRequest {
                        discussion_id: parent_for_checks.clone(),
                        placement: crate::models::PlanningPlacement::Active,
                        is_primary: false,
                        position: None,
                        actor: test_actor(),
                    },
                )?;
                Ok((
                    prepare_task_execution(
                        conn,
                        &task_for_checks,
                        &parent_for_checks,
                        &MessageTarget::discussion_agent(AgentType::ClaudeCode),
                    )?,
                    prepare_task_execution(
                        conn,
                        &task_for_checks,
                        &parent_for_checks,
                        &MessageTarget::agent(AgentType::Ollama),
                    )?,
                    prepare_task_execution(
                        conn,
                        &task_for_checks,
                        &parent_for_checks,
                        &MessageTarget::agent(AgentType::ClaudeCode),
                    )?,
                    prepare_task_execution(
                        conn,
                        &task_for_checks,
                        &parent_for_checks,
                        &MessageTarget::discussion_agent(AgentType::Ollama),
                    )?,
                ))
            })
            .await
            .unwrap();

        for refused in [bad_host, bad_http] {
            assert!(!refused.launchable);
            assert!(refused
                .reasons
                .iter()
                .any(|reason| reason.code == "worker_transport"));
        }
        assert!(good_host.launchable, "{:#?}", good_host.reasons);
        assert!(good_http.launchable, "{:#?}", good_http.reasons);

        let error = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: MessageTarget::discussion_agent(AgentType::ClaudeCode),
                base_rev: Some("main".into()),
                idempotency_key: Some("cross-transport-refusal".into()),
            },
        )
        .await
        .expect_err("launch must re-check transport compatibility");
        assert!(matches!(
            error,
            ProvisionError::NotLaunchable(ref reason) if reason.contains("worker_transport")
        ));
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM task_executions").await,
            0,
            "refusal happens before execution/worktree/dispatch creation"
        );
    }

    #[tokio::test]
    async fn native_review_uses_parent_room_and_preserves_anti_oracle_refusal() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id.clone(),
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("native-review".into()),
            },
        )
        .await
        .unwrap();
        let child = execution.sub_discussion_id.clone().unwrap();
        let dispatch_trigger = {
            let dispatch_job_id = execution.dispatch_job_id.clone().unwrap();
            db.with_conn(move |conn| {
                Ok(crate::db::agent_dispatch::get(conn, &dispatch_job_id)?
                    .unwrap()
                    .trigger_message_id)
            })
            .await
            .unwrap()
        };
        let manifest = projected_manifest_for_execution(&db, &execution.id).await;
        deliver_native_worker_manifest(
            &db,
            &execution.id,
            NativeExecutionCaller {
                discussion_id: &child,
                agent_type: &AgentType::ClaudeCode,
                source_message_id: Some(&dispatch_trigger),
                alias: "Claude Code",
                actor_session_id: Some("native-delivery-turn"),
            },
            &manifest,
        )
        .await
        .unwrap();
        let decision = review_request_changes("tighten the regression test");

        let foreign = decide_native_review(
            &db,
            &execution.id,
            &decision,
            NativeExecutionCaller {
                discussion_id: "foreign-room",
                agent_type: &AgentType::Ollama,
                source_message_id: Some("foreign-trigger"),
                alias: "Ollama",
                actor_session_id: Some("native-review-turn"),
            },
        )
        .await
        .unwrap();
        assert!(matches!(foreign, ReviewOutcome::NotAddressed));

        let reviewed = decide_native_review(
            &db,
            &execution.id,
            &decision,
            NativeExecutionCaller {
                discussion_id: &parent_id,
                agent_type: &AgentType::Ollama,
                source_message_id: Some("principal-review-turn"),
                alias: "Ollama",
                actor_session_id: Some("native-review-turn"),
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            reviewed,
            ReviewOutcome::Reviewed {
                verdict: ReviewVerdict::RequestChanges,
                ..
            }
        ));
    }

    /// Happy path (DoD-1/2/3): the exact worker delivers → manifest persisted, execution
    /// `AwaitingReview`, a queryable `review_requested` obligation naming the targeted
    /// principal, and a principal-targeted review request in the PARENT room (zero dispatch).
    #[tokio::test]
    async fn worker_delivers_a_manifest_and_requests_review() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _child_id, exec_id) = attached_cli_worker(&db, repo.path()).await;

        let manifest = clean_manifest_for_execution(&db, &exec_id).await;
        let delivered_head = parse_delivery_manifest(&manifest).unwrap().head_sha;
        let outcome = deliver_worker_manifest(&db, &exec_id, "ClaudeCode", "sess-a", &manifest)
            .await
            .unwrap();
        match outcome {
            DeliverOutcome::Delivered {
                review_discussion_id,
                execution,
            } => {
                assert_eq!(
                    review_discussion_id, parent_id,
                    "review request lands in the parent"
                );
                assert_eq!(
                    execution.status,
                    TaskExecutionStatus::AwaitingReview,
                    "execution flips to AwaitingReview via the guarded CAS"
                );
            }
            other => panic!("expected Delivered, got {other:?}"),
        }

        // The manifest is persisted for (exec, attempt 0) with the denormalized head_sha.
        let e = exec_id.clone();
        let delivery = db
            .with_conn(move |conn| crate::db::worker_deliveries::get_delivery(conn, &e, 0))
            .await
            .unwrap()
            .expect("a delivery row must persist");
        assert_eq!(delivery.head_sha, delivered_head);

        // A `review_requested` event exists, attributed to the worker (Agent), and its
        // payload names the TARGETED principal identity (DoD-3 — obligation, not deduced).
        let e = exec_id.clone();
        let (n_events, changes, actor_kind, actor_session_id) = db
            .with_conn(move |conn| {
                let n: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM task_execution_events \
                     WHERE task_execution_id = ?1 AND action = 'review_requested'",
                    [&e],
                    |r| r.get(0),
                )?;
                let (changes, kind, session): (String, String, Option<String>) = conn.query_row(
                    "SELECT changes_json, actor_kind, actor_session_id FROM task_execution_events \
                     WHERE task_execution_id = ?1 AND action = 'review_requested'",
                    [&e],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?;
                Ok((n, changes, kind, session))
            })
            .await
            .unwrap();
        assert_eq!(n_events, 1, "exactly one review_requested obligation");
        assert_eq!(
            actor_kind,
            PlanningActorKind::Agent.as_str(),
            "the delivery is attributed to the worker, not the backend"
        );
        assert_eq!(actor_session_id.as_deref(), Some("sess-a"));
        assert!(
            changes.contains(&parent_id),
            "obligation names the principal room"
        );
        assert!(
            changes.contains("principal_target"),
            "obligation carries the targeted identity"
        );

        // The review request is in the PARENT room, targeted at the native principal, ZERO dispatch.
        let (p, e) = (parent_id.clone(), exec_id.clone());
        let (msg_targets, dispatches) = db
            .with_conn(move |conn| {
                let mid = format!("orch-review-request:{e}:0");
                let disc: String = conn.query_row(
                    "SELECT discussion_id FROM messages WHERE id = ?1",
                    [&mid],
                    |r| r.get(0),
                )?;
                assert_eq!(disc, p, "review request is posted in the parent room");
                let targets = crate::db::discussions::list_message_targets(conn, &mid)?;
                let d: i64 =
                    conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |r| r.get(0))?;
                Ok((targets, d))
            })
            .await
            .unwrap();
        assert_eq!(msg_targets.len(), 1);
        assert_eq!(
            msg_targets[0].kind,
            MessageTargetKind::DiscussionAgent,
            "review request is addressed to the native principal"
        );
        assert_eq!(
            dispatches, 0,
            "no native dispatch — immediate wake is KT-335"
        );

        // The obligation is interrogeable (DoD-3): the review is due for the principal room.
        let p = parent_id.clone();
        let due = db
            .with_conn(move |conn| {
                crate::db::orchestration::list_reviews_due_for_discussion(conn, &p)
            })
            .await
            .unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, exec_id);

        // Delivery did not touch the task status (it was already InProgress from the handshake).
        let tref = task_ref.clone();
        let task = db
            .with_conn(move |conn| crate::db::planning::get_task(conn, &tref))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(task.summary.status, PlanningTaskStatus::InProgress);
    }

    /// A session that is NOT the execution's worker cannot deliver — NotAddressed (fused
    /// with unknown-execution → anti-oracle), and nothing moves (DoD-2).
    #[tokio::test]
    async fn deliver_is_refused_for_a_wrong_worker_session() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_task_ref, parent_id, _child_id, exec_id) =
            attached_cli_worker(&db, repo.path()).await;
        // A DIFFERENT active session (same provider) — not this execution's worker.
        seed_cli_session(&db, 102, &parent_id, "sess-b").await;

        let outcome = deliver_worker_manifest(
            &db,
            &exec_id,
            "ClaudeCode",
            "sess-b",
            &manifest_json("abcdef1234567"),
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, DeliverOutcome::NotAddressed),
            "wrong worker → NotAddressed"
        );

        // Nothing moved: still Working, no delivery row, no review request.
        let e = exec_id.clone();
        let (status, delivery, review_msgs) = db
            .with_conn(move |conn| {
                let status = crate::db::orchestration::get_task_execution(conn, &e)?
                    .unwrap()
                    .status;
                let delivery = crate::db::worker_deliveries::get_delivery(conn, &e, 0)?;
                let n: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM messages WHERE id LIKE 'orch-review-request:%'",
                    [],
                    |r| r.get(0),
                )?;
                Ok((status, delivery, n))
            })
            .await
            .unwrap();
        assert_eq!(
            status,
            TaskExecutionStatus::Working,
            "execution stays Working"
        );
        assert!(
            delivery.is_none(),
            "no delivery persisted for a rejected caller"
        );
        assert_eq!(review_msgs, 0, "no review request posted");
    }

    /// A crash/double-click resubmit of the SAME attempt converges idempotently: still
    /// Delivered, exactly one delivery row and one review request (DoD-8).
    #[tokio::test]
    async fn deliver_is_idempotent_on_resume() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_task_ref, _parent_id, _child_id, exec_id) =
            attached_cli_worker(&db, repo.path()).await;

        let manifest = clean_manifest_for_execution(&db, &exec_id).await;
        let first = deliver_worker_manifest(&db, &exec_id, "ClaudeCode", "sess-a", &manifest)
            .await
            .unwrap();
        assert!(matches!(first, DeliverOutcome::Delivered { .. }));
        let again = deliver_worker_manifest(&db, &exec_id, "ClaudeCode", "sess-a", &manifest)
            .await
            .unwrap();
        assert!(
            matches!(again, DeliverOutcome::Delivered { .. }),
            "a resubmit of the same attempt converges to Delivered, got {again:?}"
        );

        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM task_execution_deliveries").await,
            1,
            "exactly one delivery row after a resubmit"
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM messages WHERE id LIKE 'orch-review-request:%'"
            )
            .await,
            1,
            "exactly one review request after a resubmit"
        );
    }

    /// The worker cannot deliver an execution that is not `Working` — a parked (Blocked)
    /// execution names the real state (NotDeliverable), reachable only after authz.
    #[tokio::test]
    async fn deliver_is_refused_when_the_execution_is_not_working() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        // Parked, NOT accepted → Blocked(awaiting_worker_acceptance), worker session = 101.
        let (_task_ref, _parent_id, _child_id, offer_id) =
            parked_cli_worker(&db, repo.path()).await;
        let exec_id = {
            let oid = offer_id.clone();
            db.with_conn(move |conn| {
                Ok(crate::db::worker_offers::get_worker_offer(conn, &oid)?
                    .unwrap()
                    .task_execution_id)
            })
            .await
            .unwrap()
        };

        let outcome = deliver_worker_manifest(
            &db,
            &exec_id,
            "ClaudeCode",
            "sess-a",
            &manifest_json("abcdef1234567"),
        )
        .await
        .unwrap();
        assert!(
            matches!(
                outcome,
                DeliverOutcome::NotDeliverable {
                    status: TaskExecutionStatus::Blocked
                }
            ),
            "a Blocked execution is not deliverable, got {outcome:?}"
        );
    }

    /// A malformed manifest is refused (InvalidManifest naming the missing field) AFTER
    /// authz — a real validation refusal, never a silent accept.
    #[tokio::test]
    async fn deliver_rejects_an_invalid_manifest() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_task_ref, _parent_id, _child_id, exec_id) =
            attached_cli_worker(&db, repo.path()).await;

        // head_sha dropped — DoD-1 requires it.
        let bad = serde_json::json!({
            "version": "1", "task_ref": "KT-319",
            "files_touched": [], "tests": [], "dod_status": [],
            "docs": [], "migrations": [], "risks": [], "limitations": [], "summary": "x"
        })
        .to_string();
        let outcome = deliver_worker_manifest(&db, &exec_id, "ClaudeCode", "sess-a", &bad)
            .await
            .unwrap();
        match outcome {
            DeliverOutcome::InvalidManifest(detail) => {
                assert!(
                    detail.contains("head_sha"),
                    "must name the missing field, got: {detail}"
                );
            }
            other => panic!("expected InvalidManifest, got {other:?}"),
        }
        // Still Working, nothing persisted.
        let e = exec_id.clone();
        let status = db
            .with_conn(move |conn| {
                Ok(crate::db::orchestration::get_task_execution(conn, &e)?
                    .unwrap()
                    .status)
            })
            .await
            .unwrap();
        assert_eq!(status, TaskExecutionStatus::Working);
    }

    #[tokio::test]
    async fn deliver_refuses_dirty_edits_then_accepts_the_same_change_once_committed() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_task_ref, _parent_id, _child_id, exec_id) =
            attached_cli_worker(&db, repo.path()).await;
        let path = managed_worktree_path(&db, &exec_id).await;
        let dod_id = dod_id_for_execution(&db, &exec_id).await;
        std::fs::write(Path::new(&path).join("delivery-proof.txt"), "durable\n").unwrap();
        let base_head = git_rev(Path::new(&path), "HEAD");
        let dirty_manifest = manifest_json_with_files_for_dod(
            &base_head,
            serde_json::json!([{ "path": "delivery-proof.txt", "kind": "added" }]),
            &dod_id,
            true,
        );

        let refused =
            deliver_worker_manifest(&db, &exec_id, "ClaudeCode", "sess-a", &dirty_manifest)
                .await
                .unwrap();
        match refused {
            DeliverOutcome::InvalidManifest(detail) => assert!(
                detail.contains("uncommitted changes") && detail.contains("delivery-proof.txt"),
                "the refusal must tell the worker exactly what to commit: {detail}"
            ),
            other => panic!("dirty delivery must be refused, got {other:?}"),
        }
        assert_eq!(
            exec_of(&db, &exec_id).await.status,
            TaskExecutionStatus::Working,
            "a refused manifest must leave the worker able to commit and retry"
        );

        git(Path::new(&path), &["add", "delivery-proof.txt"]);
        git(Path::new(&path), &["commit", "-m", "add delivery proof"]);
        let committed_head = git_rev(Path::new(&path), "HEAD");
        let committed_manifest = manifest_json_with_files_for_dod(
            &committed_head,
            serde_json::json!([{ "path": "delivery-proof.txt", "kind": "added" }]),
            &dod_id,
            true,
        );
        let delivered =
            deliver_worker_manifest(&db, &exec_id, "ClaudeCode", "sess-a", &committed_manifest)
                .await
                .unwrap();
        assert!(matches!(delivered, DeliverOutcome::Delivered { .. }));
    }

    #[tokio::test]
    async fn deliver_refuses_claimed_files_when_head_is_still_the_execution_base() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_task_ref, _parent_id, _child_id, exec_id) =
            attached_cli_worker(&db, repo.path()).await;
        let path = managed_worktree_path(&db, &exec_id).await;
        let dod_id = dod_id_for_execution(&db, &exec_id).await;
        let base_head = git_rev(Path::new(&path), "HEAD");
        let invented = manifest_json_with_files_for_dod(
            &base_head,
            serde_json::json!([{ "path": "backend/src/invented.rs", "kind": "modified" }]),
            &dod_id,
            true,
        );

        let outcome = deliver_worker_manifest(&db, &exec_id, "ClaudeCode", "sess-a", &invented)
            .await
            .unwrap();
        match outcome {
            DeliverOutcome::InvalidManifest(detail) => assert!(
                detail.contains("does not match the committed diff")
                    && detail.contains("invented.rs"),
                "the false file claim must be actionable: {detail}"
            ),
            other => panic!("an unchanged HEAD cannot carry invented files: {other:?}"),
        }
        assert_eq!(
            exec_of(&db, &exec_id).await.status,
            TaskExecutionStatus::Working
        );
    }

    /// Anti-oracle at the HTTP frontier: an unknown execution and a wrong worker collapse
    /// into ONE opaque refusal; the actionable refusals (reachable only after authz) stay
    /// distinct + informative. Pure, no `AppState`.
    #[test]
    fn deliver_outcome_fuses_unaddressed_and_keeps_others_distinct() {
        let unaddressed = deliver_outcome_to_response(DeliverOutcome::NotAddressed);
        assert!(!unaddressed.success);
        assert_eq!(unaddressed.error_code.as_deref(), Some("not_found"));
        assert_eq!(
            unaddressed.error.as_deref(),
            Some("execution not found or not addressed to this session")
        );

        let not_deliverable = deliver_outcome_to_response(DeliverOutcome::NotDeliverable {
            status: TaskExecutionStatus::Blocked,
        });
        assert_eq!(not_deliverable.error_code.as_deref(), Some("conflict"));
        assert!(not_deliverable
            .error
            .as_deref()
            .unwrap()
            .contains(TaskExecutionStatus::Blocked.as_str()));

        let invalid = deliver_outcome_to_response(DeliverOutcome::InvalidManifest(
            "head_sha manquant".into(),
        ));
        assert_eq!(invalid.error_code.as_deref(), Some("validation"));
        assert!(invalid.error.as_deref().unwrap().contains("head_sha"));
    }

    // ── KT-319 tranche 3a — the review decide path ──────────────────────────────

    async fn review_approve(db: &Database, exec_id: &str) -> String {
        let path = managed_worktree_path(db, exec_id).await;
        let head = git_rev(Path::new(&path), "HEAD");
        let dod_id = dod_id_for_execution(db, exec_id).await;
        serde_json::json!({
            "version": "1",
            "task_ref": "KT-319",
            "decision": "approve",
            "reviewed_head_sha": head,
            "dod_verifications": [{
                "dod_id": dod_id.clone(),
                "met": true,
                "evidence": "principal inspected the delivered SHA in this test"
            }]
        })
        .to_string()
    }

    fn review_request_changes(comment: &str) -> String {
        serde_json::json!({
            "version": "1", "task_ref": "KT-319", "decision": "request_changes",
            "comment": comment,
            "findings": [{ "path": "backend/src/x.rs", "line": 10, "issue": "the guard is missing" }]
        })
        .to_string()
    }

    async fn manifest_json_unmet(db: &Database, exec_id: &str, head_sha: &str) -> String {
        let dod_id = dod_id_for_execution(db, exec_id).await;
        manifest_json_with_files_for_dod(head_sha, serde_json::json!([]), &dod_id, false)
    }

    /// The managed child-worktree path of an execution.
    async fn managed_worktree_path(db: &Database, exec_id: &str) -> String {
        let e = exec_id.to_string();
        db.with_conn(move |conn| {
            crate::db::discussion_workspaces::get_managed_for_execution(conn, &e)
        })
        .await
        .unwrap()
        .expect("managed worktree")
        .canonical_path
        .expect("canonical path")
    }

    async fn exec_of(db: &Database, exec_id: &str) -> TaskExecution {
        let e = exec_id.to_string();
        db.with_conn(move |conn| {
            Ok(crate::db::orchestration::get_task_execution(conn, &e)?.expect("execution"))
        })
        .await
        .unwrap()
    }

    async fn review_row(
        db: &Database,
        exec_id: &str,
    ) -> Option<crate::models::TaskExecutionReview> {
        let e = exec_id.to_string();
        db.with_conn(move |conn| crate::db::worker_reviews::get_review(conn, &e, 0))
            .await
            .unwrap()
    }

    /// A worker attached + its manifest delivered against the REAL child-worktree HEAD, with a
    /// principal session (102, "sess-b") seeded in the parent room. Returns
    /// (parent, child, exec, head_full, worktree_path).
    async fn delivered_awaiting_review(
        db: &Database,
        repo: &Path,
    ) -> (String, String, String, String, String) {
        let (_task_ref, parent_id, child_id, exec_id) = attached_cli_worker(db, repo).await;
        let path = managed_worktree_path(db, &exec_id).await;
        let head = git_rev(Path::new(&path), "HEAD");
        let manifest = clean_manifest_for_execution(db, &exec_id).await;
        deliver_worker_manifest(db, &exec_id, "ClaudeCode", "sess-a", &manifest)
            .await
            .unwrap();
        seed_cli_session(db, 102, &parent_id, "sess-b").await;
        (parent_id, child_id, exec_id, head, path)
    }

    /// Happy path (DoD-2/5): a principal in the parent room approves a clean delivery →
    /// Approved, the decision is persisted, and the transition is attributed to the principal.
    #[tokio::test]
    async fn principal_approves_a_delivered_manifest() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, _child, exec_id, _head, _path) =
            delivered_awaiting_review(&db, repo.path()).await;

        let outcome = decide_review(
            &db,
            &exec_id,
            &review_approve(&db, &exec_id).await,
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        match outcome {
            ReviewOutcome::Reviewed { verdict, execution } => {
                assert_eq!(verdict, ReviewVerdict::Approve);
                assert_eq!(execution.status, TaskExecutionStatus::Approved);
            }
            other => panic!("expected Reviewed/Approve, got {other:?}"),
        }

        // The decision is persisted for (exec, attempt 0), and a review transition is journaled
        // to the deciding PRINCIPAL (Agent), not the backend (DoD-2 audit).
        let row = review_row(&db, &exec_id)
            .await
            .expect("review row persisted");
        assert_eq!(row.decision, "approve");
        let e = exec_id.clone();
        let (n_reviewed, actor_kind) = db
            .with_conn(move |conn| {
                let n: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM task_execution_events \
                     WHERE task_execution_id = ?1 AND to_status = 'Approved'",
                    [&e],
                    |r| r.get(0),
                )?;
                let kind: String = conn.query_row(
                    "SELECT actor_kind FROM task_execution_events \
                     WHERE task_execution_id = ?1 AND to_status = 'Approved'",
                    [&e],
                    |r| r.get(0),
                )?;
                Ok((n, kind))
            })
            .await
            .unwrap();
        assert_eq!(n_reviewed, 1, "exactly one Approved transition");
        assert_eq!(actor_kind, PlanningActorKind::Agent.as_str());

        let e = exec_id.clone();
        let detail = db
            .with_conn(move |conn| execution_detail(conn, &e))
            .await
            .unwrap();
        assert_eq!(detail.attempts.len(), 1);
        let attempt = &detail.attempts[0];
        assert!(attempt.delivery.is_some(), "typed manifest is exposed");
        assert_eq!(
            attempt.review.as_ref().map(|review| review.decision),
            Some(ReviewVerdict::Approve)
        );
    }

    #[tokio::test]
    async fn principal_approval_requires_evidence_for_every_dod_item() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, _child, exec_id, head, _path) =
            delivered_awaiting_review(&db, repo.path()).await;
        let decision = serde_json::json!({
            "version": "1",
            "task_ref": "KT-319",
            "decision": "approve",
            "reviewed_head_sha": head,
            "dod_verifications": [],
        })
        .to_string();

        let outcome = decide_review(&db, &exec_id, &decision, "ClaudeCode", "sess-b")
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            ReviewOutcome::ApproveBlocked {
                reason: ApproveBlockReason::ReviewEvidenceInvalid(ref detail)
            } if detail.contains("must cover every DoD item")
        ));
        assert!(review_row(&db, &exec_id).await.is_none());
    }

    #[tokio::test]
    async fn principal_cannot_approve_a_dod_it_marked_unmet() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, _child, exec_id, head, _path) =
            delivered_awaiting_review(&db, repo.path()).await;
        let dod_id = dod_id_for_execution(&db, &exec_id).await;
        let decision = serde_json::json!({
            "version": "1",
            "task_ref": "KT-319",
            "decision": "approve",
            "reviewed_head_sha": head,
            "dod_verifications": [{
                "dod_id": dod_id,
                "met": false,
                "evidence": "principal validation failed"
            }],
        })
        .to_string();

        let outcome = decide_review(&db, &exec_id, &decision, "ClaudeCode", "sess-b")
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            ReviewOutcome::ApproveBlocked {
                reason: ApproveBlockReason::DodNotMet { ref unmet }
            } if unmet == &vec![dod_id]
        ));
        assert!(review_row(&db, &exec_id).await.is_none());
    }

    #[tokio::test]
    async fn approval_requires_the_delivered_sha_in_the_review_decision() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, _child, exec_id, _head, _path) =
            delivered_awaiting_review(&db, repo.path()).await;
        let dod_id = dod_id_for_execution(&db, &exec_id).await;

        let missing = serde_json::json!({
            "version": "1", "task_ref": "KT-1", "decision": "approve",
            "dod_verifications": [{
                "dod_id": dod_id.clone(),
                "met": true,
                "evidence": "principal reviewed the delivered changes"
            }]
        })
        .to_string();
        let outcome = decide_review(&db, &exec_id, &missing, "ClaudeCode", "sess-b")
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            ReviewOutcome::ApproveBlocked {
                reason: ApproveBlockReason::ReviewedHeadMismatch { ref reviewed, .. }
            } if reviewed == "missing"
        ));

        let wrong = serde_json::json!({
            "version": "1", "task_ref": "KT-1", "decision": "approve",
            "reviewed_head_sha": "deadbeef",
            "dod_verifications": [{
                "dod_id": dod_id,
                "met": true,
                "evidence": "principal reviewed the delivered changes"
            }]
        })
        .to_string();
        let outcome = decide_review(&db, &exec_id, &wrong, "ClaudeCode", "sess-b")
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            ReviewOutcome::ApproveBlocked {
                reason: ApproveBlockReason::ReviewedHeadMismatch { .. }
            }
        ));
        assert!(review_row(&db, &exec_id).await.is_none());
    }

    /// KT-388: the transport boundary must consume the durable Approved
    /// checkpoint immediately. Merely returning Approved strands the execution:
    /// no public MCP action exists to start the protected integration afterward.
    #[tokio::test]
    async fn approved_review_continues_through_protected_integration() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, _child, exec_id, _head, _path) =
            delivered_awaiting_review(&db, repo.path()).await;

        let reviewed = decide_review(
            &db,
            &exec_id,
            &review_approve(&db, &exec_id).await,
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        let completed = continue_approved_review(&db, reviewed).await.unwrap();

        match completed {
            ReviewOutcome::Reviewed { verdict, execution } => {
                assert_eq!(verdict, ReviewVerdict::Approve);
                assert_eq!(execution.status, TaskExecutionStatus::Done);
                assert!(execution.integrated_sha.is_some());
            }
            other => panic!("expected integrated review, got {other:?}"),
        }
        assert_eq!(
            exec_of(&db, &exec_id).await.status,
            TaskExecutionStatus::Done
        );
    }

    /// DoD-5, the distinguishing case: a worker may deliver an ABBREVIATED head_sha; approve
    /// must NOT false-refuse it. resolve_commit normalizes short↔long on both sides, so a
    /// 8-char delivered sha compares equal to the full worktree HEAD.
    #[tokio::test]
    async fn approve_accepts_an_abbreviated_delivered_sha() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_task_ref, parent_id, _child, exec_id) = attached_cli_worker(&db, repo.path()).await;
        let path = managed_worktree_path(&db, &exec_id).await;
        let head = git_rev(Path::new(&path), "HEAD");
        let short = head[..8].to_string();
        assert_ne!(short, head, "the abbreviated sha differs from the full one");

        deliver_worker_manifest(
            &db,
            &exec_id,
            "ClaudeCode",
            "sess-a",
            &manifest_json_with_files_for_dod(
                &short,
                serde_json::json!([]),
                &dod_id_for_execution(&db, &exec_id).await,
                true,
            ),
        )
        .await
        .unwrap();
        seed_cli_session(&db, 102, &parent_id, "sess-b").await;

        // The stored delivered head is the SHORT form — proving the normalization does the work.
        let stored = db
            .with_conn({
                let e = exec_id.clone();
                move |conn| crate::db::worker_deliveries::get_delivery(conn, &e, 0)
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.head_sha, short);

        let outcome = decide_review(
            &db,
            &exec_id,
            &review_approve(&db, &exec_id).await,
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        assert!(
            matches!(
                outcome,
                ReviewOutcome::Reviewed {
                    verdict: ReviewVerdict::Approve,
                    ..
                }
            ),
            "an abbreviated sha equal to HEAD must approve, not drift-refuse: {outcome:?}"
        );
    }

    /// DoD-5: approve is refused when the worktree HEAD moved since delivery, and NOTHING
    /// moves — a refused approve leaves the execution AwaitingReview with no review row.
    #[tokio::test]
    async fn approve_is_refused_when_head_drifted() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, _child, exec_id, _head, path) =
            delivered_awaiting_review(&db, repo.path()).await;

        // The worker advances the branch after delivery → the reviewed state drifted.
        git(
            Path::new(&path),
            &["commit", "--allow-empty", "-m", "drift"],
        );

        let outcome = decide_review(
            &db,
            &exec_id,
            &review_approve(&db, &exec_id).await,
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        match outcome {
            ReviewOutcome::ApproveBlocked {
                reason: ApproveBlockReason::HeadDrifted { delivered, current },
            } => assert_ne!(delivered, current, "the two shas differ"),
            other => panic!("expected ApproveBlocked/HeadDrifted, got {other:?}"),
        }
        assert_eq!(
            exec_of(&db, &exec_id).await.status,
            TaskExecutionStatus::AwaitingReview,
            "a refused approve does not move the execution"
        );
        assert!(
            review_row(&db, &exec_id).await.is_none(),
            "no review persisted on refusal"
        );
    }

    #[tokio::test]
    async fn approve_is_refused_when_the_worktree_became_dirty_after_delivery() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, _child, exec_id, _head, path) =
            delivered_awaiting_review(&db, repo.path()).await;
        std::fs::write(Path::new(&path).join("late-uncommitted.txt"), "late\n").unwrap();

        let outcome = decide_review(
            &db,
            &exec_id,
            &review_approve(&db, &exec_id).await,
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        match outcome {
            ReviewOutcome::ApproveBlocked {
                reason: ApproveBlockReason::WorktreeDirty { files },
            } => assert!(
                files
                    .iter()
                    .any(|file| file.contains("late-uncommitted.txt")),
                "the refusal must name the dirty path: {files:?}"
            ),
            other => panic!("expected ApproveBlocked/WorktreeDirty, got {other:?}"),
        }
        assert_eq!(
            exec_of(&db, &exec_id).await.status,
            TaskExecutionStatus::AwaitingReview
        );
        assert!(review_row(&db, &exec_id).await.is_none());
    }

    #[tokio::test]
    async fn cancellation_ack_requires_the_exact_registry_guard_to_finish() {
        let registry = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let guard = crate::CancelGuard::insert(&registry, "dispatch-under-test");
        guard.token.cancel();

        assert!(
            !wait_for_cancelled_dispatches_to_settle(
                &registry,
                &["dispatch-under-test".into()],
                std::time::Duration::from_millis(10),
            )
            .await,
            "a cancelled token is not proof that its runtime has finished"
        );

        drop(guard);
        assert!(
            wait_for_cancelled_dispatches_to_settle(
                &registry,
                &["dispatch-under-test".into()],
                std::time::Duration::from_millis(10),
            )
            .await,
            "dropping the exact runtime guard acknowledges termination"
        );
    }

    #[tokio::test]
    async fn cancel_execution_receipt_reports_signal_and_runtime_ack_without_touching_a_peer() {
        let repo = init_repo();
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("cancel-receipt-runtime-ack".into()),
            },
        )
        .await
        .unwrap();
        let worktree_path = managed_worktree_path(&db, &execution.id).await;
        let dispatch_id = execution.dispatch_job_id.clone().unwrap();
        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );

        let worker_guard = crate::CancelGuard::insert(&state.cancel_registry, dispatch_id);
        let worker_token = worker_guard.token.clone();
        let peer_guard = crate::CancelGuard::insert(&state.cancel_registry, "peer-dispatch");
        let peer_token = peer_guard.token.clone();
        let worker_finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_finished_in_task = worker_finished.clone();
        let worker = tokio::spawn(async move {
            worker_token.cancelled().await;
            worker_finished_in_task.store(true, std::sync::atomic::Ordering::SeqCst);
            drop(worker_guard);
        });

        let response = cancel_execution(
            State(state),
            Path(execution.id.clone()),
            Json(CancelExecutionRequest {
                reason: "test cancellation".into(),
                cleanup_policy: Some(crate::models::CancellationCleanupPolicy::Preserve),
            }),
        )
        .await
        .0;
        worker.await.unwrap();

        assert!(
            response.success,
            "cancellation must succeed: {:?}",
            response.error
        );
        let receipt = response.data.expect("cancellation receipt");
        assert_eq!(receipt.execution.status, TaskExecutionStatus::Cancelled);
        assert_eq!(receipt.cancellation_signal_sent, Some(true));
        assert_eq!(receipt.termination_confirmed, Some(true));
        assert!(receipt
            .outcome
            .as_deref()
            .is_some_and(|value| value.contains("policy=preserve")));
        assert!(worker_finished.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!peer_token.is_cancelled(), "a sibling dispatch is isolated");
        assert!(
            std::path::Path::new(&worktree_path).exists(),
            "Preserve keeps the worker checkout"
        );
        drop(peer_guard);
    }

    /// KT-396 — the regression the task names: a cancellation under `Preserve`
    /// keeps the worktree and its sources, and `target/` is reclaimed. Before
    /// the fix, cancellation was the one terminal path that never called the
    /// reclaim, and task-kt-377-49a08eeb kept its 39 MiB.
    #[tokio::test]
    async fn a_cancelled_preserved_worktree_loses_its_target_and_nothing_else() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_task, _parent, _child, exec_id) = attached_cli_worker(&db, repo.path()).await;
        let path = managed_worktree_path(&db, &exec_id).await;
        let worktree = std::path::Path::new(&path);

        // Build artefacts, and a source file that must survive.
        std::fs::create_dir_all(worktree.join("target/debug")).unwrap();
        std::fs::write(worktree.join("target/debug/kronn"), b"artefact").unwrap();
        std::fs::write(worktree.join("work-in-progress.rs"), "fn wip() {}\n").unwrap();

        // Cancel the tree exactly as the handler does, then settle the workspace.
        let eid = exec_id.clone();
        db.with_conn(move |conn| {
            crate::db::orchestration::cancel_execution_tree(conn, &eid, "test", &backend_actor())
        })
        .await
        .unwrap();
        let workspace = {
            let eid = exec_id.clone();
            db.with_conn(move |conn| {
                crate::db::discussion_workspaces::get_managed_for_execution(conn, &eid)
            })
            .await
            .unwrap()
        };
        let outcome = settle_cancelled_workspace(
            &db,
            &exec_id,
            crate::models::CancellationCleanupPolicy::Preserve,
            workspace,
            Some(repo.path().to_string_lossy().to_string()),
        )
        .await;
        assert!(outcome.contains("policy=preserve"), "{outcome}");

        // The policy's promise, then the fix's: sources and worktree stay,
        // target/ is gone.
        assert!(
            worktree.exists(),
            "a Preserve cancellation keeps the worktree"
        );
        assert!(
            worktree.join("work-in-progress.rs").exists(),
            "and the sources in it"
        );
        assert!(
            !worktree.join("target").exists(),
            "but the rebuildable artefacts are reclaimed"
        );

        // Every attempt is audited, success included.
        let audited: i64 = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM task_execution_events \
                     WHERE action IN ('build_artifacts_reclaimed', 'build_artifacts_refused')",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert!(audited >= 1, "the reclaim must leave a durable event");
    }

    /// KT-402 — the undelivered-worker notice's deterministic id exists so a
    /// resume can never double-post, but the insert also has to tolerate its
    /// own replay: a stream timeout failed the dispatch, the watchdog requeued
    /// it, the requeue failed too, and the second settlement died on
    /// UNIQUE(messages.id), leaving the dispatch row unsettled.
    #[tokio::test]
    async fn a_second_undelivered_settlement_is_idempotent_not_a_unique_violation() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("kt402-idempotent-settlement".into()),
            },
        )
        .await
        .unwrap();
        let exec_id = execution.id.clone();
        let child_id = execution.sub_discussion_id.clone().unwrap();
        let dispatch_id = execution.dispatch_job_id.clone().unwrap();
        // The provisioned execution starts Working with a live dispatch — the
        // exact state a stream timeout finds it in.
        assert_eq!(
            exec_of(&db, &exec_id).await.status,
            TaskExecutionStatus::Working
        );

        async fn settle(db: &Database, job: String, disc: String) -> anyhow::Result<()> {
            db.with_conn(move |conn| {
                crate::api::discussions::runtime::persist_dispatch_settlement(
                    conn,
                    &job,
                    &disc,
                    None,
                    crate::db::workflows::BatchChildOutcome::Failed,
                    Some("stream timeout"),
                    None,
                )?;
                Ok(())
            })
            .await
        }
        settle(&db, dispatch_id.clone(), child_id.clone())
            .await
            .expect("first settlement");
        assert_eq!(
            exec_of(&db, &exec_id).await.status,
            TaskExecutionStatus::Interrupted
        );

        // The watchdog requeues: the dispatch runs again, the execution resumes.
        let (job, eid) = (dispatch_id.clone(), exec_id.clone());
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE agent_dispatch_jobs SET status = 'Running' WHERE id = ?1",
                [&job],
            )?;
            crate::db::orchestration::transition_execution(
                conn,
                &eid,
                TaskExecutionStatus::Working,
                &backend_actor(),
                serde_json::json!({ "recovery": "test requeue" }),
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // The requeued run fails the same way. This second settlement used to
        // die on the UNIQUE violation.
        settle(&db, dispatch_id.clone(), child_id.clone())
            .await
            .expect("a replayed settlement must settle, not violate UNIQUE");

        let posted: i64 = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM messages WHERE id LIKE 'orch-worker-undelivered:%'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(posted, 1, "one notice, never two, never zero");
    }

    /// DoD-5: approve is refused when no manifest is persisted for the current attempt.
    #[tokio::test]
    async fn approve_is_refused_without_a_manifest() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, _child, exec_id, _head, _path) =
            delivered_awaiting_review(&db, repo.path()).await;
        // Simulate a lost manifest for the current attempt.
        let e = exec_id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "DELETE FROM task_execution_deliveries WHERE task_execution_id = ?1 AND attempt_no = 0",
                [&e],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let outcome = decide_review(
            &db,
            &exec_id,
            &review_approve(&db, &exec_id).await,
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        assert!(
            matches!(
                outcome,
                ReviewOutcome::ApproveBlocked {
                    reason: ApproveBlockReason::NoManifest
                }
            ),
            "no manifest → NoManifest, got {outcome:?}"
        );
        assert_eq!(
            exec_of(&db, &exec_id).await.status,
            TaskExecutionStatus::AwaitingReview
        );
    }

    /// DoD-5: even when self-review is explicitly enabled, a worker cannot approve a manifest
    /// that reports a DoD as unmet. Only a distinct principal may supply overriding evidence.
    #[tokio::test]
    async fn approve_is_refused_when_a_dod_is_not_met() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_task_ref, _parent_id, _child, exec_id) = attached_cli_worker(&db, repo.path()).await;
        let path = managed_worktree_path(&db, &exec_id).await;
        let head = git_rev(Path::new(&path), "HEAD");
        deliver_worker_manifest(
            &db,
            &exec_id,
            "ClaudeCode",
            "sess-a",
            &manifest_json_unmet(&db, &exec_id, &head).await,
        )
        .await
        .unwrap();
        let e = exec_id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE orchestration_runs SET allow_self_review = 1 \
                 WHERE id = (SELECT orchestration_run_id FROM task_executions WHERE id = ?1)",
                [&e],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let worker_decision = serde_json::json!({
            "version": "1",
            "task_ref": "KT-319",
            "decision": "approve",
            "reviewed_head_sha": head,
        })
        .to_string();
        let outcome = decide_review(&db, &exec_id, &worker_decision, "ClaudeCode", "sess-a")
            .await
            .unwrap();
        match outcome {
            ReviewOutcome::ApproveBlocked {
                reason: ApproveBlockReason::DodNotMet { unmet },
            } => assert_eq!(unmet, vec![dod_id_for_execution(&db, &exec_id).await]),
            other => panic!("expected ApproveBlocked/DodNotMet, got {other:?}"),
        }
        assert_eq!(
            exec_of(&db, &exec_id).await.status,
            TaskExecutionStatus::AwaitingReview
        );
    }

    /// An HTTP worker may honestly leave a shell-owned DoD unmet. Only evidence
    /// submitted by the authorized principal for this review and SHA can satisfy it.
    #[tokio::test]
    async fn a_dod_verified_by_the_principal_unblocks_approval() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_task_ref, parent_id, _child, exec_id) = attached_cli_worker(&db, repo.path()).await;
        let path = managed_worktree_path(&db, &exec_id).await;
        let head = git_rev(Path::new(&path), "HEAD");
        deliver_worker_manifest(
            &db,
            &exec_id,
            "ClaudeCode",
            "sess-a",
            &manifest_json_unmet(&db, &exec_id, &head).await,
        )
        .await
        .unwrap();
        seed_cli_session(&db, 102, &parent_id, "sess-b").await;

        let dod_id = dod_id_for_execution(&db, &exec_id).await;
        let decision = serde_json::json!({
            "version": "1",
            "task_ref": "KT-1",
            "decision": "approve",
            "reviewed_head_sha": head,
            "dod_verifications": [{
                "dod_id": dod_id,
                "met": true,
                "evidence": "cargo test --lib: exit 0"
            }]
        })
        .to_string();
        let outcome = decide_review(&db, &exec_id, &decision, "ClaudeCode", "sess-b")
            .await
            .unwrap();
        match outcome {
            ReviewOutcome::Reviewed { verdict, execution } => {
                assert_eq!(verdict, ReviewVerdict::Approve);
                assert_eq!(execution.status, TaskExecutionStatus::Approved);
            }
            other => panic!("a principal-verified DoD must not block approval, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_live_planning_checkbox_cannot_replace_attempt_scoped_review_evidence() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_task_ref, parent_id, _child, exec_id) = attached_cli_worker(&db, repo.path()).await;
        let path = managed_worktree_path(&db, &exec_id).await;
        let head = git_rev(Path::new(&path), "HEAD");
        let dod_id = dod_id_for_execution(&db, &exec_id).await;
        deliver_worker_manifest(
            &db,
            &exec_id,
            "ClaudeCode",
            "sess-a",
            &manifest_json_unmet(&db, &exec_id, &head).await,
        )
        .await
        .unwrap();
        seed_cli_session(&db, 102, &parent_id, "sess-b").await;

        let task_id = exec_of(&db, &exec_id).await.task_id;
        let checked_id = dod_id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE planning_task_dod_items SET completed = 1 WHERE task_id = ?1 AND id = ?2",
                rusqlite::params![task_id, checked_id],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let decision = serde_json::json!({
            "version": "1",
            "task_ref": "KT-319",
            "decision": "approve",
            "reviewed_head_sha": head,
            "dod_verifications": [],
        })
        .to_string();
        let outcome = decide_review(&db, &exec_id, &decision, "ClaudeCode", "sess-b")
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            ReviewOutcome::ApproveBlocked {
                reason: ApproveBlockReason::ReviewEvidenceInvalid { .. }
            }
        ));
    }

    /// DoD-7: the worker cannot self-approve by default → SelfReviewForbidden and nothing
    /// moves; the SAME call passes once the run explicitly allows self-review.
    #[tokio::test]
    async fn worker_cannot_self_approve_unless_the_run_allows_it() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, _child, exec_id, head, _path) =
            delivered_awaiting_review(&db, repo.path()).await;
        let worker_decision = serde_json::json!({
            "version": "1",
            "task_ref": "KT-319",
            "decision": "approve",
            "reviewed_head_sha": head,
        })
        .to_string();

        // The WORKER (session 101, now in the child) tries to approve its own work.
        let refused = decide_review(&db, &exec_id, &worker_decision, "ClaudeCode", "sess-a")
            .await
            .unwrap();
        assert!(
            matches!(refused, ReviewOutcome::SelfReviewForbidden),
            "worker self-approve is forbidden by default, got {refused:?}"
        );
        assert_eq!(
            exec_of(&db, &exec_id).await.status,
            TaskExecutionStatus::AwaitingReview,
            "the forbidden self-review moved nothing"
        );
        assert!(review_row(&db, &exec_id).await.is_none());

        // Explicit policy: the run allows self-review → the exact same call now passes.
        let e = exec_id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE orchestration_runs SET allow_self_review = 1 \
                 WHERE id = (SELECT orchestration_run_id FROM task_executions WHERE id = ?1)",
                [&e],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let allowed = decide_review(&db, &exec_id, &worker_decision, "ClaudeCode", "sess-a")
            .await
            .unwrap();
        assert!(
            matches!(
                allowed,
                ReviewOutcome::Reviewed {
                    verdict: ReviewVerdict::Approve,
                    ..
                }
            ),
            "with allow_self_review the worker may self-approve, got {allowed:?}"
        );
    }

    #[tokio::test]
    async fn an_allowed_worker_self_review_cannot_forge_principal_dod_evidence() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_task_ref, _parent, _child, exec_id) = attached_cli_worker(&db, repo.path()).await;
        let path = managed_worktree_path(&db, &exec_id).await;
        let head = git_rev(Path::new(&path), "HEAD");
        let dod_id = dod_id_for_execution(&db, &exec_id).await;
        deliver_worker_manifest(
            &db,
            &exec_id,
            "ClaudeCode",
            "sess-a",
            &manifest_json_unmet(&db, &exec_id, &head).await,
        )
        .await
        .unwrap();
        let execution_id = exec_id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE orchestration_runs SET allow_self_review = 1 \
                 WHERE id = (SELECT orchestration_run_id FROM task_executions WHERE id = ?1)",
                [&execution_id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let decision = serde_json::json!({
            "version": "1", "task_ref": "KT-1", "decision": "approve",
            "reviewed_head_sha": head,
            "dod_verifications": [{
                "dod_id": dod_id,
                "met": true,
                "evidence": "I claim the principal ran it"
            }]
        })
        .to_string();

        let outcome = decide_review(&db, &exec_id, &decision, "ClaudeCode", "sess-a")
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            ReviewOutcome::ApproveBlocked {
                reason: ApproveBlockReason::ReviewEvidenceInvalid(_)
            }
        ));
        assert!(review_row(&db, &exec_id).await.is_none());
    }

    /// DoD-4: request_changes keeps the sub-discussion + worktree, bumps the round, and hands
    /// structured findings to the worker in the CHILD (Cli-targeted, zero dispatch).
    #[tokio::test]
    async fn request_changes_keeps_worktree_bumps_round_and_hands_findings_to_the_worker() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, child_id, exec_id, head, path) =
            delivered_awaiting_review(&db, repo.path()).await;
        let rounds_before = exec_of(&db, &exec_id).await.review_rounds;

        let outcome = decide_review(
            &db,
            &exec_id,
            &review_request_changes("Corrige la garde manquante"),
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        match outcome {
            ReviewOutcome::Reviewed { verdict, execution } => {
                assert_eq!(verdict, ReviewVerdict::RequestChanges);
                // A CLI worker is re-activated (DoD-9): request_changes flips through
                // ChangesRequested and parks Blocked(awaiting_worker_acceptance) for the re-offer.
                assert_eq!(execution.status, TaskExecutionStatus::Blocked);
                assert_eq!(
                    execution.blocked_reason_code,
                    Some(crate::models::BlockedReasonCode::AwaitingWorkerAcceptance),
                    "the re-offer window is a visible, coded Blocked, never a silent wait"
                );
                assert_eq!(
                    execution.review_rounds,
                    rounds_before + 1,
                    "the round is bumped"
                );
            }
            other => panic!("expected Reviewed/RequestChanges, got {other:?}"),
        }

        // The findings landed in the CHILD, targeted to the worker (Cli 101), with ZERO dispatch.
        let e = exec_id.clone();
        let (disc, targets, dispatches) = db
            .with_conn(move |conn| {
                let fid = format!("orch-review-findings:{e}:0");
                let disc: String = conn.query_row(
                    "SELECT discussion_id FROM messages WHERE id = ?1",
                    [&fid],
                    |r| r.get(0),
                )?;
                let targets = crate::db::discussions::list_message_targets(conn, &fid)?;
                let d: i64 =
                    conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |r| r.get(0))?;
                Ok((disc, targets, d))
            })
            .await
            .unwrap();
        assert_eq!(
            disc, child_id,
            "findings posted in the child, not the parent"
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, MessageTargetKind::Cli);
        assert_eq!(
            targets[0].cli_session_id,
            Some(101),
            "targeted at the worker session"
        );
        assert_eq!(
            dispatches, 0,
            "no native dispatch — the worker is woken via wait_for_peer (KT-330)"
        );

        // The worktree is UNTOUCHED (DoD-4): its HEAD is unchanged.
        assert_eq!(
            git_rev(Path::new(&path), "HEAD"),
            head,
            "request_changes never touches the worktree"
        );
        assert_eq!(
            review_row(&db, &exec_id).await.unwrap().decision,
            "request_changes"
        );
    }

    /// KT-385: a native worker cannot be woken through wait_for_peer. Its rework
    /// therefore advances the attempt and queues a new durable dispatch in the
    /// same transaction as request_changes. This is the real Ollama pilot path.
    #[tokio::test]
    async fn request_changes_redispatches_a_native_worker_as_a_new_attempt() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, child_id, exec_id, _head, _path) =
            delivered_awaiting_review(&db, repo.path()).await;
        let e = exec_id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE task_executions SET worker_target_kind = 'agent', \
                     worker_cli_session_id = NULL, worker_agent_type = 'Ollama' WHERE id = ?1",
                [&e],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let outcome = decide_review(
            &db,
            &exec_id,
            &review_request_changes("commit puis relivre"),
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        let execution = match outcome {
            ReviewOutcome::Reviewed {
                verdict: ReviewVerdict::RequestChanges,
                execution,
            } => execution,
            other => panic!("expected native rework, got {other:?}"),
        };
        assert_eq!(execution.status, TaskExecutionStatus::Working);
        assert_eq!(execution.attempt_no, 1);
        let dispatch_id = execution.dispatch_job_id.expect("fresh dispatch attached");

        let (status, dedupe, discussion_id, trigger_message_id, target) = db
            .with_conn(move |conn| {
                let job = crate::db::agent_dispatch::get(conn, &dispatch_id)?
                    .context("native rework dispatch vanished")?;
                let target =
                    crate::db::discussions::list_message_targets(conn, &job.trigger_message_id)?
                        .into_iter()
                        .next()
                        .context("findings lost their worker target")?;
                Ok((
                    job.status,
                    job.dedupe_key,
                    job.discussion_id,
                    job.trigger_message_id,
                    target,
                ))
            })
            .await
            .unwrap();
        assert_eq!(status, crate::db::agent_dispatch::DispatchStatus::Pending);
        assert_eq!(dedupe, format!("orch-rework:{exec_id}:1"));
        assert_eq!(discussion_id, child_id);
        assert_eq!(
            trigger_message_id,
            format!("orch-review-findings:{exec_id}:0")
        );
        assert_eq!(target.kind, MessageTargetKind::Agent);
        assert_eq!(target.agent_type, AgentType::Ollama);
    }

    /// Both handoffs — the wake and the reassignment — must match what the
    /// execution has actually produced. The reassignment wording used to
    /// declare the manifests authoritative unconditionally, which sent a worker
    /// that had delivered nothing back to inventory an empty state.
    #[test]
    fn a_handoff_matches_what_the_execution_has_produced() {
        let started_over = handoff_notice(false, None);
        assert!(
            started_over.starts_with("**Démarre la tâche**"),
            "{started_over}"
        );
        assert!(
            !started_over.contains("autoritatifs"),
            "nothing is authoritative yet: {started_over}"
        );
        assert!(
            started_over.contains("offset"),
            "the slice hint must survive: {started_over}"
        );

        let resumed = handoff_notice(true, None);
        assert!(resumed.starts_with("**Reprise**"), "{resumed}");
        assert!(resumed.contains("l'étape inachevée"), "{resumed}");

        let reassigned_cold = handoff_notice(false, Some(4));
        assert!(
            reassigned_cold.starts_with("**Démarre la tâche — génération 4**"),
            "{reassigned_cold}"
        );
        assert!(
            !reassigned_cold.contains("Ne recommence que l'étape inachevée"),
            "generation 4 of a delivery-less execution has no unfinished step: {reassigned_cold}"
        );

        let reassigned_warm = handoff_notice(true, Some(4));
        assert!(
            reassigned_warm.starts_with("**Reprise — génération 4**"),
            "{reassigned_warm}"
        );
        assert!(
            reassigned_warm.contains("autoritatifs"),
            "{reassigned_warm}"
        );
    }

    #[tokio::test]
    async fn handoff_with_dirty_worktree_tells_worker_to_commit() {
        // KT-404: dirty worktree → explicit message to review diff and commit
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let exec = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("kt-404-dirty".into()),
            },
        )
        .await
        .unwrap();

        // Dirty the exact managed workspace persisted by provisioning. Recomputing
        // its layout here can point at a sibling path and make this regression test
        // pass without exercising the handoff inspection at all.
        let workspace_exec_id = exec.id.clone();
        let workspace_path = db
            .with_conn(move |conn| {
                let workspace = crate::db::discussion_workspaces::get_managed_for_execution(
                    conn,
                    &workspace_exec_id,
                )?
                .context("managed workspace missing")?;
                workspace.canonical_path.context("canonical path missing")
            })
            .await
            .expect("managed workspace must exist after provisioning");
        std::fs::write(
            std::path::Path::new(&workspace_path).join("dirty.txt"),
            "uncommitted",
        )
        .unwrap();

        let exec_id = exec.id.clone();
        let msg = db
            .with_conn(move |conn| {
                Ok(handoff_notice_with_context(
                    false,
                    None,
                    Some(conn),
                    Some(&exec_id),
                ))
            })
            .await
            .unwrap();

        assert!(
            msg.contains("Du travail non commité"),
            "must name dirty state: {msg}"
        );
        assert!(msg.contains("git_diff"), "must ask to review diff");
        assert!(msg.contains("git_commit"), "must ask to commit");
        assert!(
            !msg.contains("aucun travail n'a"),
            "must NOT claim empty state: {msg}"
        );
    }

    #[tokio::test]
    async fn handoff_with_clean_worktree_uses_generic_message() {
        // KT-404: clean worktree → standard generic message about workflow
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let exec = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("kt-404-clean".into()),
            },
        )
        .await
        .unwrap();

        let exec_id = exec.id.clone();
        let msg = db
            .with_conn(move |conn| {
                Ok(handoff_notice_with_context(
                    false,
                    None,
                    Some(conn),
                    Some(&exec_id),
                ))
            })
            .await
            .unwrap();

        assert!(
            msg.contains("Aucun travail n'a encore été enregistré"),
            "clean → generic message"
        );
        assert!(msg.contains("search_text"), "must include workflow hints");
        assert!(msg.contains("offset"), "must include offset/limit hint");
        assert!(
            !msg.contains("Du travail non commité"),
            "must NOT claim dirty"
        );
    }

    #[tokio::test]
    async fn handoff_with_git_inspection_failure_uses_conservative_message() -> anyhow::Result<()> {
        // KT-404: git inspection fails (e.g., worktree path invalid) → conservative message
        // that does NOT claim state is empty, but asks worker to verify with git status/diff
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;

        // Provision an execution with a deliberately invalid canonical_path
        let exec = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("kt-404-invalid-path".into()),
            },
        )
        .await
        .unwrap();

        let exec_id = exec.id.clone();

        // Update the worktree's canonical_path to a non-existent path
        let nonexistent_path = format!("/nonexistent/kt404/test/{}", uuid::Uuid::new_v4());
        let exec_id_for_update = exec_id.clone();
        db.with_conn(move |conn| {
            use rusqlite::params;
            conn.execute(
                "UPDATE discussion_workspaces SET canonical_path = ?1 \
                 WHERE task_execution_id = ?2",
                params![&nonexistent_path, &exec_id_for_update],
            )?;
            Ok(())
        })
        .await?;

        let msg = db
            .with_conn(move |conn| {
                Ok(handoff_notice_with_context(
                    false,
                    None,
                    Some(conn),
                    Some(&exec_id),
                ))
            })
            .await
            .unwrap();

        // Verify the conservative message about inspection failure
        assert!(
            msg.contains("n'a pas pu vérifier complètement son état"),
            "must mention inspection failure: {msg}"
        );
        assert!(
            msg.contains("git status") && msg.contains("git diff"),
            "must ask to verify with git status and diff: {msg}"
        );
        assert!(
            !msg.contains("Aucun travail n'a encore été enregistré"),
            "must NOT claim empty state during inspection failure: {msg}"
        );
        Ok(())
    }

    #[test]
    fn handoff_with_no_database_context_uses_generic_message() {
        // KT-404: no database context (None) → generic message without claiming empty
        let msg = handoff_notice_with_context(false, None, None, None);

        assert!(
            msg.contains("Aucun travail n'a encore été enregistré"),
            "generic fallback"
        );
        assert!(msg.contains("search_text"), "must include workflow hints");
        assert!(msg.contains("offset"), "must include the slice hint");
    }

    #[test]
    fn handoff_with_generation_maintains_suffix() {
        // The generation suffix must be preserved in all cases.
        let msg_gen3 = handoff_notice_with_context(false, Some(3), None, None);
        assert!(
            msg_gen3.contains("— génération 3"),
            "generation suffix must appear: {msg_gen3}"
        );
    }

    #[tokio::test]
    async fn native_reassignment_delivers_the_principals_reason_to_the_worker() {
        let repo = init_repo();
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("reassign-reason-visible".into()),
            },
        )
        .await
        .unwrap();
        let execution_id = execution.id.clone();
        let child = execution.sub_discussion_id.clone().unwrap();
        let legacy_child = child.clone();
        let dispatch = execution.dispatch_job_id.clone().unwrap();
        let dispatch_for_token = dispatch.clone();
        let interrupted_id = execution_id.clone();
        db.with_conn(move |conn| {
            crate::db::agent_dispatch::mark_completed(conn, &dispatch)?;
            conn.execute(
                "UPDATE discussions SET pin_first_message = 0 WHERE id = ?1",
                [&legacy_child],
            )?;
            crate::db::orchestration::transition_execution(
                conn,
                &interrupted_id,
                TaskExecutionStatus::Interrupted,
                &backend_actor(),
                serde_json::json!({}),
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let replaced_worker_token = tokio_util::sync::CancellationToken::new();
        state
            .cancel_registry
            .lock()
            .unwrap()
            .insert(dispatch_for_token, replaced_worker_token.clone());
        let reason = "Le HEAD existe déjà : ne réexplore pas, livre uniquement le manifeste.";
        reassign_native_execution(
            &state,
            &execution_id,
            crate::models::CampaignWorkerSelection {
                target: MessageTarget::discussion_agent(AgentType::Ollama),
                model: Some("qwen3.8:27b-mlx".into()),
                profile_id: Some("profile-local-worker".into()),
            },
            reason,
        )
        .await
        .unwrap();

        let (message, worker_room) = db
            .with_conn(move |conn| {
                let message = conn.query_row(
                    "SELECT content FROM messages
                      WHERE discussion_id = ?1 AND id LIKE 'orch-reassign:%'
                      ORDER BY sort_order DESC LIMIT 1",
                    [&child],
                    |row| row.get::<_, String>(0),
                )?;
                let room = crate::db::discussions::get_discussion(conn, &child)?
                    .context("reassigned child room missing")?;
                Ok((message, room))
            })
            .await
            .unwrap();
        assert!(
            message.contains("Consigne du principal pour cette réaffectation"),
            "{message}"
        );
        assert!(message.contains(reason), "{message}");
        assert_eq!(worker_room.agent, AgentType::Ollama);
        assert_eq!(worker_room.participants, vec![AgentType::Ollama]);
        assert_eq!(worker_room.tier, ModelTier::Default);
        assert_eq!(worker_room.model.as_deref(), Some("qwen3.8:27b-mlx"));
        assert_eq!(worker_room.profile_ids, vec!["profile-local-worker"]);
        assert!(worker_room.pin_first_message);
        assert!(
            replaced_worker_token.is_cancelled(),
            "the superseded dispatch must not keep running after commit"
        );
    }

    #[tokio::test]
    async fn native_reassignment_resumes_an_escalated_worker_only_after_the_explicit_call() {
        let repo = init_repo();
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("reassign-escalated-worker".into()),
            },
        )
        .await
        .unwrap();
        let execution_id = execution.id.clone();
        let child = execution.sub_discussion_id.clone().unwrap();
        let workspace_id = execution.workspace_id.clone();
        let old_dispatch = execution.dispatch_job_id.clone().unwrap();
        let escalated_id = execution_id.clone();
        let child_for_count = child.clone();
        db.with_conn(move |conn| {
            crate::db::agent_dispatch::mark_failed(
                conn,
                &old_dispatch,
                "deterministic provider failure",
            )?;
            crate::db::orchestration::transition_execution(
                conn,
                &escalated_id,
                TaskExecutionStatus::Escalated,
                &backend_actor(),
                serde_json::json!({ "failure_kind": "worker_failed_without_delivery" }),
            )?;
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM agent_dispatch_jobs WHERE discussion_id = ?1",
                [&child_for_count],
                |row| row.get(0),
            )?;
            assert_eq!(count, 1, "escalation itself cannot retry the worker");
            Ok(())
        })
        .await
        .unwrap();

        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        reassign_native_execution(
            &state,
            &execution_id,
            crate::models::CampaignWorkerSelection {
                target: MessageTarget::discussion_agent(AgentType::Ollama),
                model: Some("qwen3.6:35b-mlx".into()),
                profile_id: Some("profile-local-worker".into()),
            },
            "explicit principal fallback",
        )
        .await
        .unwrap();

        let resumed_id = execution_id.clone();
        db.with_conn(move |conn| {
            let resumed = crate::db::orchestration::get_task_execution(conn, &resumed_id)?
                .context("resumed execution missing")?;
            assert_eq!(resumed.status, TaskExecutionStatus::Working);
            assert_eq!(resumed.sub_discussion_id.as_deref(), Some(child.as_str()));
            assert_eq!(resumed.workspace_id, workspace_id);
            assert_eq!(resumed.worker_agent_type.as_deref(), Some("Ollama"));
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM agent_dispatch_jobs WHERE discussion_id = ?1",
                [&child],
                |row| row.get(0),
            )?;
            assert_eq!(count, 2, "the explicit call queues exactly one replacement");
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn native_reassignment_rolls_back_when_the_child_room_vanished() {
        let repo = init_repo();
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("reassign-missing-child-rollback".into()),
            },
        )
        .await
        .unwrap();
        let execution_id = execution.id.clone();
        let interrupted_id = execution_id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE task_executions SET sub_discussion_id = NULL WHERE id = ?1",
                [&interrupted_id],
            )?;
            crate::db::orchestration::transition_execution(
                conn,
                &interrupted_id,
                TaskExecutionStatus::Interrupted,
                &backend_actor(),
                serde_json::json!({}),
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let before_id = execution_id.clone();
        let before = db
            .with_conn(move |conn| {
                crate::db::orchestration::get_task_execution(conn, &before_id)?
                    .context("execution missing before failed reassignment")
            })
            .await
            .unwrap();
        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );

        let error = match reassign_native_execution(
            &state,
            &execution_id,
            crate::models::CampaignWorkerSelection {
                target: MessageTarget::discussion_agent(AgentType::Ollama),
                model: Some("qwen3.8:27b-mlx".into()),
                profile_id: Some("profile-local-worker".into()),
            },
            "must roll back",
        )
        .await
        {
            Ok(_) => panic!("reassignment must fail when its child room vanished"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("execution has no child discussion"),
            "{error:#}"
        );

        let after_id = execution_id.clone();
        let after = db
            .with_conn(move |conn| {
                crate::db::orchestration::get_task_execution(conn, &after_id)?
                    .context("execution missing after failed reassignment")
            })
            .await
            .unwrap();
        assert_eq!(after.worker_agent_type, before.worker_agent_type);
        assert_eq!(after.worker_model, before.worker_model);
        assert_eq!(after.worker_model_tier, before.worker_model_tier);
        assert_eq!(after.worker_profile_id, before.worker_profile_id);
        assert_eq!(after.dispatch_job_id, before.dispatch_job_id);
    }

    #[tokio::test]
    async fn a_resume_with_nothing_to_resume_tells_the_worker_to_start() {
        // KT-400 — the resume notice was written for a worker interrupted
        // mid-work. Handed to one that never started, it is an instruction to
        // audit: there is no unfinished step, nothing persisted is
        // authoritative, and "do not replay a proven action" reads as "go check
        // what is already done". Three consecutive real generations answered
        // with a status report and wrote no code.
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("resume-wording".into()),
            },
        )
        .await
        .unwrap();
        let exec_id = execution.id.clone();
        let child = execution.sub_discussion_id.clone().unwrap();
        let dispatch = execution.dispatch_job_id.clone().unwrap();

        // The worker finished without delivering: dispatch terminal, execution
        // interrupted, worktree untouched.
        let e = exec_id.clone();
        db.with_conn(move |conn| {
            crate::db::agent_dispatch::mark_completed(conn, &dispatch)?;
            crate::db::orchestration::transition_execution(
                conn,
                &e,
                TaskExecutionStatus::Interrupted,
                &backend_actor(),
                serde_json::json!({}),
            )?;
            Ok(())
        })
        .await
        .unwrap();

        wake_recovered_worker(&db, &exec_id).await.unwrap();

        let room = child.clone();
        let notice: String = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT content FROM messages WHERE discussion_id = ?1 \
                     AND content LIKE '%tâche%' OR content LIKE '%Reprise%' \
                     ORDER BY sort_order DESC LIMIT 1",
                    rusqlite::params![room],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();

        assert!(
            notice.contains("Démarre la tâche"),
            "a worker with no delivery must be told to start, got: {notice}"
        );
        assert!(
            !notice.contains("l'étape inachevée"),
            "there is no unfinished step to resume: {notice}"
        );
        // It is also told how to read a large file without drowning — the other
        // half of what stopped the real run.
        assert!(
            notice.contains("offset"),
            "the slice hint must survive: {notice}"
        );
    }

    #[tokio::test]
    async fn settled_worker_failure_survives_boot_classification_without_redispatch() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("settled-worker-failure".into()),
            },
        )
        .await
        .unwrap();
        let dispatch_id = execution.dispatch_job_id.clone().unwrap();
        db.with_conn(move |conn| {
            crate::db::agent_dispatch::mark_failed(
                conn,
                &dispatch_id,
                "deterministic CLI configuration failure",
            )?;
            let actor = crate::models::PlanningActor {
                kind: crate::models::PlanningActorKind::Backend,
                id: Some("test-settlement".into()),
                session_id: None,
                source_message_id: None,
            };
            crate::db::orchestration::interrupt_undelivered_execution_for_dispatch(
                conn,
                &dispatch_id,
                "worker_failed_without_delivery",
                &actor,
            )?
            .context("worker failure must interrupt the execution")?;
            Ok(())
        })
        .await
        .unwrap();

        // Reproduce several hot-reload boots. Classification may refresh a
        // pending checkpoint, but must never reinterpret Working-origin as
        // permission to create another provider call after a settled failure.
        for _ in 0..3 {
            classify_interrupted_execution(&db, &execution.id, &[AgentType::ClaudeCode])
                .await
                .unwrap();
        }

        let execution_id = execution.id.clone();
        db.with_conn(move |conn| {
            let recovery = crate::db::orchestration::get_execution_recovery(conn, &execution_id)?
                .context("human recovery checkpoint")?;
            assert_eq!(
                recovery.recovery_action,
                ExecutionRecoveryAction::AwaitHuman
            );
            assert_eq!(recovery.recovery_reason, "worker_failed_without_delivery");
            let dispatch_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM agent_dispatch_jobs WHERE discussion_id = ?1",
                [execution.sub_discussion_id.as_deref().unwrap()],
                |row| row.get(0),
            )?;
            assert_eq!(dispatch_count, 1, "no boot may enqueue a retry");
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn worker_recovery_requeues_terminal_dedupe_and_stays_idempotent_while_active() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (task_ref, parent_id, _) = seed(&db, repo.path()).await;
        let execution = provision_single_task_execution(
            &db,
            ProvisionInput {
                task_reference: task_ref,
                parent_discussion_id: parent_id,
                worker: native_worker(),
                base_rev: Some("main".into()),
                idempotency_key: Some("terminal-recovery-dedupe".into()),
            },
        )
        .await
        .unwrap();
        let exec_id = execution.id.clone();
        let child = execution.sub_discussion_id.clone().unwrap();
        let workspace_id = execution.workspace_id.clone().unwrap();
        let initial_dispatch = execution.dispatch_job_id.clone().unwrap();
        let marker_path = {
            let room = child.clone();
            db.with_conn(move |conn| {
                let workspace = crate::db::discussion_workspaces::list_for_discussion(conn, &room)?
                    .into_iter()
                    .find(|workspace| workspace.id == workspace_id)
                    .context("managed worktree missing")?;
                Ok(std::path::PathBuf::from(
                    workspace.canonical_path.context("canonical path missing")?,
                )
                .join("ollama-progress.txt"))
            })
            .await
            .unwrap()
        };
        std::fs::write(&marker_path, "unfinished local work").unwrap();
        let e = exec_id.clone();
        let seeded_child = child.clone();
        db.with_conn(move |conn| {
            crate::db::agent_dispatch::mark_completed(conn, &initial_dispatch)?;
            let message_id = format!("orch-resume-worker:{e}:0");
            let message = orchestrator_message(message_id, "Reprise déjà consommée".into());
            crate::db::discussions::insert_message(conn, &seeded_child, &message)?;
            let stale = crate::db::agent_dispatch::enqueue_for_latest_user(
                conn,
                crate::db::agent_dispatch::NewLatestUserDispatch {
                    id: "terminal-resume-job",
                    discussion_id: &seeded_child,
                    dedupe_key: &format!("orch-resume-worker:{e}:0"),
                    agent_override: None,
                    chain_prompt_ids: &[],
                    batch_item: None,
                    group_id: None,
                    group_concurrency_limit: None,
                },
            )?;
            conn.execute(
                "UPDATE agent_dispatch_jobs SET status = 'Cancelled' WHERE id = ?1",
                [&stale.id],
            )?;
            crate::db::orchestration::attach_execution_dispatch(conn, &e, &stale.id)?;
            crate::db::orchestration::transition_execution(
                conn,
                &e,
                TaskExecutionStatus::Interrupted,
                &test_actor(),
                serde_json::json!({ "reason": "test reboot" }),
            )?;
            Ok(())
        })
        .await
        .unwrap();

        wake_recovered_worker(&db, &exec_id).await.unwrap();
        let first_retry = exec_of(&db, &exec_id).await;
        assert_eq!(first_retry.status, TaskExecutionStatus::Working);
        let first_retry_dispatch = first_retry.dispatch_job_id.unwrap();
        assert_ne!(first_retry_dispatch, "terminal-resume-job");

        // A concurrent/manual retry while this wake is active must reuse it,
        // never create a second worker.
        wake_recovered_worker(&db, &exec_id).await.unwrap();
        assert_eq!(
            exec_of(&db, &exec_id).await.dispatch_job_id.as_deref(),
            Some(first_retry_dispatch.as_str())
        );

        // Reproduce the real dogfood sequence: another backend stop makes the
        // first retry terminal before delivery, then boot recovery runs again.
        let e = exec_id.clone();
        let failed = first_retry_dispatch.clone();
        db.with_conn(move |conn| {
            crate::db::agent_dispatch::mark_failed(conn, &failed, "backend restarted again")?;
            crate::db::orchestration::transition_execution(
                conn,
                &e,
                TaskExecutionStatus::Interrupted,
                &test_actor(),
                serde_json::json!({ "reason": "second test reboot" }),
            )?;
            Ok(())
        })
        .await
        .unwrap();

        wake_recovered_worker(&db, &exec_id).await.unwrap();
        let second_retry = exec_of(&db, &exec_id).await;
        assert_eq!(second_retry.status, TaskExecutionStatus::Working);
        assert_ne!(
            second_retry.dispatch_job_id.as_deref(),
            Some(first_retry_dispatch.as_str())
        );
        assert_eq!(
            second_retry.sub_discussion_id.as_deref(),
            Some(child.as_str())
        );
        assert!(
            marker_path.exists(),
            "unfinished worktree state must survive"
        );

        let e = exec_id.clone();
        let retry_jobs: i64 = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM agent_dispatch_jobs \
                     WHERE dedupe_key LIKE ?1",
                    [format!("orch-resume-worker:{e}:0%")],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(retry_jobs, 3, "one terminal base wake plus two retries");
    }

    /// DoD-6 + DoD-9 boundary (the change of test that proves the change of rule): `max_review_rounds`
    /// is the number of rounds the run is CONFIGURED to ALLOW — `>` semantics, so `max = 2` DELIVERS
    /// two re-offers (request_changes #1 AND #2 re-activate the worker) and escalates only on the
    /// third. Each below-budget round `Opened` its re-offer and parked `Blocked(awaiting_worker_
    /// acceptance)`; the worker re-accepts + re-delivers between rounds. The third round escalates,
    /// records the round, solicits the principal in the parent room, and opens NO further offer.
    #[tokio::test]
    async fn review_loop_reoffers_up_to_the_budget_then_escalates_past_it() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (parent_id, _child_id, exec_id, head, path) =
            delivered_awaiting_review(&db, repo.path()).await;
        // A run configured for TWO review rounds must deliver two re-offers before escalating.
        set_max_review_rounds(&db, &exec_id, 2).await;

        // ── Rounds 1 and 2: request_changes stays within budget → re-offer + park Blocked, and the
        // worker re-accepts + re-delivers each time. `max = 2` → both re-offer (the `>` boundary). ──
        for round in 1..=2u32 {
            let outcome = decide_review(
                &db,
                &exec_id,
                &review_request_changes(&format!("round {round}: corrige")),
                "ClaudeCode",
                "sess-b",
            )
            .await
            .unwrap();
            let execution = match outcome {
                ReviewOutcome::Reviewed {
                    verdict: ReviewVerdict::RequestChanges,
                    execution,
                } => execution,
                other => panic!("round {round}: expected RequestChanges, got {other:?}"),
            };
            // Parked in a VISIBLE, coded Blocked; round + attempt both bumped.
            assert_eq!(execution.status, TaskExecutionStatus::Blocked);
            assert_eq!(
                execution.blocked_reason_code,
                Some(crate::models::BlockedReasonCode::AwaitingWorkerAcceptance)
            );
            assert_eq!(
                execution.review_rounds, round,
                "round {round}: the round is recorded"
            );
            assert_eq!(
                execution.attempt_no, round,
                "round {round}: the attempt is bumped"
            );
            // The re-offer `Opened` (cancel-first left no self-clash) — EXACTLY ONE live offer on
            // the worker session at all times, for this round's attempt (DoD-9 scenario a).
            let live = live_offers_for_session(&db, 101).await;
            assert_eq!(
                live.len(),
                1,
                "round {round}: exactly one live offer on the session"
            );
            assert_eq!(live[0].attempt_no, round);
            assert_eq!(live[0].status, crate::models::WorkerOfferStatus::Pending);
            assert_eq!(
                git_rev(Path::new(&path), "HEAD"),
                head,
                "round {round}: worktree untouched"
            );

            // Worker re-accepts the re-offer → rework checkpoint resumes Working; re-deliver.
            let offer_id = live[0].id.clone();
            let out =
                accept_worker_offer_and_attach(&db, &offer_id, "ClaudeCode", "sess-a", "sess-a")
                    .await
                    .unwrap();
            assert!(
                matches!(out, AcceptAttachOutcome::Attached { .. }),
                "round {round}: re-accept attaches"
            );
            assert_eq!(
                exec_of(&db, &exec_id).await.status,
                TaskExecutionStatus::Working
            );
            let manifest = clean_manifest_for_execution(&db, &exec_id).await;
            deliver_worker_manifest(&db, &exec_id, "ClaudeCode", "sess-a", &manifest)
                .await
                .unwrap();
            assert_eq!(
                exec_of(&db, &exec_id).await.status,
                TaskExecutionStatus::AwaitingReview
            );
        }

        // ── Round 3: PAST the budget → escalate, no re-offer. ──
        let outcome = decide_review(
            &db,
            &exec_id,
            &review_request_changes("round 3 : budget dépassé"),
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        match outcome {
            ReviewOutcome::Escalated { execution } => {
                assert_eq!(execution.status, TaskExecutionStatus::Escalated);
                assert_eq!(
                    execution.review_rounds, 3,
                    "the exhausting round is still recorded"
                );
            }
            other => panic!("round 3: expected Escalated, got {other:?}"),
        }
        // The escalation opened NO re-offer, and the decision is persisted for its attempt.
        assert!(
            live_offers_for_session(&db, 101).await.is_empty(),
            "escalation opens no re-offer"
        );

        // An `escalated` event, attributed to the PRINCIPAL, names the reason + targeted identity;
        // the solicitation lands in the PARENT room targeted at the native principal, ZERO dispatch.
        let e = exec_id.clone();
        let (n_escalated, changes, actor_kind, disc, targets, dispatches) = db
            .with_conn(move |conn| {
                let n: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM task_execution_events \
                     WHERE task_execution_id = ?1 AND action = 'escalated'",
                    [&e],
                    |r| r.get(0),
                )?;
                let (changes, kind): (String, String) = conn.query_row(
                    "SELECT changes_json, actor_kind FROM task_execution_events \
                     WHERE task_execution_id = ?1 AND action = 'escalated'",
                    [&e],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                // At the escalating decide, exec.attempt_no was 2 (bumped by rounds 1 and 2).
                let mid = format!("orch-escalation:{e}:2");
                let disc: String = conn.query_row(
                    "SELECT discussion_id FROM messages WHERE id = ?1",
                    [&mid],
                    |r| r.get(0),
                )?;
                let targets = crate::db::discussions::list_message_targets(conn, &mid)?;
                let d: i64 =
                    conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |r| r.get(0))?;
                Ok((n, changes, kind, disc, targets, d))
            })
            .await
            .unwrap();
        assert_eq!(n_escalated, 1, "exactly one escalated obligation");
        assert_eq!(
            actor_kind,
            PlanningActorKind::Agent.as_str(),
            "attributed to the principal"
        );
        assert!(
            changes.contains("review_budget_exhausted"),
            "names the reason"
        );
        assert!(
            changes.contains("principal_target"),
            "carries the targeted identity"
        );
        assert_eq!(
            disc, parent_id,
            "escalation solicits the principal in the parent room"
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, MessageTargetKind::DiscussionAgent);
        assert_eq!(
            dispatches, 0,
            "no native dispatch — immediate wake is KT-335"
        );

        // A further decision is refused (Escalated is not AwaitingReview) — no duplicate (DoD-8).
        let second = decide_review(
            &db,
            &exec_id,
            &review_request_changes("encore"),
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        assert!(
            matches!(
                second,
                ReviewOutcome::NotReviewable {
                    status: TaskExecutionStatus::Escalated
                }
            ),
            "a decide of an escalated execution is refused, not re-run: {second:?}"
        );
    }

    /// DoD-4: a request_changes with no comment is not actionable — rejected as InvalidDecision
    /// (validated AFTER authz), and nothing moves.
    #[tokio::test]
    async fn request_changes_requires_a_comment() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, _child, exec_id, _head, _path) =
            delivered_awaiting_review(&db, repo.path()).await;
        let no_comment =
            serde_json::json!({ "version": "1", "task_ref": "KT-319", "decision": "request_changes" })
                .to_string();

        let outcome = decide_review(&db, &exec_id, &no_comment, "ClaudeCode", "sess-b")
            .await
            .unwrap();
        assert!(
            matches!(outcome, ReviewOutcome::InvalidDecision(_)),
            "got {outcome:?}"
        );
        assert_eq!(
            exec_of(&db, &exec_id).await.status,
            TaskExecutionStatus::AwaitingReview
        );
        assert!(review_row(&db, &exec_id).await.is_none());
    }

    /// DoD-2 (anti-oracle): a session that is neither the worker nor a member of the parent
    /// room is refused as NotAddressed — the same opaque refusal as an unknown execution —
    /// and nothing moves.
    #[tokio::test]
    async fn a_non_party_session_cannot_review() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, child_id, exec_id, _head, _path) =
            delivered_awaiting_review(&db, repo.path()).await;
        // A stranger: an active session that is NOT the worker and NOT in the parent room.
        seed_cli_session(&db, 103, &child_id, "sess-c").await;

        let outcome = decide_review(
            &db,
            &exec_id,
            &review_approve(&db, &exec_id).await,
            "ClaudeCode",
            "sess-c",
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, ReviewOutcome::NotAddressed),
            "stranger → NotAddressed, got {outcome:?}"
        );
        assert_eq!(
            exec_of(&db, &exec_id).await.status,
            TaskExecutionStatus::AwaitingReview
        );
        assert!(review_row(&db, &exec_id).await.is_none());
    }

    /// DoD-8: a repeated approve consumes the already-decided checkpoint without
    /// duplicating its review row. The transport may then retry the protected
    /// integration if the first attempt was refused after approval persisted.
    #[tokio::test]
    async fn review_is_idempotent_on_a_second_decision() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, _child, exec_id, _head, _path) =
            delivered_awaiting_review(&db, repo.path()).await;

        let first = decide_review(
            &db,
            &exec_id,
            &review_approve(&db, &exec_id).await,
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        assert!(matches!(
            first,
            ReviewOutcome::Reviewed {
                verdict: ReviewVerdict::Approve,
                ..
            }
        ));

        let second = decide_review(
            &db,
            &exec_id,
            &review_approve(&db, &exec_id).await,
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        assert!(
            matches!(
                second,
                ReviewOutcome::Reviewed {
                    verdict: ReviewVerdict::Approve,
                    execution: TaskExecution {
                        status: TaskExecutionStatus::Approved,
                        ..
                    }
                }
            ),
            "a repeated approve resumes the durable checkpoint: {second:?}"
        );

        let e = exec_id.clone();
        let n_reviews: i64 = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM task_execution_reviews WHERE task_execution_id = ?1",
                    [&e],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(n_reviews, 1, "exactly one review row (DoD-8)");
    }

    #[tokio::test]
    async fn approved_review_retries_integration_after_dirty_parent_is_committed() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_task_ref, parent_id, _child, exec_id) = attached_cli_worker(&db, repo.path()).await;
        let child_path = managed_worktree_path(&db, &exec_id).await;
        std::fs::write(Path::new(&child_path).join("worker.txt"), "worker change").unwrap();
        assert!(git(Path::new(&child_path), &["add", "worker.txt"])
            .status
            .success());
        assert!(
            git(Path::new(&child_path), &["commit", "-m", "worker change"])
                .status
                .success()
        );
        let child_head = git_rev(Path::new(&child_path), "HEAD");
        let manifest = manifest_json_with_files_for_dod(
            &child_head,
            serde_json::json!([{ "path": "worker.txt", "kind": "added" }]),
            &dod_id_for_execution(&db, &exec_id).await,
            true,
        );
        deliver_worker_manifest(&db, &exec_id, "ClaudeCode", "sess-a", &manifest)
            .await
            .unwrap();
        seed_cli_session(&db, 102, &parent_id, "sess-b").await;

        let parent_file = repo.path().join("parent-after-review.txt");
        std::fs::write(&parent_file, "parent moved").unwrap();
        let reviewed = decide_review(
            &db,
            &exec_id,
            &review_approve(&db, &exec_id).await,
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        let error = continue_approved_review(&db, reviewed)
            .await
            .expect_err("a dirty parent must refuse the first integration attempt");
        assert!(matches!(error, ProvisionError::NotLaunchable(_)));
        assert_eq!(
            exec_of(&db, &exec_id).await.status,
            TaskExecutionStatus::Approved,
            "the durable approval remains available for retry"
        );

        assert!(git(repo.path(), &["add", "parent-after-review.txt"])
            .status
            .success());
        assert!(git(repo.path(), &["commit", "-m", "parent advances"])
            .status
            .success());

        let retried = decide_review(
            &db,
            &exec_id,
            &review_approve(&db, &exec_id).await,
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        let completed = continue_approved_review(&db, retried).await.unwrap();
        assert!(matches!(
            completed,
            ReviewOutcome::Reviewed {
                verdict: ReviewVerdict::Approve,
                execution: TaskExecution {
                    status: TaskExecutionStatus::Done,
                    ..
                }
            }
        ));
        assert!(parent_file.exists(), "the newer parent commit is preserved");
        assert!(repo.path().join("worker.txt").exists());

        let execution_id = exec_id.clone();
        let (reviews, approvals): (i64, i64) = db
            .with_conn(move |conn| {
                let reviews = conn.query_row(
                    "SELECT COUNT(*) FROM task_execution_reviews WHERE task_execution_id = ?1",
                    [&execution_id],
                    |row| row.get(0),
                )?;
                let approvals = conn.query_row(
                    "SELECT COUNT(*) FROM task_execution_events \
                     WHERE task_execution_id = ?1 AND to_status = 'Approved'",
                    [&execution_id],
                    |row| row.get(0),
                )?;
                Ok((reviews, approvals))
            })
            .await
            .unwrap();
        assert_eq!(reviews, 1, "retry does not duplicate the review row");
        assert_eq!(approvals, 1, "retry does not duplicate the Approved event");
    }

    /// Pure anti-oracle mapping: NotAddressed fuses unknown-execution and non-party into ONE
    /// opaque refusal (no leak of which), while the post-authz refusals stay precise.
    #[test]
    fn review_outcome_fuses_unaddressed_and_keeps_others_distinct() {
        let unaddressed = review_outcome_to_response(ReviewOutcome::NotAddressed);
        assert!(!unaddressed.success);
        assert_eq!(unaddressed.error_code.as_deref(), Some("not_found"));
        let msg = unaddressed.error.as_deref().unwrap();
        assert!(
            !msg.contains("worker") && !msg.contains("principal"),
            "the opaque refusal must not reveal which party check failed: {msg}"
        );

        let self_review = review_outcome_to_response(ReviewOutcome::SelfReviewForbidden);
        assert_eq!(self_review.error_code.as_deref(), Some("conflict"));
        assert!(self_review
            .error
            .as_deref()
            .unwrap()
            .contains("self-review"));

        let not_reviewable = review_outcome_to_response(ReviewOutcome::NotReviewable {
            status: TaskExecutionStatus::Approved,
        });
        assert_eq!(not_reviewable.error_code.as_deref(), Some("conflict"));
        assert!(not_reviewable
            .error
            .as_deref()
            .unwrap()
            .contains(TaskExecutionStatus::Approved.as_str()));

        let blocked = review_outcome_to_response(ReviewOutcome::ApproveBlocked {
            reason: ApproveBlockReason::HeadDrifted {
                delivered: "aaaaaaa".into(),
                current: "bbbbbbb".into(),
            },
        });
        assert_eq!(blocked.error_code.as_deref(), Some("conflict"));
        assert!(blocked.error.as_deref().unwrap().contains("drifted"));

        let invalid = review_outcome_to_response(ReviewOutcome::InvalidDecision("bad".into()));
        assert_eq!(invalid.error_code.as_deref(), Some("validation"));
    }

    // ── KT-319 tranche 3b helpers ──────────────────────────────────────────────────────────
    async fn set_max_review_rounds(db: &Database, exec_id: &str, n: i64) {
        let e = exec_id.to_string();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE task_executions SET max_review_rounds = ?2 WHERE id = ?1",
                rusqlite::params![e, n],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    async fn live_offers_for_session(
        db: &Database,
        session_pk: i64,
    ) -> Vec<crate::models::TaskExecutionWorkerOffer> {
        db.with_conn(move |conn| {
            let ids: Vec<String> = conn
                .prepare(
                    "SELECT id FROM task_execution_worker_offers \
                     WHERE target_cli_session_id = ?1 AND status IN ('pending', 'accepting') \
                     ORDER BY attempt_no",
                )?
                .query_map([session_pk], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut offers = Vec::new();
            for id in ids {
                offers.push(crate::db::worker_offers::get_worker_offer(conn, &id)?.unwrap());
            }
            Ok(offers)
        })
        .await
        .unwrap()
    }

    async fn offer_by_id(db: &Database, id: &str) -> crate::models::TaskExecutionWorkerOffer {
        let id = id.to_string();
        db.with_conn(
            move |conn| Ok(crate::db::worker_offers::get_worker_offer(conn, &id)?.unwrap()),
        )
        .await
        .unwrap()
    }

    async fn task_status(db: &Database, exec_id: &str) -> String {
        let e = exec_id.to_string();
        db.with_conn(move |conn| {
            let task_id: String = conn.query_row(
                "SELECT task_id FROM task_executions WHERE id = ?1",
                [&e],
                |r| r.get(0),
            )?;
            Ok(conn.query_row(
                "SELECT status FROM planning_tasks WHERE id = ?1",
                [&task_id],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap()
    }

    async fn transition_count(db: &Database, exec_id: &str, from: &str, to: &str) -> i64 {
        let (e, f, t) = (exec_id.to_string(), from.to_string(), to.to_string());
        db.with_conn(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM task_execution_events \
                 WHERE task_execution_id = ?1 AND from_status = ?2 AND to_status = ?3",
                rusqlite::params![e, f, t],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap()
    }

    /// DoD-9 (constructed scenario b): if a PRIOR attempt's offer is still LIVE when a
    /// request_changes re-activates the worker (a worker that never re-accepted), cancel-first
    /// CANCELS it before opening the next — so the re-offer `Opened` instead of tripping the
    /// session's live-offer uniqueness (`SessionCommittedElsewhere` onto itself). Contrived: the
    /// accepted initial offer is forced back to `pending` to stand in for a still-live prior.
    #[tokio::test]
    async fn cli_reoffer_cancels_a_still_live_prior_offer_before_opening_the_next() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, _child, exec_id, _head, _path) =
            delivered_awaiting_review(&db, repo.path()).await;
        // Model a still-live prior offer: force the accepted attempt-0 offer back to `pending`.
        let e = exec_id.clone();
        let prior_id: String = db
            .with_conn(move |conn| {
                let id: String = conn.query_row(
                    "SELECT id FROM task_execution_worker_offers \
                     WHERE task_execution_id = ?1 AND attempt_no = 0",
                    [&e],
                    |r| r.get(0),
                )?;
                conn.execute(
                    "UPDATE task_execution_worker_offers SET status = 'pending' WHERE id = ?1",
                    [&id],
                )?;
                Ok(id)
            })
            .await
            .unwrap();

        let outcome = decide_review(
            &db,
            &exec_id,
            &review_request_changes("re-offer despite a still-live prior"),
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome,
            ReviewOutcome::Reviewed {
                verdict: ReviewVerdict::RequestChanges,
                ..
            }
        ));

        // The live prior is CANCELLED (not left to wedge the session), and the next attempt's
        // offer `Opened` — the session holds exactly ONE live offer, the new one.
        assert_eq!(
            offer_by_id(&db, &prior_id).await.status,
            crate::models::WorkerOfferStatus::Cancelled,
            "cancel-first cancelled the still-live prior offer"
        );
        let live = live_offers_for_session(&db, 101).await;
        assert_eq!(live.len(), 1, "exactly one live offer remains");
        assert_eq!(
            live[0].attempt_no, 1,
            "the re-offer is for the next attempt"
        );
        assert_eq!(live[0].status, crate::models::WorkerOfferStatus::Pending);
        let exec = exec_of(&db, &exec_id).await;
        assert_eq!(exec.status, TaskExecutionStatus::Blocked);
        assert_eq!(
            exec.blocked_reason_code,
            Some(crate::models::BlockedReasonCode::AwaitingWorkerAcceptance)
        );
    }

    /// DoD-9: the rework re-accept resumes `Blocked`(ChangesRequested-origin) → `Working` WITHOUT
    /// re-running the task-CAS — the task is ALREADY InProgress from the first accept. The
    /// anti-race authority the task-CAS held at provisioning is REPLACED (not removed) by the two
    /// CAS here; exactly ONE `Blocked → Working` transition is journaled.
    #[tokio::test]
    async fn rework_reaccept_resumes_blocked_to_working_without_task_cas() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, _child, exec_id, _head, _path) =
            delivered_awaiting_review(&db, repo.path()).await;
        decide_review(
            &db,
            &exec_id,
            &review_request_changes("fix it"),
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        let task_before = task_status(&db, &exec_id).await;
        assert_ne!(
            task_before, "todo",
            "the task was already started at the first accept"
        );
        let offer_id = live_offers_for_session(&db, 101).await[0].id.clone();
        // The initial handshake already ran one Provisioning → Working; the rework resume adds
        // exactly one more (Blocked → Provisioning → Working, the mirror of that handshake).
        let pw_before = transition_count(&db, &exec_id, "Provisioning", "Working").await;

        let out = accept_worker_offer_and_attach(&db, &offer_id, "ClaudeCode", "sess-a", "sess-a")
            .await
            .unwrap();
        assert!(
            matches!(out, AcceptAttachOutcome::Attached { .. }),
            "got {out:?}"
        );
        let resumed = exec_of(&db, &exec_id).await;
        assert_eq!(
            resumed.status,
            TaskExecutionStatus::Working,
            "resumed Working"
        );
        assert_eq!(
            resumed.blocked_reason, None,
            "rework acceptance consumes the visible Blocked hold"
        );
        assert_eq!(resumed.blocked_reason_code, None);
        assert_eq!(
            offer_by_id(&db, &offer_id).await.status,
            crate::models::WorkerOfferStatus::Accepted,
            "the offer settled accepting → accepted"
        );
        assert_eq!(
            task_status(&db, &exec_id).await,
            task_before,
            "the task is untouched — no task-CAS re-run on rework"
        );
        assert_eq!(
            transition_count(&db, &exec_id, "Provisioning", "Working").await,
            pw_before + 1,
            "exactly one rework resume (Blocked → Provisioning → Working)"
        );
        // Re-accepting again is idempotent: no duplicate resume (DoD-8).
        let again =
            accept_worker_offer_and_attach(&db, &offer_id, "ClaudeCode", "sess-a", "sess-a")
                .await
                .unwrap();
        assert!(matches!(again, AcceptAttachOutcome::Attached { .. }));
        assert_eq!(
            transition_count(&db, &exec_id, "Provisioning", "Working").await,
            pw_before + 1,
            "a duplicate re-accept does not double-resume"
        );
    }

    /// KT-425 / anti-race: a duplicate exact-session call that observes a rework offer in
    /// `accepting` resumes the idempotent checkpoint. It may return the same successful
    /// attachment as the original caller, but the durable resume transition happens once.
    #[tokio::test]
    async fn duplicate_reaccept_of_accepting_rework_converges_without_double_resume() {
        let repo = init_repo();
        let db = Database::open_in_memory().unwrap();
        let (_parent, _child, exec_id, _head, _path) =
            delivered_awaiting_review(&db, repo.path()).await;
        decide_review(
            &db,
            &exec_id,
            &review_request_changes("fix it"),
            "ClaudeCode",
            "sess-b",
        )
        .await
        .unwrap();
        let offer_id = live_offers_for_session(&db, 101).await[0].id.clone();
        let pw_before = transition_count(&db, &exec_id, "Provisioning", "Working").await;
        // The winning re-accept has staged (pending → accepting) but not yet finalized.
        let o = offer_id.clone();
        db.with_conn(move |conn| {
            crate::db::worker_offers::transition_offer_status(
                conn,
                &o,
                crate::models::WorkerOfferStatus::Pending,
                crate::models::WorkerOfferStatus::Accepting,
                None,
            )
        })
        .await
        .unwrap();

        // A duplicate exact-session call resumes the SAME accepting offer. Treating it as
        // a permanent conflict would strand the saga after a process crash.
        let out = accept_worker_offer_and_attach(&db, &offer_id, "ClaudeCode", "sess-a", "sess-a")
            .await
            .unwrap();
        assert!(
            matches!(out, AcceptAttachOutcome::Attached { .. }),
            "the exact-session retry must converge to Attached: {out:?}"
        );
        assert_eq!(
            exec_of(&db, &exec_id).await.status,
            TaskExecutionStatus::Working,
            "the accepting saga resumed"
        );
        assert_eq!(
            offer_by_id(&db, &offer_id).await.status,
            crate::models::WorkerOfferStatus::Accepted
        );
        assert_eq!(
            transition_count(&db, &exec_id, "Provisioning", "Working").await,
            pw_before + 1,
            "the durable resume transition happens exactly once"
        );

        let replay =
            accept_worker_offer_and_attach(&db, &offer_id, "ClaudeCode", "sess-a", "sess-a")
                .await
                .unwrap();
        assert!(matches!(replay, AcceptAttachOutcome::Attached { .. }));
        assert_eq!(
            transition_count(&db, &exec_id, "Provisioning", "Working").await,
            pw_before + 1,
            "a replay after settlement cannot double-resume"
        );
    }
}
