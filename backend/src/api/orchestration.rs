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

use crate::core::room_lang::RoomLang;
use crate::core::task_worker_env::{Capability, ToolchainCapabilities};
use crate::core::{scanner, worktree};
use crate::db::orchestration::{CheckpointOutcome, ProvisioningCheckpoint};
use crate::db::planning::StartTaskCheckpoint;
use crate::db::Database;
use crate::models::{
    AgentType, ApiErrorCode, ApiResponse, BlockedReasonCode, DeliveryManifestV1, Discussion,
    DiscussionMessage, ExecutionRecoveryAction, FileChangeKind, ManifestFile, MessageChannel,
    MessageRole, MessageTarget, MessageTargetKind, ModelTier, OrchestrationActor, PlanningActor,
    PlanningActorKind, PlanningDodItem, PlanningDodVerification, PlanningTaskStatus,
    ReviewDecisionV1, ReviewFinding, ReviewVerdict, SagaResumeAction, SummaryStrategy,
    TaskExecution, TaskExecutionStatus, TaskWorkerScope, DELIVERY_CONTRACT_VERSION,
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
    /// KT-1345 — the review-round limit of a single-task launch: the caller's
    /// explicit one, else Settings > Workers. `None` keeps the model default.
    max_review_rounds: Option<u32>,
    /// KT-1564 — the launch's explicit provider-fallback alternatives.
    quota_fallback_agents: &'a [crate::models::AgentType],
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
    /// KT-1318: the volume is below `server.disk_launch_min_free_gib` even after
    /// the orphan sweep. The execution is left `Blocked` (resumable once space
    /// is freed) and the principal room was told.
    DiskPressure(String),
}

/// Validate principal-authored mechanical gates before they are persisted.
///
/// Quick Exec owns the execution ceiling. Keeping this check at the orchestration
/// boundary prevents a newly-created run from carrying a timeout that the
/// validation runner would later refuse. Persisted runs from older Kronn builds
/// are handled separately by `run_one_validation` so upgrades remain resumable.
///
/// The command line is held to the rules its run applies (KT-839): a validation
/// that Quick Exec will refuse — `cd frontend && …`, a `VAR=` prefix, a binary
/// off the allowlist — is refused HERE, while the principal can still fix it,
/// rather than at integration after the worker delivered and the review passed.
pub fn validate_new_validation_specs(
    validations: &[crate::models::ValidationSpec],
) -> Result<(), String> {
    let max_timeout = crate::core::quick_exec::MAX_TIMEOUT_SECS;
    if let Some(bad) = validations.iter().find(|spec| {
        spec.command.trim().is_empty()
            || spec
                .timeout_secs
                .is_some_and(|timeout| timeout == 0 || u64::from(timeout) > max_timeout)
    }) {
        return Err(format!(
            "command must be non-empty and timeout_secs must be between 1 and {max_timeout} (got {})",
            serde_json::to_string(bad).unwrap_or_default()
        ));
    }
    // KT-1751: a name some project lists is not refused here; the run checks it
    // against the project it runs in (that check is the authoritative one).
    let listed = crate::core::task_worker_lane_settings::all_validation_executables();
    for spec in validations {
        if let Err(rejection) =
            crate::core::quick_exec::check_command_line_with(&spec.command, &listed)
        {
            return Err(format!(
                "validation `{}` can never run: {rejection}",
                spec.command
            ));
        }
    }
    Ok(())
}

/// A backend/system actor — every provisioning write is attributed to it.
pub fn backend_actor() -> OrchestrationActor {
    PlanningActor {
        kind: PlanningActorKind::Backend,
        id: Some("orchestrator".to_string()),
        session_id: None,
        source_message_id: None,
        token_declared: false,
    }
}

/// A worker (agent) actor — a worker's own delivery is attributed to IT, not the backend,
/// so the journal shows who delivered (DoD-2). `Agent` is a non-spoofable planning actor
/// kind, distinct from the `Backend` orchestrator transitions.
/// An agent actor (`kind = Agent`) attributed to `alias` — the worker at delivery, the
/// principal at review. The non-spoofable identity planning already uses for its journal.
pub fn agent_actor(alias: &str, session_id: Option<&str>) -> OrchestrationActor {
    PlanningActor {
        kind: PlanningActorKind::Agent,
        id: Some(alias.to_string()),
        session_id: session_id.map(str::to_string),
        source_message_id: None,
        token_declared: false,
    }
}

/// First 8 chars of the execution id — the collision-free short id in the
/// deterministic worktree path/branch.
pub fn exec_short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

#[derive(Debug, Default, Serialize)]
pub struct OrchestrationBootReport {
    pub interrupted: usize,
    pub classified: usize,
    pub resumed_or_parked: usize,
    /// Classified executions whose resume can run Git, validations or a
    /// provisioning: [`spawn_deferred_boot_resumes`] applies them once HTTP is served.
    pub deferred_resumes: Vec<String>,
    pub orphan_workspaces_removed: usize,
    pub orphan_workspaces_preserved: usize,
    /// Managed checkouts of finished executions released at boot (KT-1397).
    pub terminal_worktrees_released: usize,
    /// Managed checkouts of finished executions kept at boot: recently
    /// finished, still in use or refused (KT-1397).
    pub terminal_worktrees_kept: usize,
    /// Lane targets Kronn marked, whose execution finished and whose worktree
    /// is gone, removed at boot (KT-1256).
    pub orphan_lane_targets_removed: usize,
    /// Marked lanes of a finished execution that were not removed: the
    /// worktree is still there (clean or dirty) or a refusal; reasons in
    /// `lane_target_report`.
    pub orphan_lane_targets_preserved: usize,
    /// Bytes the removed lane targets held, floored at the scan budget.
    pub orphan_lane_target_bytes: u64,
    /// `wt-*` directories without Kronn's ownership marker: reported, never
    /// touched.
    pub lane_targets_unmarked: usize,
    /// The sweep's dry-run view in plain lines, one per lane target that was
    /// removed, could be removed, was kept or is not Kronn's.
    pub lane_target_report: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Default)]
pub struct DeferredResumeReport {
    pub resumed_or_parked: usize,
    /// Already resumed, cancelled or being resumed by another caller.
    skipped: usize,
    pub errors: Vec<String>,
}

/// One execution's `/resume` slot, released on drop.
struct ResumeSlot {
    slots: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    execution_id: String,
}

impl ResumeSlot {
    fn claim(
        slots: &std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
        execution_id: &str,
    ) -> Option<Self> {
        let mut held = slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        held.insert(execution_id.to_string()).then(|| Self {
            slots: slots.clone(),
            execution_id: execution_id.to_string(),
        })
    }
}

impl Drop for ResumeSlot {
    fn drop(&mut self) {
        self.slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.execution_id);
    }
}

/// Whether this boot decision can run Git, validations or a provisioning. The rest stay
/// before the dispatcher starts, so it never claims a job of a still-Interrupted execution.
fn boot_resume_is_deferred(execution: &TaskExecution, action: ExecutionRecoveryAction) -> bool {
    use ExecutionRecoveryAction::*;
    // An interrupted Applying hold replays its apply whatever the decision says.
    if execution.interrupted_from_status == Some(TaskExecutionStatus::Blocked)
        && execution.blocked_from_status == Some(TaskExecutionStatus::Applying)
    {
        return true;
    }
    match action {
        ResumeProvisioning | RebuildCandidate | RunValidations | ApplyFastForward
        | IdempotentClose => true,
        ResumeWorker
        | AwaitReview
        | AwaitHuman
        | BlockDirtyTarget
        | BlockMissingWorkspace
        | BlockMissingDiscussion
        | BlockAgentUnavailable => false,
    }
}

pub fn available_agent_types(
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

pub async fn available_agent_types_for_state(state: &AppState) -> Vec<AgentType> {
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
/// No status is declared successful from a checkpoint alone. Decisions that can
/// run for minutes are returned in `deferred_resumes` instead of being applied.
pub async fn reconcile_at_boot(state: &AppState) -> OrchestrationBootReport {
    let available_agents = available_agent_types_for_state(state).await;
    reconcile_at_boot_with_agents(state, &available_agents).await
}

pub async fn reconcile_at_boot_with_agents(
    state: &AppState,
    available_agents: &[AgentType],
) -> OrchestrationBootReport {
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

    for id in ids {
        // Recovery decisions describe the real execution/Git state at one
        // instant. A failed apply may leave that decision pending while the
        // execution advances to a newer Interrupted checkpoint. Reclassify on
        // every boot before consuming it so a stale ResumeProvisioning can
        // never be replayed against an Applying-origin interruption.
        if let Err(error) = classify_interrupted_execution(&state.db, &id, available_agents).await {
            report
                .errors
                .push(format!("execution {id} classification: {error}"));
            continue;
        }
        report.classified += 1;
        let deferred = {
            let id = id.clone();
            state
                .db
                .with_conn(move |conn| {
                    let execution = crate::db::orchestration::get_task_execution(conn, &id)?
                        .context("execution vanished after classification")?;
                    let recovery = crate::db::orchestration::get_execution_recovery(conn, &id)?
                        .context("classification left no recovery decision")?;
                    Ok(boot_resume_is_deferred(
                        &execution,
                        recovery.recovery_action,
                    ))
                })
                .await
        };
        match deferred {
            Ok(false) => {}
            Ok(true) => {
                report.deferred_resumes.push(id);
                continue;
            }
            Err(error) => {
                report
                    .errors
                    .push(format!("execution {id} recovery scheduling: {error}"));
                report.deferred_resumes.push(id);
                continue;
            }
        }
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
        let had_checkout = checkout.exists();
        let removed = if !had_checkout {
            Ok(())
        } else {
            worktree::remove_cancelled_task_worktree(&repo, &checkout_path, &orphan.branch)
        };
        match removed {
            Ok(()) => {
                // KT-1318 — an orphaned checkout removed just now takes its lane
                // build dir with it, like at every other end of life (Done,
                // Cancelled). A checkout that was already gone leaves its lane
                // to the idle-aware sweep below.
                if had_checkout {
                    reclaim_lane_target(&state.db, &repo, &checkout_path).await;
                }
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

    // KT-1397 — a finished execution (Done, Failed, Cancelled) keeps its
    // checkout when the cancel carried the `preserve` policy, the tree was
    // dirty, or no cleanup path ran at all. Released before the lane sweep so
    // the lane of a checkout removed here goes with it.
    release_terminal_worktrees(
        &state.db,
        crate::core::disk_usage::sweep_idle(),
        &mut report,
    )
    .await;

    // KT-1256 — a lane target outlives its worktree whenever the removal that
    // should have taken it was a crash, a refusal or a build away. Only lanes
    // Kronn marked are candidates, judged from durable state per project; one
    // project's failure never stops the rest.
    let projects = state
        .db
        .with_conn(crate::db::projects::list_projects)
        .await
        .unwrap_or_else(|error| {
            report
                .errors
                .push(format!("lane target sweep project scan: {error}"));
            Vec::new()
        });
    for project in projects {
        let repo = scanner::resolve_host_path(&project.path);
        // KT-1519 — a feature branch merged while Kronn was down or by a PR
        // outside it: its integrated tasks close now.
        close_merged_awaiting_tasks(&state.db, &project.id, &repo, false).await;
        if !repo.join("target").is_dir() {
            continue;
        }
        // Boot deletes only what is provably Kronn's, finished and already
        // detached from its worktree; every other `wt-*` is reported.
        match sweep_project_lane_targets_idle(
            &state.db,
            &project.id,
            &repo,
            worktree::LaneSweepScope::WorktreeGone,
            crate::core::disk_usage::sweep_idle(),
        )
        .await
        {
            Ok(sweep) => {
                report.orphan_lane_targets_removed += sweep.removed.len();
                report.orphan_lane_target_bytes += sweep.bytes_removed();
                report.lane_targets_unmarked += sweep.unmarked.len();
                report.orphan_lane_targets_preserved += sweep.would_remove.len() + sweep.kept.len();
                for (owner, removed) in &sweep.removed {
                    report.lane_target_report.push(format!(
                        "removed {} ({} bytes{}; execution {})",
                        removed.target_path.display(),
                        removed.bytes_reclaimed,
                        if removed.bytes_are_partial { "+" } else { "" },
                        owner.execution_id
                    ));
                }
                for (owner, removable) in &sweep.would_remove {
                    report.lane_target_report.push(format!(
                        "reclaimable, kept with its clean worktree: {} ({} bytes{}; execution {})",
                        removable.target_path.display(),
                        removable.bytes_reclaimed,
                        if removable.bytes_are_partial { "+" } else { "" },
                        owner.execution_id
                    ));
                }
                for (path, reason) in &sweep.kept {
                    report
                        .lane_target_report
                        .push(format!("kept {}: {reason}", path.display()));
                }
                for path in &sweep.unmarked {
                    report.lane_target_report.push(format!(
                        "not Kronn's, untouched: {} (no {} marker)",
                        path.display(),
                        worktree::LANE_OWNER_MARKER
                    ));
                }
            }
            Err(error) => report.errors.push(format!(
                "lane target sweep of project {}: {error}",
                project.id
            )),
        }
    }
    report
}

/// Apply the decisions [`reconcile_at_boot`] deferred, in boot order, on a
/// background task so a replayed validation never holds up the HTTP listener.
pub fn spawn_deferred_boot_resumes(state: AppState, execution_ids: Vec<String>) {
    if execution_ids.is_empty() {
        return;
    }
    tokio::spawn(async move {
        let available_agents = available_agent_types_for_state(&state).await;
        let report = resume_deferred_at_boot(state, execution_ids, available_agents).await;
        if report.errors.is_empty() {
            tracing::info!(
                resumed_or_parked = report.resumed_or_parked,
                skipped = report.skipped,
                "Deferred task-execution resumes completed"
            );
        } else {
            tracing::warn!(
                resumed_or_parked = report.resumed_or_parked,
                skipped = report.skipped,
                errors = ?report.errors,
                "Deferred task-execution resumes completed"
            );
        }
    });
}

pub async fn resume_deferred_at_boot(
    state: AppState,
    execution_ids: Vec<String>,
    available_agents: Vec<AgentType>,
) -> DeferredResumeReport {
    let mut report = DeferredResumeReport::default();
    for id in execution_ids {
        // A user resume that started first owns the execution.
        let Some(_slot) = ResumeSlot::claim(&state.execution_resumes, &id) else {
            report.skipped += 1;
            tracing::info!(execution_id = %id, "deferred boot resume skipped: another resume is running");
            continue;
        };
        let still_interrupted = {
            let id = id.clone();
            state
                .db
                .with_conn(move |conn| {
                    Ok(
                        crate::db::orchestration::get_task_execution(conn, &id)?.is_some_and(
                            |execution| execution.status == TaskExecutionStatus::Interrupted,
                        ),
                    )
                })
                .await
        };
        match still_interrupted {
            Ok(true) => {}
            Ok(false) => {
                report.skipped += 1;
                tracing::info!(execution_id = %id, "deferred boot resume skipped: no longer interrupted");
                continue;
            }
            Err(error) => {
                let message = format!("execution {id} deferred resume scan: {error}");
                tracing::warn!("{message}");
                report.errors.push(message);
                continue;
            }
        }
        // Minutes may separate the boot decision from this apply, and an earlier
        // resume may have moved the target: decide again from the refs as they are.
        if let Err(error) = classify_interrupted_execution(&state.db, &id, &available_agents).await
        {
            let message = format!("execution {id} classification: {error}");
            tracing::warn!("{message}");
            report.errors.push(message);
            continue;
        }
        let started = std::time::Instant::now();
        let response = resume_claimed_execution(state.clone(), id.clone()).await.0;
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if response.success {
            report.resumed_or_parked += 1;
            let outcome = response.data.and_then(|view| view.outcome);
            tracing::info!(execution_id = %id, elapsed_ms, ?outcome, "deferred boot resume completed");
        } else {
            let message = format!(
                "execution {id} recovery apply: {}",
                response
                    .error
                    .unwrap_or_else(|| "unknown recovery error".into())
            );
            tracing::warn!(execution_id = %id, elapsed_ms, "{message}");
            report.errors.push(message);
        }
    }
    report
}

/// KT-1564 — a quota refusal that announced its reset closes itself. Once that
/// instant has passed, an execution goes back to the worker it was given:
///
/// - one still parked `Escalated` on the refused provider returns to that same
///   provider, tier, model and profile (the same-provider retry of KT-838);
/// - one a quota fallback (explicit campaign opt-in) moved to another provider
///   returns to the worker it left, not to a worker rebuilt from the campaign
///   defaults or its allow-list.
///
/// The escalation closes, the worker is handed its work again in its own room.
/// Another provider is never picked here. A paused, human-gated or finished run
/// is left alone, and the reassignment re-checks all of this in its own
/// transaction so a person acting in the meantime is never overridden. Returns
/// the executions it handed back.
pub async fn return_quota_escalations_past_reset(state: &AppState) -> Vec<String> {
    let now = chrono::Utc::now();
    let due = match state
        .db
        .with_read_conn(move |conn| {
            let mut due = crate::db::orchestration::executions_past_quota_reset(conn, now)?;
            for id in crate::db::orchestration::fallback_executions_past_origin_reset(conn, now)? {
                if !due.contains(&id) {
                    due.push(id);
                }
            }
            Ok(due)
        })
        .await
    {
        Ok(due) => due,
        Err(error) => {
            tracing::warn!("quota reset scan failed: {error}");
            return Vec::new();
        }
    };
    let mut returned = Vec::new();
    for execution_id in due {
        let id = execution_id.clone();
        let selection = state
            .db
            .with_conn(move |conn| {
                let execution = crate::db::orchestration::get_task_execution(conn, &id)?
                    .context("execution vanished before its quota return")?;
                if let Some(origin) = crate::db::orchestration::quota_origin(conn, &id)? {
                    // Already back on the original worker (provider and tier):
                    // nothing to hand back, and replacing a live worker with
                    // itself would cancel its dispatch. The memory is spent.
                    if origin.is_worker_of(&execution) {
                        crate::db::orchestration::clear_quota_origin(conn, &id)?;
                        return Ok(None);
                    }
                    if origin.is_due(now) {
                        return Ok(Some(origin.selection));
                    }
                }
                Ok(Some(crate::models::CampaignWorkerSelection {
                    target: worker_target_from_execution(&execution)?,
                    model: execution.worker_model.clone(),
                    profile_id: execution.worker_profile_id.clone(),
                }))
            })
            .await;
        let selection = match selection {
            Ok(Some(selection)) => Ok(selection),
            Ok(None) => {
                tracing::info!(
                    execution_id,
                    "quota origin spent: the execution is already on its original worker"
                );
                continue;
            }
            Err(error) => Err(error),
        };
        let result = match selection {
            Ok(selection) => reassign_native_execution_as(
                state,
                &execution_id,
                selection,
                "the provider's announced quota reset has passed: back to the original provider",
                ReassignCause::QuotaReturn,
            )
            .await,
            Err(error) => Err(error),
        };
        match result {
            Ok(_) => {
                tracing::info!(
                    execution_id,
                    "quota reset elapsed: execution returned to its provider"
                );
                returned.push(execution_id);
            }
            Err(error) => {
                tracing::warn!(
                    execution_id,
                    "quota return to the original provider failed: {error}"
                );
            }
        }
    }
    returned
}

/// Stop the processes a task worker's dispatch left behind (KT-1266). Only a
/// dispatch that is a worker's launch has any; an ordinary turn costs one read.
pub async fn reap_task_worker_processes(state: &AppState, dispatch_job_id: &str) {
    let lookup = dispatch_job_id.to_string();
    let is_worker = state
        .db
        .with_read_conn(move |conn| {
            crate::db::orchestration::get_execution_for_dispatch(conn, &lookup)
        })
        .await
        .ok()
        .flatten()
        .is_some();
    if is_worker {
        crate::core::dispatch_reaper::reap_dispatch_async(dispatch_job_id).await;
    }
}

/// KT-1758 — how much of a refusal the rooms quote. The native ACP refusal is
/// about 230 characters; the whole sentence is the point.
pub const LAUNCH_REFUSAL_DETAIL_CHARS: usize = 600;

/// KT-1758 — consecutive deferrals of a task worker's dispatch for the same
/// unavailable-runtime reason before Kronn stops retrying and says why: ten
/// tries at 30 s is five minutes, longer than any start that is only slow. A
/// refusal that never changes used to be retried forever (live: a native ACP
/// agent refused as a task worker, every 30 s for 1h35).
pub const RUNTIME_UNAVAILABLE_MAX_DEFERRALS: u32 = 10;

/// KT-1758 — how long a quota fallback's dispatch may stay `Pending` before the
/// room is told why. A free slot starts it within seconds, so five minutes is
/// already a dispatch that something is holding.
pub const FALLBACK_DISPATCH_WAIT_NOTICE_AFTER_SECONDS: i64 = 300;

/// KT-1758 — the room line of a fallback dispatch that is still waiting.
pub fn fallback_dispatch_waiting_message_id(dispatch_id: &str) -> String {
    format!("fallback-dispatch-waiting:{dispatch_id}")
}

/// KT-1758 — why a fallback dispatch is not running, from what the queue holds
/// against it, most specific first. Never a guess: when nothing explains the
/// wait, it says so, which is itself the finding.
pub fn fallback_dispatch_wait_reason(
    wait: &crate::db::agent_dispatch::WaitingFallbackDispatch,
    limits: &serde_json::Value,
    now: chrono::DateTime<chrono::Utc>,
    lang: RoomLang,
) -> String {
    let provider = wait.provider.as_str();
    let last_error = wait.last_error.as_deref().unwrap_or_default();
    if let Some((deferrals, detail)) =
        crate::db::agent_dispatch::parse_runtime_unavailable(last_error)
    {
        // The reason as the runtime gave it: it is what an operator needs, and
        // the longest of them (the native ACP refusal) is about 230 characters.
        let detail: String = detail
            .trim()
            .chars()
            .take(LAUNCH_REFUSAL_DETAIL_CHARS)
            .collect();
        let bound = RUNTIME_UNAVAILABLE_MAX_DEFERRALS;
        return match lang {
            RoomLang::Fr => format!(
                "le runtime `{provider}` n'a pas pu démarrer ({detail}) ; Kronn réessaie toutes les 30 s ({deferrals}/{bound} essais) puis s'arrête"
            ),
            RoomLang::En => format!(
                "the `{provider}` runtime could not start ({detail}); Kronn retries every 30 s ({deferrals}/{bound} tries), then stops"
            ),
        };
    }
    if last_error.starts_with("audit_running") {
        return lang
            .pick(
                "un audit du projet écrit dans docs/ : le worker démarre à la fin de l'audit",
                "an audit of the project is writing docs/: the worker starts when it ends",
            )
            .to_string();
    }
    if wait.progress_detail.as_deref().is_some_and(|detail| {
        detail.starts_with(crate::db::agent_dispatch::TASK_WORKER_QUEUED_DETAIL_PREFIX)
    }) {
        let cap = limits
            .get(crate::db::agent_dispatch::TASK_WORKER_LIMIT_KEY)
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(wait.task_workers_running);
        let running = wait.task_workers_running;
        return match lang {
            RoomLang::Fr => format!(
                "tous les emplacements de workers locaux sont pris ({running}/{cap}) ; il démarre dès qu'un se libère"
            ),
            RoomLang::En => format!(
                "every local task-worker slot is taken ({running}/{cap}); it starts as soon as one frees"
            ),
        };
    }
    if let Some(cap) = limits.get(provider).and_then(serde_json::Value::as_i64) {
        if wait.provider_running >= cap {
            let running = wait.provider_running;
            return match lang {
                RoomLang::Fr => format!(
                    "`{provider}` fait déjà tourner {running} dispatch(es) sur {cap} autorisé(s) ; il démarre à la fin de l'un d'eux"
                ),
                RoomLang::En => format!(
                    "`{provider}` already runs {running} dispatch(es) of {cap} allowed; it starts when one ends"
                ),
            };
        }
    }
    if wait.available_at > now {
        return lang
            .pick(
                "il attend la fin d'un délai de reprise",
                "it is waiting out a retry delay",
            )
            .to_string();
    }
    if wait.waiting_ahead > 0 {
        let ahead = wait.waiting_ahead;
        return match lang {
            RoomLang::Fr => format!("{ahead} worker(s) plus ancien(s) attendent devant lui"),
            RoomLang::En => format!("{ahead} older task worker(s) are queued ahead of it"),
        };
    }
    lang.pick(
        "aucune cause enregistrée : le dispatcher ne l'a pas encore pris",
        "no cause recorded: the dispatcher has not claimed it yet",
    )
    .to_string()
}

/// KT-1758 — tell the parent room, once per dispatch, that a quota fallback is
/// still waiting to start after
/// [`FALLBACK_DISPATCH_WAIT_NOTICE_AFTER_SECONDS`], and why. The fallback exists
/// so work continues while the original provider is limited; a dispatch that
/// sits `Pending` silently defeats that without anyone noticing. Returns the
/// dispatches announced now.
pub async fn notify_waiting_fallback_dispatches(state: &AppState) -> Vec<String> {
    let now = chrono::Utc::now();
    let since = now - chrono::Duration::seconds(FALLBACK_DISPATCH_WAIT_NOTICE_AFTER_SECONDS);
    let waiting = match state
        .db
        .with_read_conn(move |conn| {
            crate::db::agent_dispatch::list_waiting_fallback_dispatches(conn, since)
        })
        .await
    {
        Ok(waiting) => waiting,
        Err(error) => {
            tracing::warn!("waiting fallback dispatch scan failed: {error}");
            return Vec::new();
        }
    };
    if waiting.is_empty() {
        return Vec::new();
    }
    let limits: serde_json::Value = {
        let config = state.config.read().await;
        serde_json::from_str(&crate::agents::runner::dispatch_concurrency_limits(&config))
            .unwrap_or_default()
    };
    let mut announced = Vec::new();
    for wait in waiting {
        let message_id = fallback_dispatch_waiting_message_id(&wait.dispatch_id);
        let lang = room_lang(&state.db, &wait.parent_discussion_id).await;
        let minutes = (now - wait.waiting_since).num_minutes().max(0);
        let reason = fallback_dispatch_wait_reason(&wait, &limits, now, lang);
        let content = match lang {
            RoomLang::Fr => format!(
                "**Repli quota — le worker n'a pas démarré**\n\nL'exécution `{}` a basculé sur `{}` mais son dispatch `{}` attend depuis {minutes} min sans avoir démarré : {reason}.",
                wait.execution_id, wait.provider, wait.dispatch_id
            ),
            RoomLang::En => format!(
                "**Quota fallback — the worker has not started**\n\nExecution `{}` moved to `{}` but its dispatch `{}` has been waiting {minutes} min without starting: {reason}.",
                wait.execution_id, wait.provider, wait.dispatch_id
            ),
        };
        let message = orchestrator_message(message_id.clone(), content);
        let room = wait.parent_discussion_id.clone();
        let posted = state
            .db
            .with_conn(move |conn| {
                // One line per dispatch: a replay or the next tick posts nothing.
                let already: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1)",
                    [&message_id],
                    |row| row.get(0),
                )?;
                if already {
                    return Ok(false);
                }
                crate::db::discussions::insert_message(conn, &room, &message)?;
                Ok(true)
            })
            .await;
        match posted {
            Ok(true) => announced.push(wait.dispatch_id),
            Ok(false) => {}
            Err(error) => tracing::warn!(
                dispatch_job_id = %wait.dispatch_id,
                "waiting fallback dispatch notice could not be persisted: {error}"
            ),
        }
    }
    announced
}

/// KT-1664 — the room line that closes a quota escalation the provider
/// re-check resolved. Its id carries the refused dispatch the escalation notice
/// (`provider-quota-escalation:<dispatch>`) is keyed by, so the room shows that
/// notice as settled and a replay cannot post it twice.
pub fn quota_resumed_message_id(refused_dispatch_id: &str) -> String {
    format!("provider-quota-resumed:{refused_dispatch_id}")
}

/// KT-1664 — a provider that answered the KT-1657 re-check closes the quota
/// escalation of `execution_id` (one that announced no reset): the execution
/// goes back to its own worker, in its kept child discussion and worktree, and
/// its parent room gets one line saying so. Never another provider. The
/// reassignment re-checks in its own transaction that the execution is still
/// parked on that quota, so an owner who acted meanwhile is never overridden.
pub async fn resume_quota_escalation_after_probe(
    state: &AppState,
    execution_id: &str,
) -> Result<()> {
    let id = execution_id.to_string();
    let (execution, selection) = state
        .db
        .with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &id)?
                .context("execution vanished before its quota resume")?;
            let selection = crate::models::CampaignWorkerSelection {
                target: worker_target_from_execution(&execution)?,
                model: execution.worker_model.clone(),
                profile_id: execution.worker_profile_id.clone(),
            };
            Ok((execution, selection))
        })
        .await?;
    let provider = crate::db::orchestration::agent_type_to_db(&selection.target.agent_type);
    reassign_native_execution_as(
        state,
        execution_id,
        selection,
        "the provider answers again: the quota is back",
        ReassignCause::QuotaProbed,
    )
    .await?;
    let lang = room_lang(&state.db, &execution.parent_discussion_id).await;
    let refused = execution.dispatch_job_id.as_deref().unwrap_or(execution_id);
    let message = orchestrator_message(
        quota_resumed_message_id(refused),
        match lang {
            RoomLang::Fr => format!(
                "**Quota rétabli — worker relancé**\n\nLe provider `{provider}` répond de nouveau : Kronn a relancé l'exécution `{execution_id}` sur le même worker, dans sa sous-discussion et son worktree existants. Plus aucune action humaine n'est requise pour cette escalade."
            ),
            RoomLang::En => format!(
                "**Quota back — worker resumed**\n\nProvider `{provider}` answers again: Kronn resumed execution `{execution_id}` on the same worker, in its existing child discussion and worktree. No human action is required for that escalation any more."
            ),
        },
    );
    let room = execution.parent_discussion_id.clone();
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::insert_message(conn, &room, &message)?;
            Ok(())
        })
        .await
    {
        tracing::warn!(
            execution_id,
            "quota resumed notice could not be persisted: {error}"
        );
    }
    Ok(())
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
            return_quota_escalations_past_reset(&state).await;
            notify_waiting_fallback_dispatches(&state).await;
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
                                        (
                                            execution.dispatch_job_id,
                                            execution.sub_discussion_id,
                                            execution.status,
                                        )
                                    },
                                ),
                            )
                        })
                        .await
                        .ok()
                        .flatten()
                };
                // KT-1293 — a delivered attempt waiting for its review keeps its
                // worker reachable (a `request_changes` wakes it): the total
                // deadline alerts the principal there, it never stops the run.
                let awaiting_review = keys.as_ref().is_some_and(|(_, _, status)| {
                    *status == crate::models::TaskExecutionStatus::AwaitingReview
                });
                let cancel_provider_run = || {
                    if let (Some((dispatch, child, _)), Ok(registry)) =
                        (keys.as_ref(), state.cancel_registry.lock())
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
                };
                // A total deadline cancels first, as it always did. A silent
                // worker (KT-1247) is settled FIRST — dispatch failed, execution
                // `Interrupted` — and only then is its process stopped, so the
                // run's own teardown finds the row already parked instead of
                // racing the interruption. A worker that produced activity in
                // the meantime is not applied, hence never stopped.
                if kind == crate::models::ExecutionTimeoutKind::TotalDuration && !awaiting_review {
                    cancel_provider_run();
                }
                let id = execution_id.clone();
                match state
                    .db
                    .with_conn(move |conn| {
                        crate::db::orchestration::apply_execution_timeout(conn, &id, kind)
                    })
                    .await
                {
                    Ok(applied) => {
                        if applied && kind == crate::models::ExecutionTimeoutKind::Activity {
                            cancel_provider_run();
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            execution_id,
                            ?kind,
                            "orchestration timeout apply failed: {error}"
                        );
                    }
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
                // KT-1266 — the run is gone (a restarted backend, a lost
                // stream): what the worker started under it is not.
                reap_task_worker_processes(&state, &job.id).await;
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
                        let job_id = &job.id;
                        let message = orchestrator_message(
                            format!("agent-dispatch-watchdog:{}", job.id),
                            match room_lang(&state.db, &discussion_id).await {
                                RoomLang::Fr => format!(
                                    "**Intervention humaine requise — agent bloqué**\n\nLe dispatch `{job_id}` n'a plus de processus vivant après son unique relance automatique. Kronn ne le relancera plus à l'aveugle. Vérifie le provider, puis relance ou réaffecte explicitement."
                                ),
                                RoomLang::En => format!(
                                    "**Human intervention required — agent stalled**\n\nDispatch `{job_id}` has no live process left after its single automatic retry. Kronn will not blindly retry it again. Check the provider, then explicitly retry or reassign."
                                ),
                            },
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

/// Recovery decision for an integration-saga origin, read from the real Git
/// target. Shared by boot classification and `/resume` so both apply one table.
fn integration_recovery_decision(
    execution: &TaskExecution,
    origin: TaskExecutionStatus,
    project_path: Option<&str>,
    target_branch: Option<&str>,
) -> (ExecutionRecoveryAction, String) {
    match (project_path, target_branch) {
        (None, _) => (
            ExecutionRecoveryAction::BlockMissingWorkspace,
            "the integration project no longer resolves; preserve checkpoints for repair".into(),
        ),
        (_, None) => (
            ExecutionRecoveryAction::AwaitHuman,
            "the integration target branch is no longer pinned".into(),
        ),
        (Some(project_path), Some(target)) => {
            let repo = scanner::resolve_host_path(project_path);
            let tip = worktree::resolve_commit(&repo, target).ok();
            // A target no worktree has checked out has nothing to be dirty:
            // only an unreadable or ambiguous target is an unknown state.
            let checkout = worktree::integration_target(&repo, target);
            let dirty = checkout
                .as_ref()
                .ok()
                .and_then(|target| worktree::integration_target_dirty_files(target).ok())
                .map(|files| !files.is_empty())
                .unwrap_or(true);
            // The target may have moved past a candidate it already contains
            // (the apply landed, then more work did): that apply is done.
            let applied = origin == TaskExecutionStatus::Applying
                && execution
                    .candidate_merge_sha
                    .as_deref()
                    .zip(tip.as_deref())
                    .is_some_and(|(merge, tip)| {
                        merge == tip || worktree::is_ancestor(&repo, merge, tip).unwrap_or(false)
                    });
            match checkout {
                Err(reason) => (ExecutionRecoveryAction::BlockMissingWorkspace, reason),
                Ok(_) if applied => (
                    ExecutionRecoveryAction::IdempotentClose,
                    "the real target already contains the candidate; close without replaying Git"
                        .into(),
                ),
                Ok(_) => match crate::models::saga_resume_action(
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
                },
            }
        }
    }
}

/// Keep a recovery decision only if `/resume` accepts it from `origin`; otherwise
/// park it for a human. An interrupted Blocked row resumes through its own restore
/// path, so its decision is not consumed as-is.
pub fn resumable_recovery(
    origin: TaskExecutionStatus,
    action: ExecutionRecoveryAction,
    reason: String,
) -> (ExecutionRecoveryAction, String) {
    if origin == TaskExecutionStatus::Blocked || action.resumable_from(origin) {
        (action, reason)
    } else {
        (
            ExecutionRecoveryAction::AwaitHuman,
            format!(
                "{} cannot resume from {}: {reason}",
                action.as_str(),
                origin.as_str()
            ),
        )
    }
}

/// Re-derive the integration recovery decision of an interrupted execution from
/// its current origin and the real Git target, and persist it as pending. A
/// decision taken before a later interruption otherwise names a refused resume.
async fn refresh_integration_recovery(
    db: &Database,
    exec_id: &str,
) -> Result<crate::models::TaskExecutionRecovery> {
    let id = exec_id.to_string();
    let (execution, run, project_path) = db
        .with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &id)?
                .context("interrupted execution vanished")?;
            let run = crate::db::orchestration::get_orchestration_run(
                conn,
                &execution.orchestration_run_id,
            )?
            .context("orchestration run vanished")?;
            let project_path = run
                .project_id
                .as_deref()
                .map(|project_id| crate::db::projects::get_project(conn, project_id))
                .transpose()?
                .flatten()
                .map(|project| project.path);
            Ok((execution, run, project_path))
        })
        .await?;
    let origin = execution
        .interrupted_from_status
        .filter(|_| execution.status == TaskExecutionStatus::Interrupted)
        .context("execution is not interrupted")?;
    if !matches!(
        origin,
        TaskExecutionStatus::Integrating
            | TaskExecutionStatus::Validating
            | TaskExecutionStatus::Applying
    ) {
        bail!("{} is not an integration origin", origin.as_str());
    }
    let (action, reason) = integration_recovery_decision(
        &execution,
        origin,
        project_path.as_deref(),
        run.target_branch.as_deref(),
    );
    let (action, reason) = resumable_recovery(origin, action, reason);
    db.with_conn(move |conn| {
        crate::db::orchestration::set_execution_recovery(conn, &execution, &run, action, &reason)
    })
    .await
}

/// KT-1537 — give an `Interrupted` execution whose decision is consumed (or
/// absent) a fresh pending one, so `/resume` never answers « already consumed »
/// to an interruption nobody has decided yet.
///
/// Only an undelivered `resume_worker` is re-armed as is (a restart
/// classification would instead park it for a human, and it has no state to
/// re-read). Every other decision predates this interruption and is re-derived:
/// from the real Git target for an integration origin, from the child room, the
/// worktree and the CLI for any other.
async fn rearm_stranded_interruption(state: &AppState, exec_id: &str) -> Result<()> {
    let db = &state.db;
    let id = exec_id.to_string();
    if db
        .with_conn(move |conn| crate::db::orchestration::rearm_consumed_recovery(conn, &id))
        .await?
    {
        return Ok(());
    }
    let id = exec_id.to_string();
    let origin = db
        .with_conn(move |conn| {
            Ok(crate::db::orchestration::get_task_execution(conn, &id)?
                .and_then(|execution| execution.interrupted_from_status))
        })
        .await?
        .context("Interrupted execution has no durable origin")?;
    if matches!(
        origin,
        TaskExecutionStatus::Integrating
            | TaskExecutionStatus::Validating
            | TaskExecutionStatus::Applying
    ) {
        refresh_integration_recovery(db, exec_id).await?;
    } else {
        let available_agents = available_agent_types_for_state(state).await;
        classify_interrupted_execution(db, exec_id, &available_agents).await?;
    }
    Ok(())
}

pub async fn classify_interrupted_execution(
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
                     WHERE id = ?1 AND (disc_id = ?2 OR disc_id = ?4) AND agent_type = ?3 \
                       AND status <> 'left')",
                    // Accepted offers move this exact joined session into the
                    // pinned child. A backend restart must recognize that
                    // handoff, never a same-provider substitute or third room.
                    rusqlite::params![
                        session,
                        execution.parent_discussion_id,
                        agent_type,
                        execution.sub_discussion_id,
                    ],
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

    // KT-1540 — an undelivered worker is parked as a pending `resume_worker` for
    // the principal; a restart must still not wake it on its own.
    let durable_human_gate = prior_recovery.as_ref().filter(|recovery| {
        (recovery.recovery_action == ExecutionRecoveryAction::AwaitHuman
            && (crate::db::orchestration::is_undelivered_worker_reason(&recovery.recovery_reason)
                || recovery.recovery_reason.starts_with("quota_exhausted:")))
            || (recovery.recovery_action == ExecutionRecoveryAction::ResumeWorker
                && crate::db::orchestration::is_undelivered_worker_reason(
                    &recovery.recovery_reason,
                ))
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
        integration_recovery_decision(
            &execution,
            origin,
            project_path.as_deref(),
            run.target_branch.as_deref(),
        )
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
    let (action, reason) = resumable_recovery(interrupted_origin, action, reason);
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
#[cfg(any(test, feature = "test-support"))]
pub fn handoff_notice(has_delivered: bool, generation: Option<u32>) -> String {
    handoff_notice_with_context(has_delivered, generation, None, None)
}

#[cfg(any(test, feature = "test-support"))]
pub fn handoff_notice_with_context(
    has_delivered: bool,
    generation: Option<u32>,
    tx: Option<&rusqlite::Connection>,
    execution_id: Option<&str>,
) -> String {
    handoff_notice_in(RoomLang::Fr, has_delivered, generation, tx, execution_id)
}

pub fn handoff_notice_in(
    lang: RoomLang,
    has_delivered: bool,
    generation: Option<u32>,
    tx: Option<&rusqlite::Connection>,
    execution_id: Option<&str>,
) -> String {
    // KT-1408 — an earlier generation the execution time limit stopped: the
    // handoff names how long it ran and the WIP checkpoint it left, and never
    // claims "no work has been recorded" over a commit that holds it.
    let limit_note = tx.zip(execution_id).and_then(|(tx, exec_id)| {
        crate::api::worker_time_limit::handoff_note(tx, exec_id, lang == RoomLang::Fr)
    });
    let Some(limit_note) = limit_note else {
        return handoff_notice_base_in(lang, has_delivered, generation, tx, execution_id);
    };
    if has_delivered {
        let base = handoff_notice_base_in(lang, has_delivered, generation, tx, execution_id);
        return format!("{base}\n\n{limit_note}");
    }
    let suffix = generation
        .map(|number| format!(" — {} {number}", lang.pick("génération", "generation")))
        .unwrap_or_default();
    let title = lang.pick("Reprends la tâche", "Resume the task");
    format!("**{title}{suffix}**\n\n{limit_note}")
}

fn handoff_notice_base_in(
    lang: RoomLang,
    has_delivered: bool,
    generation: Option<u32>,
    tx: Option<&rusqlite::Connection>,
    execution_id: Option<&str>,
) -> String {
    if lang == RoomLang::En {
        return handoff_notice_en(has_delivered, generation, tx, execution_id);
    }
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

/// [`handoff_notice_in`] for an English room.
fn handoff_notice_en(
    has_delivered: bool,
    generation: Option<u32>,
    tx: Option<&rusqlite::Connection>,
    execution_id: Option<&str>,
) -> String {
    let suffix = match generation {
        Some(number) => format!(" — generation {number}"),
        None => String::new(),
    };
    if has_delivered {
        return format!(
            "**Resume{suffix}**\n\n\
             Resume exactly the unfinished step in this same worktree. \
             The history, the manifests, the findings and the SHAs already persisted \
             remain authoritative; do not replay an action already proven."
        );
    }
    let git_status = match (tx, execution_id) {
        (Some(tx), Some(exec_id)) => Some(check_dirty_files_for_handoff(tx, exec_id)),
        _ => None,
    };
    match git_status {
        Some(GitStatusForHandoff::Dirty(file_count)) => format!(
            "**Start the task{suffix}**\n\n\
             Uncommitted work is waiting for you in this worktree ({file_count} file{}).\n\
             Read the `git_diff` of what precedes, complete it, then use the declared commit tool \
             (`task_exec_commit` for a CLI host, `git_commit` for an HTTP agent) with a message \
             that explains the context — what the previous generation left unfinished and \
             why you are finishing it. Then deliver with `task_exec_deliver`.",
            if file_count == 1 { "" } else { "s" }
        ),
        Some(GitStatusForHandoff::InspectionFailed) => format!(
            "**Start the task{suffix}**\n\n\
             The worktree inspection could not fully verify its state. \
             First check with `git status` and `git diff` whether work is waiting for you: \
             read the changes, complete them if needed, then proceed accordingly."
        ),
        _ => format!(
            "**Start the task{suffix}**\n\n\
             No work has been recorded yet for this execution: \
             no delivery, nothing to resume.\n\n\
             The shortest path: `search_text` to find the spot, \
             `read_file` with `offset` and `limit` to read that region only, \
             `edit_file` to replace exactly that passage — never any need to \
             read or rewrite the whole file —, the declared commit tool \
             (`task_exec_commit` or `git_commit`), then deliver with \
             `task_exec_deliver`."
        ),
    }
}

/// KT-1713 — the room line saying a task worker went back to its declared
/// worker once the quota window behind its fallback ended. It names the
/// execution and that it is a task worker, like the fallback notice does.
pub fn fallback_return_notice(
    execution_id: &str,
    from_provider: &str,
    declared_provider: &str,
) -> String {
    format!(
        "Task worker execution {execution_id}: the quota window that moved it to {from_provider} \
         has ended → back on its declared worker {declared_provider}"
    )
}

/// KT-1713 — resume of an `Interrupted` execution that a quota fallback moved
/// off its declared worker: when the window behind that fallback is over, hand
/// the work back to the declared worker (same child room and worktree) and say
/// so in both rooms, instead of waking the alternative again. `None`: nothing
/// to return (no fallback, window still open, already home) or the return was
/// refused, in which case the ordinary wake proceeds on the current worker.
async fn return_lapsed_fallback_on_resume(state: &AppState, exec_id: &str) -> Option<String> {
    let id = exec_id.to_string();
    let found = state
        .db
        .with_conn(move |conn| {
            let origin =
                crate::db::orchestration::lapsed_fallback_origin(conn, &id, chrono::Utc::now())?;
            let execution = crate::db::orchestration::get_task_execution(conn, &id)?;
            Ok(origin.zip(execution))
        })
        .await
        .ok()
        .flatten()?;
    let (origin, execution) = found;
    let from = execution.worker_agent_type.clone().unwrap_or_default();
    let notice = fallback_return_notice(exec_id, &from, &origin.provider);
    let view = match reassign_native_execution_as(
        state,
        exec_id,
        origin.selection,
        &notice,
        ReassignCause::FallbackLapsed,
    )
    .await
    {
        Ok(view) => view,
        Err(error) => {
            tracing::warn!(
                execution_id = exec_id,
                "fallback return to the declared worker refused on resume: {error}"
            );
            return None;
        }
    };
    let message = orchestrator_message(
        format!(
            "agent-fallback-return:{exec_id}:{}",
            view.execution
                .dispatch_job_id
                .as_deref()
                .unwrap_or_default()
        ),
        notice.clone(),
    );
    let room = execution.parent_discussion_id.clone();
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::insert_message(conn, &room, &message)?;
            Ok(())
        })
        .await
    {
        tracing::warn!(
            execution_id = exec_id,
            "fallback return notice could not be persisted: {error}"
        );
    }
    state.agent_dispatch_notify.notify_one();
    Some(format!("returned to the declared worker: {notice}"))
}

pub async fn wake_recovered_worker(db: &Database, exec_id: &str) -> Result<String> {
    let id = exec_id.to_string();
    let execution = db
        .with_conn(move |conn| {
            crate::db::orchestration::get_task_execution(conn, &id)?
                .context("execution vanished before worker wake")
        })
        .await?;
    let lang = room_lang(db, &execution.parent_discussion_id).await;
    let scope_note = reanchor_execution_scope(db, &execution)
        .await?
        .map(|reanchored| reanchored_scope_note(&reanchored, lang));
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
            let mut content = handoff_notice_in(lang, has_delivered, None, Some(&tx), Some(&id));
            content.push_str(scope_note.as_deref().unwrap_or_default());
            let message = orchestrator_message(message_id, content);
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

pub async fn wake_recovered_principal(db: &Database, exec_id: &str) -> Result<String> {
    let id = exec_id.to_string();
    db.with_conn(move |conn| {
        let tx = conn.unchecked_transaction()?;
        let execution = crate::db::orchestration::get_task_execution(&tx, &id)?
            .context("execution vanished before principal wake")?;
        // A delegation step reviews on its own resume; an agent turn here would
        // be the paid orchestrator KT-909 removes, deciding in its place.
        if crate::workflows::delegate_subtasks_step::is_delegation_discussion(
            &execution.parent_discussion_id,
        ) {
            tx.commit()?;
            return Ok("left to the delegating workflow step".into());
        }
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
                match RoomLang::from_code(&principal.language) {
                    RoomLang::Fr => format!(
                        "**Revue à reprendre après redémarrage**\n\n\
                         L'exécution `{id}` possède déjà une livraison durable. Relis le manifeste, \
                         les constats et les SHA persistés puis rends la décision de revue ; ne demande \
                         pas au worker de rejouer son travail."
                    ),
                    RoomLang::En => format!(
                        "**Review to resume after restart**\n\n\
                         Execution `{id}` already has a durable delivery. Read the manifest, \
                         the findings and the persisted SHAs again, then give the review decision; \
                         do not ask the worker to replay its work."
                    ),
                },
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
pub struct Prepared {
    pub execution: TaskExecution,
    pub project_id: String,
    pub repo_path: String,
    pub task_reference: String,
    pub task_title: String,
    pub task_description: String,
    pub dod: Vec<PlanningDodItem>,
    /// KT-1688 — the validated integration branch the base commit is pinned
    /// from; never the `main` fallback this used to be.
    pub target_branch: String,
    /// The idempotent replay found an execution already past `Provisioning`
    /// (running/terminal) — the saga returns it untouched.
    pub already_launched: bool,
}

/// Serde hook for the `max_review_rounds` fields: a plain `Option` reads an
/// explicit `null` as absent, which would silently apply the default. Here only
/// a missing key is `None`; `null` stays a value, and [`review_rounds_argument`]
/// refuses it like any other non-integer.
fn explicit_review_rounds<'de, D>(deserializer: D) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <serde_json::Value as Deserialize>::deserialize(deserializer).map(Some)
}

/// KT-1345 — the optional `max_review_rounds` of a launch: absent is fine (the
/// Settings > Workers default applies), anything that is not an integer in
/// range or `"unlimited"` is refused rather than defaulted.
pub fn review_rounds_argument(value: Option<&serde_json::Value>) -> Result<Option<u32>, String> {
    value
        .map(crate::models::parse_review_rounds_arg)
        .transpose()
}

/// Launch a single ready task. A native worker drives to `Working` (its task
/// `InProgress`); a `Cli` worker opens a durable control offer in the origin room
/// and parks `Blocked(awaiting_worker_acceptance)` until the exact session accepts
/// (KT-328 tranche 2). On any refusal/failure the returned error is structured and
/// the durable state is resumable (DoD-6).
///
/// Takes no `AppState`, so it cannot read Settings > Workers: the execution gets
/// the built-in review-round default. Production launches go through
/// [`provision_single_task_execution_with_options`]; this is the entry point of
/// the integration tests.
pub async fn provision_single_task_execution(
    db: &Database,
    input: ProvisionInput,
) -> Result<TaskExecution, ProvisionError> {
    provision_task_execution_inner(
        db,
        input,
        None,
        None,
        None,
        Vec::new(),
        SingleTaskLaunchOptions::default(),
    )
    .await
}

/// Launch one task with mechanical gates selected by the authorized principal.
/// The worker cannot influence this policy through its brief or delivery manifest.
#[cfg(any(test, feature = "test-support"))]
pub async fn provision_single_task_execution_with_validations(
    db: &Database,
    input: ProvisionInput,
    validations: Vec<crate::models::ValidationSpec>,
) -> Result<TaskExecution, ProvisionError> {
    provision_task_execution_inner(
        db,
        input,
        None,
        None,
        None,
        validations,
        SingleTaskLaunchOptions::default(),
    )
    .await
}

#[cfg(any(test, feature = "test-support"))]
pub async fn provision_single_task_execution_with_scope_and_validations(
    db: &Database,
    input: ProvisionInput,
    worker_scope: Option<TaskWorkerScope>,
    validations: Vec<crate::models::ValidationSpec>,
) -> Result<TaskExecution, ProvisionError> {
    provision_task_execution_inner(
        db,
        input,
        None,
        None,
        worker_scope,
        validations,
        SingleTaskLaunchOptions::default(),
    )
    .await
}

/// KT-1345 — the review-round limit of a single-task launch. `Some` is the
/// caller's explicit value; `None` falls back to Settings > Workers.
///
/// KT-1564 — `quota_fallback_agents` is the launch's own, explicit opt-in to a
/// provider fallback on quota: the providers it names, in preference order.
/// Empty (the default) never falls back to another provider.
#[derive(Debug, Clone, Default)]
pub struct SingleTaskLaunchOptions {
    pub max_review_rounds: Option<u32>,
    pub quota_fallback_agents: Vec<crate::models::AgentType>,
}

/// Launch one task with the worker scope and gates the principal chose, and the
/// review-round limit resolved: the explicit one, else the Settings > Workers
/// default. Every principal-driven launch (MCP, native tool, REST) comes here.
pub async fn provision_single_task_execution_with_options(
    state: &AppState,
    input: ProvisionInput,
    worker_scope: Option<TaskWorkerScope>,
    validations: Vec<crate::models::ValidationSpec>,
    options: SingleTaskLaunchOptions,
) -> Result<TaskExecution, ProvisionError> {
    let rounds = match options.max_review_rounds {
        Some(rounds) => rounds,
        None => state
            .config
            .read()
            .await
            .workers
            .effective_max_review_rounds(),
    };
    provision_task_execution_inner(
        &state.db,
        input,
        None,
        None,
        worker_scope,
        validations,
        SingleTaskLaunchOptions {
            max_review_rounds: Some(rounds),
            quota_fallback_agents: options.quota_fallback_agents,
        },
    )
    .await
}

pub async fn resume_provisioning_execution(
    db: &Database,
    input: ProvisionInput,
    execution_id: &str,
) -> Result<TaskExecution, ProvisionError> {
    provision_task_execution_inner(
        db,
        input,
        None,
        Some(execution_id),
        None,
        Vec::new(),
        SingleTaskLaunchOptions::default(),
    )
    .await
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
        SingleTaskLaunchOptions::default(),
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
    options: SingleTaskLaunchOptions,
) -> Result<TaskExecution, ProvisionError> {
    let SingleTaskLaunchOptions {
        max_review_rounds,
        quota_fallback_agents,
    } = options;
    // Phases A–D are worker-agnostic: the worker kind forks ONLY at Phase E (native
    // dispatch vs. CLI control offer), so a Cli worker provisions its sub-discussion
    // and worktree exactly like a native one and diverges only at the final step.

    // KT-1646 — over the worktree cap, free the clean worktrees of finished
    // executions first, with no connection held, so Phase A does not refuse.
    reclaim_finished_worktrees_for_launch(db, &input.task_reference, &input.worker).await;

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
        let quota_fallback_agents = quota_fallback_agents.clone();
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
                    max_review_rounds,
                    quota_fallback_agents: &quota_fallback_agents,
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

    // ── Phase A′ (KT-1318) — the disk must have room for the worker's lane.
    // Below `server.disk_launch_min_free_gib` Kronn sweeps its orphaned lane
    // build dirs first; still below, it refuses BEFORE creating anything, parks
    // the execution `Blocked` (resumable) and tells the principal room — a
    // worker that dies mid-build on "No space left on device" costs far more.
    if let Err(pressure) =
        crate::api::disk::ensure_launch_headroom(db, &prepared.project_id, &repo_path).await
    {
        let reason = pressure.reason("launch a task worker");
        mark_blocked(
            db,
            &prepared.execution.id,
            format!("disk pressure: {reason}"),
        )
        .await;
        post_disk_pressure_alert(
            db,
            &input.parent_discussion_id,
            &prepared.task_reference,
            &reason,
        )
        .await;
        return Err(ProvisionError::DiskPressure(reason));
    }

    // ── Phase B — pin the base commit (git). Nothing external created yet, so a
    // failure only marks the execution resumable. ──
    let base_rev = prepared.target_branch.clone();
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
        // KT-1637 — only a checkout this launch really makes is timed: adopting
        // the one a prior attempt made would overwrite its duration with the
        // few milliseconds of a HEAD check.
        let mut worktree_created_ms: Option<u64> = None;
        let create_result = if worktree_path.exists() {
            worktree::verify_worktree_head(&worktree_path, &base_sha)
                .and_then(|_| worktree::materialize_task_submodules(&repo_path, &worktree_path))
        } else {
            let creating = std::time::Instant::now();
            let created = worktree::create_task_worktree(
                &repo_path,
                &prepared.task_reference,
                exec_short(&prepared.execution.id),
                &base_sha,
            )
            .map(|_| ());
            worktree_created_ms = Some(creating.elapsed().as_millis() as u64);
            created
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
        if let Some(duration_ms) = worktree_created_ms {
            record_warming_step(
                db,
                &prepared.execution.id,
                crate::models::WarmingStepKind::WorktreeCreation,
                duration_ms,
            )
            .await;
        }
    }

    // The project's generated, gitignored assets (KT-1438), whatever the worker
    // kind: a fresh worktree has the tracked tree only. A resume refreshes them.
    if let Some(duration_ms) = seed_lane_assets(
        db,
        &prepared.execution.id,
        &prepared.project_id,
        &repo_path,
        &worktree_path,
    )
    .await
    {
        record_warming_step(
            db,
            &prepared.execution.id,
            crate::models::WarmingStepKind::LaneSeeds,
            duration_ms,
        )
        .await;
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

    // A joined CLI worker keeps ownership of its principal room. Instead of a native
    // child-room dispatch, open a durable CONTROL OFFER in that room, targeted at the
    // exact session, and park the execution `Blocked(awaiting_worker_acceptance)`;
    // acceptance grants the execution-scoped worker role and drives it to Working.
    // Phases A–D already provisioned its sub-discussion + worktree.
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

    let launch = NativeLaunch {
        prepared,
        target,
        sub_disc_id,
        worktree_path,
        branch,
        base_sha,
        warm_cancel: tokio_util::sync::CancellationToken::new(),
    };

    // KT-1250 — a lane that has a first build to run does not hold the caller
    // for it (a 30-minute build behind an MCP call times the client out, and
    // eight lanes would launch one after the other). The execution and its
    // worktree are persisted; the warm-up, smoke check, brief and dispatch
    // continue in a background task, the execution `Provisioning` meanwhile.
    if launch.is_claude_worker()
        && crate::core::task_worker_lane_warm::first_build_pending(&launch.worktree_path)
    {
        return defer_native_provisioning(db, launch).await;
    }
    finish_native_provisioning(db, launch).await
}

/// What Phase E needs once the worktree exists, owned so the warm-up can move
/// into a background task.
struct NativeLaunch {
    prepared: Prepared,
    target: MessageTarget,
    sub_disc_id: String,
    worktree_path: std::path::PathBuf,
    branch: String,
    base_sha: String,
    /// Fired when the execution is cancelled while its lane warms: stops the
    /// queued or running first build.
    warm_cancel: tokio_util::sync::CancellationToken,
}

impl NativeLaunch {
    fn is_claude_worker(&self) -> bool {
        self.prepared.execution.worker_agent_type.as_deref() == Some("ClaudeCode")
    }
}

/// Executions whose warm-up THIS backend process is running. A second launch
/// or resume of the same execution while one is in flight must not warm (and
/// dispatch) it twice; a process restart empties the set, which is exactly
/// when the boot recovery resumes the execution.
///
/// Each entry carries the warm-up's cancellation: firing it stops the queued or
/// running first build and frees its gate slot.
pub fn warming_executions(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, tokio_util::sync::CancellationToken>>
{
    static WARMING: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, tokio_util::sync::CancellationToken>>,
    > = std::sync::OnceLock::new();
    WARMING.get_or_init(Default::default)
}

struct WarmingClaim {
    exec_id: String,
    cancel: tokio_util::sync::CancellationToken,
}

impl WarmingClaim {
    fn take(exec_id: &str) -> Option<Self> {
        let mut warming = warming_executions()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if warming.contains_key(exec_id) {
            return None;
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        warming.insert(exec_id.to_string(), cancel.clone());
        Some(Self {
            exec_id: exec_id.to_string(),
            cancel,
        })
    }
}

impl Drop for WarmingClaim {
    fn drop(&mut self) {
        warming_executions()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.exec_id);
    }
}

/// Stop the warm-up this process runs for `exec_id`, if any. `true` when one
/// was signalled.
fn signal_warm_up_cancel(exec_id: &str) -> bool {
    let token = warming_executions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(exec_id)
        .cloned();
    match token {
        Some(token) => {
            token.cancel();
            true
        }
        None => false,
    }
}

/// Wait until `exec_id`'s warm-up has let go of its claim (its first build is
/// killed and its gate slot free), up to `timeout`.
async fn wait_for_warm_up_to_settle(exec_id: &str, timeout: std::time::Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let warming = warming_executions()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(exec_id);
        if !warming {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// How often the background warm-up checks the execution is still
/// `Provisioning` (any cancel path — a run cancel, a reassignment — ends it).
const WARM_UP_STATE_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// The poll interval of `repo_path`'s warm-up watcher. Tests may pin it per
/// repository, so a scenario that depends on the watcher NOT having fired yet
/// does not race the wall clock.
fn warm_up_state_poll(repo_path: &str) -> std::time::Duration {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(poll) = test_warm_up_state_polls()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(repo_path)
    {
        return *poll;
    }
    let _ = repo_path;
    WARM_UP_STATE_POLL
}

#[cfg(any(test, feature = "test-support"))]
pub fn test_warm_up_state_polls(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, std::time::Duration>> {
    static POLLS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::time::Duration>>,
    > = std::sync::OnceLock::new();
    POLLS.get_or_init(Default::default)
}

/// The probe runner of a task worker's launch: real processes confined to the
/// worker's sandbox profile (KT-1255), so a build script or proc-macro the
/// worker wrote is never compiled with the host's rights. Tests register a
/// runner per repository to control the first build.
pub fn worker_probe_runner(
    repo_path: &str,
    worktree: &std::path::Path,
) -> std::sync::Arc<dyn crate::core::task_worker_env::ProbeRunner> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(runner) = test_probe_runners()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(repo_path)
    {
        return runner.clone();
    }
    let _ = repo_path;
    std::sync::Arc::new(
        crate::core::task_worker_sandbox::SandboxedProbeRunner::for_worktree(worktree),
    )
}

#[cfg(any(test, feature = "test-support"))]
pub fn test_probe_runners() -> &'static std::sync::Mutex<
    std::collections::HashMap<
        String,
        std::sync::Arc<dyn crate::core::task_worker_env::ProbeRunner>,
    >,
> {
    static RUNNERS: std::sync::OnceLock<
        std::sync::Mutex<
            std::collections::HashMap<
                String,
                std::sync::Arc<dyn crate::core::task_worker_env::ProbeRunner>,
            >,
        >,
    > = std::sync::OnceLock::new();
    RUNNERS.get_or_init(Default::default)
}

/// Hand the rest of the launch to a background task and return the execution
/// as it is: `Provisioning`, journalled as warming.
///
/// * A backend restart during the warm-up is the ordinary boot recovery: the
///   execution is interrupted, classified `ResumeProvisioning` and re-enters
///   the saga, which skips what is persisted and warms again. The dispatch is
///   only ever made by the `Provisioning → Working` checkpoint, a compare and
///   swap, so a resume cannot dispatch twice.
/// * A warm-up that fails or times out is recorded and the worker is still
///   dispatched, cold (see `warm_lane`).
/// * An error of the background task parks the execution `Blocked`, resumable,
///   never leaves it silently `Provisioning`.
async fn defer_native_provisioning(
    db: &Database,
    mut launch: NativeLaunch,
) -> Result<TaskExecution, ProvisionError> {
    let exec_id = launch.prepared.execution.id.clone();
    let Some(claim) = WarmingClaim::take(&exec_id) else {
        // This process is already warming it: report where it stands.
        return db
            .with_conn(move |conn| {
                crate::db::orchestration::get_task_execution(conn, &exec_id)?
                    .context("execution vanished while its lane warms")
            })
            .await
            .map_err(|e| ProvisionError::Internal(e.to_string()));
    };
    let bound =
        crate::core::task_worker_lane_warm::WarmStartSettings::for_worktree(&launch.worktree_path)
            .first_build_timeout
            .as_secs();
    let journal_id = exec_id.clone();
    db.with_conn(move |conn| {
        crate::db::orchestration::record_lane_warming(conn, &journal_id, bound, &backend_actor())
    })
    .await
    .map_err(|e| ProvisionError::Internal(e.to_string()))?;

    launch.warm_cancel = claim.cancel.clone();

    let background = std::sync::Arc::new(db.share_handle());
    let task_db = background.clone();
    let id = exec_id.clone();
    let state_poll = warm_up_state_poll(&launch.prepared.repo_path);
    tokio::spawn(async move {
        let _claim = claim;
        // Any cancel path (the execution's own, its run's, a reassignment)
        // ends with the execution no longer `Provisioning`: stop the build
        // then, not only when this endpoint's token fires.
        let watcher = tokio::spawn({
            let db = task_db.clone();
            let id = id.clone();
            let cancel = launch.warm_cancel.clone();
            async move {
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(state_poll) => {}
                    }
                    let probe_id = id.clone();
                    let status = db
                        .with_conn(move |conn| {
                            Ok(
                                crate::db::orchestration::get_task_execution(conn, &probe_id)?
                                    .map(|execution| execution.status),
                            )
                        })
                        .await;
                    match status {
                        Ok(Some(TaskExecutionStatus::Provisioning)) | Err(_) => {}
                        Ok(_) => {
                            cancel.cancel();
                            return;
                        }
                    }
                }
            }
        });
        let work = tokio::spawn({
            let db = task_db.clone();
            async move { finish_native_provisioning(&db, launch).await }
        });
        let outcome = work.await;
        watcher.abort();
        let failure = match outcome {
            Ok(Ok(_)) => None,
            // Settled where it happened: the checkpoint refusal parked the
            // execution, a raced cancel/interrupt owns its own state.
            Ok(Err(ProvisionError::CheckpointRefused(_))) => None,
            Ok(Err(error)) => Some(provision_error_parts(&error).1),
            Err(join) => Some(format!("the warm-up task did not complete: {join}")),
        };
        if let Some(reason) = failure {
            tracing::warn!(execution_id = %id, "lane warm-up failed: {reason}");
            mark_blocked(
                &task_db,
                &id,
                format!("launch failed after the lane warm-up: {reason}"),
            )
            .await;
        }
    });

    db.with_conn(move |conn| {
        crate::db::orchestration::get_task_execution(conn, &exec_id)?
            .context("execution vanished while its lane starts warming")
    })
    .await
    .map_err(|e| ProvisionError::Internal(e.to_string()))
}

/// Phase E for a native worker: warm the lane (Claude), smoke-check it, write
/// the brief and take the single atomic checkpoint that dispatches the worker.
async fn finish_native_provisioning(
    db: &Database,
    launch: NativeLaunch,
) -> Result<TaskExecution, ProvisionError> {
    let NativeLaunch {
        prepared,
        target,
        sub_disc_id,
        worktree_path,
        branch,
        base_sha,
        warm_cancel,
    } = launch;
    let worktree_path_str = worktree_path.to_string_lossy().to_string();

    // KT-1246 — a Claude task worker builds and tests in its own lane. The lane
    // is provisioned and smoke-checked here, before the brief is written, so the
    // brief states what the worker can really run. Other runtimes are unchanged.
    let toolchain = if prepared.execution.worker_agent_type.as_deref() == Some("ClaudeCode") {
        // KT-1256 — the lane target is stamped with the execution it was made
        // for before anything is built in it: that marker is the only thing
        // that lets a later sweep treat the directory as Kronn's to remove.
        mark_lane_target_for_execution(
            std::path::Path::new(&prepared.repo_path),
            &worktree_path,
            &prepared.execution.id,
        )
        .await;
        // KT-1649 — what the worker's lock files reference and the host caches
        // lack is fetched here, outside its sandbox and from the registries of
        // the lock files only, so the first build and the worker can compile it.
        record_dependency_fetch(
            db,
            &prepared.execution.id,
            &worktree_path,
            std::sync::Arc::new(crate::core::task_worker_env::ProcessProbeRunner),
            warm_cancel.clone(),
        )
        .await;
        // KT-1250 — the lane target is seeded from a warm one and its first build
        // runs here, under the host's cap, so the worker starts warm.
        // KT-1980 — unless the execution's validations (principal-owned) start no
        // build: then the lane stays cold and the record says why.
        let warm = warm_lane_for_validations(
            db,
            &prepared.execution.id,
            prepared.execution.attempt_no,
            &sub_disc_id,
            &worktree_path,
            &prepared.repo_path,
            warm_cancel,
        )
        .await;
        let cold = warm
            .as_ref()
            .and_then(|record| record.skipped_by_validations.clone());
        // KT-1736 — a first build that failed or timed out hands the worker a cold
        // lane (tens of minutes before its first test): say so in its room, with
        // the cause, instead of leaving it to find out.
        if let Some(record) = &warm {
            let lang = room_lang(db, &prepared.execution.parent_discussion_id).await;
            if let Some(content) = warm_failure_notice(lang, &record.first_build) {
                post_warm_failure_notice(
                    db,
                    &prepared.execution.id,
                    prepared.execution.attempt_no,
                    &sub_disc_id,
                    content,
                )
                .await;
            }
        }
        // Cancelled while it warmed: nothing more to run for this execution (the
        // cancel owns its state; the first build is already stopped).
        if warm.as_ref().is_some_and(|record| {
            matches!(
                record.first_build,
                crate::core::task_worker_lane_warm::FirstBuild::Cancelled { .. }
            )
        }) {
            return Err(ProvisionError::CheckpointRefused(
                "cancelled while the lane warmed".into(),
            ));
        }
        let smoke_check = std::time::Instant::now();
        let capabilities = record_toolchain_smoke_check_on(
            db,
            &prepared.execution.id,
            &worktree_path,
            worker_probe_runner(&prepared.repo_path, &worktree_path),
            cold.as_ref(),
        )
        .await;
        record_warming_step(
            db,
            &prepared.execution.id,
            crate::models::WarmingStepKind::SmokeCheck,
            smoke_check.elapsed().as_millis() as u64,
        )
        .await;
        Some(capabilities)
    } else {
        None
    };

    // ── Native worker: the single atomic "durably launchable" checkpoint. ──
    let lang = room_lang(db, &prepared.execution.parent_discussion_id).await;
    let brief = build_brief(
        &prepared,
        &worktree_path_str,
        &branch,
        &base_sha,
        toolchain.as_ref(),
        lang,
    );
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

/// A prelocalized target that no longer fits the file the next round reads.
#[derive(Debug)]
pub struct StaleWorkerScope(pub String);

impl std::fmt::Display for StaleWorkerScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StaleWorkerScope {}

fn describe_worker_scope(scope: &TaskWorkerScope) -> String {
    match scope {
        TaskWorkerScope::PrelocalizedEdit {
            start_line,
            end_line,
            ..
        } => format!("lines {start_line}-{end_line}"),
        TaskWorkerScope::PrelocalizedInsertAfter { anchor_line, .. } => {
            format!("anchor line {anchor_line}")
        }
    }
}

fn worker_scope_path(scope: &TaskWorkerScope) -> &str {
    match scope {
        TaskWorkerScope::PrelocalizedEdit { path, .. }
        | TaskWorkerScope::PrelocalizedInsertAfter { path, .. } => path,
    }
}

/// The lines that differ between `base` and `current`, numbered in `current`.
fn observed_line_change(base: &[&[u8]], current: &[&[u8]]) -> String {
    let window = |start: usize, end: usize| {
        if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        }
    };
    let prefix = base
        .iter()
        .zip(current)
        .take_while(|(left, right)| left == right)
        .count();
    let room = base.len().min(current.len()) - prefix;
    let suffix = base
        .iter()
        .rev()
        .zip(current.iter().rev())
        .take(room)
        .take_while(|(left, right)| left == right)
        .count();
    if current.len() - suffix > prefix {
        format!("lines {}", window(prefix + 1, current.len() - suffix))
    } else {
        format!(
            "base lines {} removed",
            window(prefix + 1, base.len() - suffix)
        )
    }
}

/// Map the launch-time scope onto the file as it is now. Only the scoped lines
/// may differ from the pinned base, so everything around them locates the new
/// range exactly; any other difference makes the range stale.
pub fn reanchor_worker_scope(
    original: &TaskWorkerScope,
    base: &[u8],
    current: Option<&[u8]>,
) -> Result<TaskWorkerScope, StaleWorkerScope> {
    let path = worker_scope_path(original);
    let launched = describe_worker_scope(original);
    let stale = |detail: String| {
        StaleWorkerScope(format!(
            "stale range: {detail}; relaunch with a new worker_scope"
        ))
    };
    let Some(current) = current else {
        return Err(stale(format!(
            "`{path}` ({launched} at launch) no longer exists in the worktree"
        )));
    };
    let base_lines = base
        .split_inclusive(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    let current_lines = current
        .split_inclusive(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    let (before, after) = match original {
        TaskWorkerScope::PrelocalizedEdit {
            start_line,
            end_line,
            ..
        } => (
            (*start_line as usize).saturating_sub(1),
            base_lines.len().saturating_sub(*end_line as usize),
        ),
        TaskWorkerScope::PrelocalizedInsertAfter { anchor_line, .. } => (*anchor_line as usize, 0),
    };
    let untouched = before + after <= base_lines.len()
        && before + after <= current_lines.len()
        && current_lines[..before] == base_lines[..before]
        && current_lines[current_lines.len() - after..] == base_lines[base_lines.len() - after..];
    if !untouched {
        return Err(stale(format!(
            "`{path}` changed outside the prelocalized {launched} since launch (observed change: {})",
            observed_line_change(&base_lines, &current_lines)
        )));
    }
    let TaskWorkerScope::PrelocalizedEdit { start_line, .. } = original else {
        return Ok(original.clone());
    };
    let end_line = current_lines.len() - after;
    if end_line < *start_line as usize {
        return Err(stale(format!(
            "the delivered change removed every line of `{path}` {launched} (observed range: empty)"
        )));
    }
    let moved = TaskWorkerScope::PrelocalizedEdit {
        path: path.to_string(),
        start_line: *start_line,
        end_line: end_line as u32,
    };
    moved.validate().map_err(|error| {
        stale(format!(
            "`{path}` {launched} now spans {} ({error})",
            describe_worker_scope(&moved)
        ))
    })?;
    Ok(moved)
}

/// A prelocalized scope that moved since launch, for the worker's hand-off.
pub struct ReanchoredScope {
    pub original: TaskWorkerScope,
    pub current: TaskWorkerScope,
}

fn reanchored_scope_note(scope: &ReanchoredScope, lang: RoomLang) -> String {
    let target = |scope: &TaskWorkerScope| match (scope, lang) {
        (
            TaskWorkerScope::PrelocalizedEdit {
                start_line,
                end_line,
                ..
            },
            RoomLang::Fr,
        ) => format!("la plage inclusive `{start_line}..={end_line}`"),
        (
            TaskWorkerScope::PrelocalizedEdit {
                start_line,
                end_line,
                ..
            },
            RoomLang::En,
        ) => format!("the inclusive range `{start_line}..={end_line}`"),
        (TaskWorkerScope::PrelocalizedInsertAfter { anchor_line, .. }, RoomLang::Fr) => {
            format!("l'insertion après la ligne `{anchor_line}`")
        }
        (TaskWorkerScope::PrelocalizedInsertAfter { anchor_line, .. }, RoomLang::En) => {
            format!("the insertion after line `{anchor_line}`")
        }
    };
    let path = worker_scope_path(&scope.current);
    let (current, original) = (target(&scope.current), target(&scope.original));
    match lang {
        RoomLang::Fr => format!(
            "\n\n## Cible prélocalisée réancrée\n\
             `{path}` a changé depuis le lancement : la cible est maintenant {current} \
             (au lancement : {original}). Les outils imposent cette nouvelle cible ; ne te \
             fie pas aux numéros de ligne du brief initial."
        ),
        RoomLang::En => format!(
            "\n\n## Prelocalized target re-anchored\n\
             `{path}` changed since launch: the target is now {current} \
             (at launch: {original}). The tools enforce this new target; do not rely on the \
             line numbers of the initial brief."
        ),
    }
}

/// Re-anchor a prelocalized execution before another worker round reads its
/// frozen range: a delivery that changed the line count moved the content.
/// Refusals are `StaleWorkerScope` errors; the re-anchored scope is persisted.
pub async fn reanchor_execution_scope(
    db: &Database,
    exec: &TaskExecution,
) -> Result<Option<ReanchoredScope>> {
    let (Some(persisted), Some(base_sha)) = (exec.worker_scope.clone(), exec.base_sha.clone())
    else {
        return Ok(None);
    };
    let execution_id = exec.id.clone();
    let (original, workspace) = db
        .with_conn(move |conn| {
            Ok((
                crate::db::orchestration::launch_worker_scope(conn, &execution_id)?,
                crate::db::discussion_workspaces::get_managed_for_execution(conn, &execution_id)?,
            ))
        })
        .await?;
    let Some(worktree) = workspace.and_then(|workspace| workspace.canonical_path) else {
        return Ok(None);
    };
    let original = original.unwrap_or_else(|| persisted.clone());
    let worktree = std::path::PathBuf::from(worktree);
    let path = worker_scope_path(&original);
    let base =
        worktree::file_at_revision(&worktree, &base_sha, path).map_err(anyhow::Error::msg)?;
    let current = match std::fs::read(worktree.join(path)) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => bail!("cannot read `{path}` in the worktree: {error}"),
    };
    let reanchored = reanchor_worker_scope(&original, &base, current.as_deref())?;
    if reanchored != persisted {
        let changes = serde_json::json!({
            "original": original,
            "from": persisted,
            "to": reanchored,
            "base_sha": base_sha,
        });
        let (id, scope) = (exec.id.clone(), reanchored.clone());
        db.with_conn(move |conn| {
            crate::db::orchestration::reanchor_execution_worker_scope(
                conn,
                &id,
                &scope,
                &backend_actor(),
                changes,
            )
        })
        .await?;
    }
    Ok((reanchored != original).then_some(ReanchoredScope {
        original,
        current: reanchored,
    }))
}

/// What one integration attempt did (KT-320 DoD-3/7/8).
#[derive(Debug)]
pub enum IntegrationOutcome {
    /// The parent branch advanced onto the validated candidate.
    Integrated { sha: String },
    /// The candidate could not be built or did not validate. Branch, worktree and
    /// sub-discussion are untouched. The worker was re-activated with the failure
    /// and the principal that approved was told (KT-862): by the time this is
    /// returned, both are durable, so a caller only has to log it.
    SentBack { reason: String },
    /// A precondition was not met and the target is untouched. A live execution
    /// is parked with the reason (approval hold, `Blocked` or `Interrupted`).
    Refused { reason: String },
    /// The execution is not `Approved` — a replay of an integration already run
    /// lands here rather than doing it twice.
    NotIntegrable { status: TaskExecutionStatus },
}

/// KT-1748 — the project's own managed worktree roots, on top of the global
/// Settings > Workers list and the built-in ones.
fn project_managed_roots(project: &crate::models::Project) -> Vec<std::path::PathBuf> {
    project
        .workspace
        .as_ref()
        .map(|workspace| {
            crate::models::effective_managed_worktree_roots(&workspace.managed_worktree_roots)
        })
        .unwrap_or_default()
}

/// KT-1748 — `Some(refusal text)` when the target is held by a Kronn-managed
/// worktree that a run or a live agent is using right now: integrating would move
/// its files under that run. `None`: the target is held by nothing, or by an idle
/// worktree. A database or Git error reads as busy, never as idle.
async fn busy_target_refusal(
    db: &Database,
    repo: &std::path::Path,
    target_branch: &str,
    exec_id: &str,
    waiting_runs: &[String],
) -> Option<String> {
    let repo = repo.to_path_buf();
    let target = target_branch.to_string();
    let own = exec_id.to_string();
    let waiting = waiting_runs.to_vec();
    let verdict = db
        .with_conn(move |conn| {
            Ok(worktree::integration_target_idle(&repo, &target, &|path| {
                crate::db::discussion_workspaces::active_holder_of_checkout(
                    conn,
                    path,
                    Some(&own),
                    &waiting,
                )
            }))
        })
        .await;
    let unreadable = |error: String| {
        format!(
            "integration_target_busy_worktree: cannot tell whether the target's worktree is in use ({error})"
        )
    };
    match verdict {
        Ok(Ok(Ok(()))) => None,
        Ok(Ok(Err(refusal))) => Some(refusal.message()),
        Ok(Err(error)) => Some(unreadable(error)),
        Err(error) => Some(unreadable(error.to_string())),
    }
}

/// Park a refused integration with its reason and fix, and tell the principal
/// room, so neither an approval nor a validated candidate is silently stranded.
/// A row parked `Interrupted` gets its recovery decision from the real Git state;
/// a row in no parkable state reports the refusal as an error.
pub async fn hold_integration(
    db: &Database,
    exec_id: &str,
    code: BlockedReasonCode,
    reason: String,
    fix: &str,
) -> Result<IntegrationOutcome, ProvisionError> {
    let id = exec_id.to_string();
    let durable_reason = reason.clone();
    let fix = fix.to_string();
    let parked = db
        .with_conn(move |conn| {
            crate::db::orchestration::hold_refused_integration(
                conn,
                &id,
                code,
                &durable_reason,
                &fix,
                &backend_actor(),
            )
        })
        .await
        .map_err(|error| ProvisionError::Internal(error.to_string()))?;
    match parked {
        None => Err(ProvisionError::CheckpointRefused(reason)),
        Some(TaskExecutionStatus::Interrupted) => {
            if let Err(error) = refresh_integration_recovery(db, exec_id).await {
                tracing::warn!(execution_id = %exec_id, %error, "refused integration has no recovery decision");
            }
            Ok(IntegrationOutcome::Refused { reason })
        }
        Some(_) => Ok(IntegrationOutcome::Refused { reason }),
    }
}

/// Name the fix for a target branch that is not checked out in exactly one worktree.
fn target_checkout_fix(repo_path: &std::path::Path, target: &str) -> (BlockedReasonCode, String) {
    match worktree::target_branch_checkouts(repo_path, target) {
        Ok((branch, checkouts)) if checkouts.is_empty() => (
            BlockedReasonCode::IntegrationTargetNotCheckedOut,
            format!("Crée un worktree sur `{branch}` (`git worktree add <chemin> {branch}`)."),
        ),
        Ok((branch, checkouts)) if checkouts.len() > 1 => (
            BlockedReasonCode::IntegrationTargetNotCheckedOut,
            format!(
                "Garde `{branch}` extraite dans un seul worktree ({} actuellement).",
                checkouts.len()
            ),
        ),
        _ => (
            BlockedReasonCode::IntegrationRefused,
            format!("Fais de `{target}` une branche locale extraite dans un worktree propre."),
        ),
    }
}

/// Name the fix for a fast-forward the target checkout refused.
fn apply_refusal_fix(repo_path: &std::path::Path, target: &str) -> (BlockedReasonCode, String) {
    match worktree::integration_target(repo_path, target) {
        Err(_) => target_checkout_fix(repo_path, target),
        Ok(worktree::IntegrationTarget::NotCheckedOut { branch }) => (
            BlockedReasonCode::IntegrationRefused,
            format!(
                "Vérifie que `{branch}` n'a pas avancé ni été extraite dans un worktree pendant l'intégration, puis relance-la."
            ),
        ),
        Ok(worktree::IntegrationTarget::CheckedOut(checkout)) => match worktree::worktree_dirty_files(&checkout) {
            Ok(dirty) if !dirty.is_empty() => (
                BlockedReasonCode::IntegrationRefused,
                format!(
                    "Committe ou retire les fichiers non commités de `{}`.",
                    checkout.display()
                ),
            ),
            _ => (
                BlockedReasonCode::IntegrationRefused,
                format!("Vérifie l'état Git de `{}`.", checkout.display()),
            ),
        },
    }
}

/// How many times one integration attempt re-anchors on a target that advanced
/// during its validations before it parks for a human.
pub const MAX_INTEGRATION_REBUILDS: u32 = 3;

pub const CHILD_WORKTREE_FIX: &str = "Restaure le worktree de la tâche ou réaffecte l'exécution.";
const BACKUP_REF_FIX: &str = "Vérifie que le dépôt accepte l'écriture de `refs/kronn-backup/` \
     (verrou `.lock` résiduel, disque plein).";

pub fn backup_slug(execution: &TaskExecution) -> String {
    format!("{}-{}", execution.task_id, exec_short(&execution.id))
}

/// A built candidate, the tip it was built on and the verified ref back to it.
struct AnchoredCandidate {
    target_sha: String,
    merge_sha: String,
    backup_ref: String,
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
    run_integration_waited_on(db, exec_id, &[]).await
}

/// `run_integration` for a caller that `waiting_runs` wait on (a DelegateSubtasks
/// step, its run and the parent runs sharing its worktree): they write nothing
/// meanwhile, so they do not hold the target (KT-1795). Every other path,
/// a human approval or a resume included, keeps the strict check.
pub async fn run_integration_waited_on(
    db: &Database,
    exec_id: &str,
    waiting_runs: &[String],
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
            // The same gap can precede the report: publishing is idempotent.
            publish_accepted_delivery_or_warn(db, &execution).await;
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
    let generic = BlockedReasonCode::IntegrationRefused;
    // KT-1262 — the DoD is checked BEFORE anything touches the target: an item
    // added after the approve, which neither the approve nor the delivery
    // verified, holds the approval here. The Integrated checkpoint refuses the
    // same shape, but only after the target moved.
    let unverified = {
        let id = exec_id.to_string();
        db.with_conn(move |conn| crate::db::orchestration::unverified_worker_dod(conn, &id))
            .await
            .map_err(|e| internal(e.to_string()))?
    };
    if !unverified.is_empty() {
        let listed = unverified
            .iter()
            .map(|(dod_id, sentence)| format!("`{dod_id}` ({sentence})"))
            .collect::<Vec<_>>()
            .join(", ");
        return hold_integration(
            db,
            exec_id,
            generic,
            format!("the DoD has item(s) the approve did not verify: {listed}"),
            "Relance une revue : approuve avec une entrée `dod_verifications` pour chaque \
             item de DoD (ou déclare-le `post_integration`).",
        )
        .await;
    }
    let Some(target_branch) = run.target_branch.clone() else {
        return hold_integration(
            db,
            exec_id,
            generic,
            "no pinned target branch".into(),
            "Épingle une branche cible sur le run d'orchestration.",
        )
        .await;
    };
    let Some(project_id) = run.project_id.clone() else {
        return hold_integration(
            db,
            exec_id,
            generic,
            "run has no project".into(),
            "Rattache le run d'orchestration à un projet.",
        )
        .await;
    };
    let Some(child_path) = workspace.and_then(|w| w.canonical_path) else {
        return hold_integration(
            db,
            exec_id,
            generic,
            "no managed worktree".into(),
            CHILD_WORKTREE_FIX,
        )
        .await;
    };

    let project_path = {
        let pid = project_id.clone();
        db.with_conn(move |conn| Ok(crate::db::projects::get_project(conn, &pid)?.map(|p| p.path)))
            .await
            .map_err(|e| internal(e.to_string()))?
    };
    let Some(project_path) = project_path else {
        return hold_integration(
            db,
            exec_id,
            generic,
            "project vanished".into(),
            "Restaure le projet du run d'orchestration.",
        )
        .await;
    };
    let repo_path = scanner::resolve_host_path(&project_path);
    let child = std::path::Path::new(&child_path);
    let target_shape = match worktree::integration_target(&repo_path, &target_branch) {
        Ok(shape) => shape,
        Err(reason) => {
            let (code, fix) = target_checkout_fix(&repo_path, &target_branch);
            return hold_integration(db, exec_id, code, reason, &fix).await;
        }
    };

    // ── Preflight: never apply over uncommitted work ──
    // A target checked out nowhere has no working tree to protect.
    if let worktree::IntegrationTarget::CheckedOut(target_checkout) = &target_shape {
        match worktree::worktree_dirty_files(target_checkout) {
            Ok(dirty) if !dirty.is_empty() => {
                return hold_integration(
                    db,
                    exec_id,
                    generic,
                    format!("target has {} uncommitted file(s)", dirty.len()),
                    &format!(
                        "Committe ou retire les fichiers non commités de `{}`.",
                        target_checkout.display()
                    ),
                )
                .await;
            }
            Err(e) => {
                return hold_integration(
                    db,
                    exec_id,
                    generic,
                    e,
                    &format!("Vérifie l'état Git de `{}`.", target_checkout.display()),
                )
                .await;
            }
            Ok(_) => {}
        }
    }
    if let Err(error) = worktree::resolve_commit(child, "HEAD") {
        return hold_integration(
            db,
            exec_id,
            generic,
            format!("managed worktree is unusable: {error}"),
            CHILD_WORKTREE_FIX,
        )
        .await;
    }

    // ── Anchor: pin the tip the candidate is built on ──
    let target_sha = match worktree::resolve_commit(&repo_path, &target_branch) {
        Ok(sha) => sha,
        Err(e) => {
            return hold_integration(
                db,
                exec_id,
                generic,
                e,
                &format!("Vérifie que la branche `{target_branch}` pointe sur un commit."),
            )
            .await;
        }
    };
    // The way back is written before the anchor, where a refusal can still hold
    // the approval; the Armed checkpoint records it later.
    let backup_ref =
        match worktree::write_backup_ref(&repo_path, &backup_slug(&execution), &target_sha) {
            Ok(backup) => backup,
            Err(error) => {
                return hold_integration(db, exec_id, generic, error, BACKUP_REF_FIX).await
            }
        };
    checkpoint(db, exec_id, CheckpointStep::Anchored(target_sha.clone())).await?;

    // ── Phase 1: build the candidate in the CHILD worktree ──
    let hooks = worker_commit_hooks_of(db, Some(&project_id)).await;
    let build = match worktree::build_candidate_with_hooks(child, &target_sha, hooks) {
        Ok(build) => build,
        Err(error) => {
            return hold_integration(db, exec_id, generic, error, CHILD_WORKTREE_FIX).await
        }
    };
    let merge_sha = match build.outcome {
        worktree::CandidateOutcome::Built { sha } => sha,
        worktree::CandidateOutcome::HookRefused { hook, output } => {
            return send_back_hook_refusal(db, exec_id, hook, output).await;
        }
        worktree::CandidateOutcome::Conflict { files } => {
            send_back(
                db,
                exec_id,
                &ReworkReport::conflict(conflict_reason(
                    "merge conflict",
                    &files,
                    build.detail.as_deref(),
                )),
            )
            .await?;
            return Ok(IntegrationOutcome::SentBack {
                reason: format!("conflict in {}", files.join(", ")),
            });
        }
    };
    note_citations_settled(db, exec_id, &merge_sha, build.settled.as_ref()).await;
    checkpoint(db, exec_id, CheckpointStep::Built(merge_sha.clone())).await?;

    // ── Validations: a failure sends the work back, it never blocks the parent ──
    checkpoint(db, exec_id, CheckpointStep::Validating).await?;
    validate_and_apply(
        db,
        &execution,
        &run,
        &repo_path,
        &child_path,
        waiting_runs,
        AnchoredCandidate {
            target_sha,
            merge_sha,
            backup_ref,
        },
    )
    .await
}

/// Which git hooks Kronn runs for its own merge commits for a worker of this
/// project (KT-1745); none unless a human set otherwise.
async fn worker_commit_hooks_of(
    db: &Database,
    project_id: Option<&str>,
) -> crate::models::WorkerCommitHooks {
    let project_id = project_id.map(str::to_string);
    db.with_read_conn(move |conn| {
        Ok(crate::db::projects::worker_commit_hooks(
            conn,
            project_id.as_deref(),
        ))
    })
    .await
    .unwrap_or_default()
}

/// A hook refused the candidate merge commit (KT-1745): the worker reads which
/// hook and its first lines, not an empty conflict.
async fn send_back_hook_refusal(
    db: &Database,
    exec_id: &str,
    hook: String,
    output: String,
) -> Result<IntegrationOutcome, ProvisionError> {
    let reason = crate::core::commit_hooks::HookRefusal { hook, output }.to_string();
    send_back(db, exec_id, &ReworkReport::conflict(reason.clone())).await?;
    Ok(IntegrationOutcome::SentBack { reason })
}

/// What the worker reads as the cause of a merge conflict: the files, and for a
/// bounce on doc citations the exact citations behind it (KT-1669).
fn conflict_reason(prefix: &str, files: &[String], detail: Option<&str>) -> String {
    let base = format!("{prefix} in {}", files.join(", "));
    match detail {
        Some(detail) => format!("{base}: {detail}"),
        None => base,
    }
}

/// Journal that the candidate merge settled a citation-only doc conflict
/// itself, for the delivery card. Best effort: the merge exists either way and
/// a lost note only leaves the card less informative.
async fn note_citations_settled(
    db: &Database,
    exec_id: &str,
    merge_sha: &str,
    settled: Option<&crate::core::docs_citations::merge::Settlement>,
) {
    let Some(settled) = settled.cloned() else {
        return;
    };
    let (id, sha) = (exec_id.to_string(), merge_sha.to_string());
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::orchestration::record_citations_settled(
                conn,
                &id,
                &sha,
                &settled,
                &backend_actor(),
            )
        })
        .await
    {
        tracing::warn!(execution = %exec_id, %error, "citation settlement not journaled");
    }
}

/// Validate, arm and fast-forward a built candidate; the row is `Validating` on
/// entry. A target that advanced meanwhile is re-anchored and rebuilt, at most
/// `MAX_INTEGRATION_REBUILDS` times, and every other refusal parks the row
/// instead of leaving it `Applying`.
async fn validate_and_apply(
    db: &Database,
    execution: &TaskExecution,
    run: &crate::models::OrchestrationRun,
    repo: &std::path::Path,
    child_path: &str,
    waiting_runs: &[String],
    mut candidate: AnchoredCandidate,
) -> Result<IntegrationOutcome, ProvisionError> {
    let exec_id = execution.id.as_str();
    let child = std::path::Path::new(child_path);
    let target_branch = run
        .target_branch
        .as_deref()
        .ok_or_else(|| ProvisionError::Internal("run has no pinned target branch".into()))?;
    let generic = BlockedReasonCode::IntegrationRefused;
    let reach = ValidationReach::of_run(db, run.project_id.as_deref(), &run.discussion_id).await;
    // The source may have been regenerated since the worker started (KT-1438).
    if let Some(project_id) = run.project_id.as_deref() {
        seed_lane_assets(db, exec_id, project_id, repo, child).await;
    }
    let mut rebuilds = 0;
    loop {
        if let Some(failure) = run_pending_validations(
            db,
            exec_id,
            &run.validations,
            child,
            &candidate.merge_sha,
            reach.launch(),
        )
        .await?
        {
            // The command never reached the worker's code: the worker cannot fix
            // it, so the delivery stays approved and a human gets the fix.
            if let Some(cause) = reach.environment_cause(&failure) {
                let reason = format!(
                    "validation environment: `{}` could not run ({})",
                    failure.command,
                    first_line(&failure.output)
                );
                let outcome = hold_integration(db, exec_id, generic, reason, &cause.fix).await;
                if cause.docker_elevation && outcome.is_ok() {
                    post_docker_elevation(db, exec_id).await;
                }
                return outcome;
            }
            let report = ReworkReport::validation(failure);
            send_back(db, exec_id, &report).await?;
            return Ok(IntegrationOutcome::SentBack {
                reason: report.reason,
            });
        }
        checkpoint(
            db,
            exec_id,
            CheckpointStep::Armed(candidate.backup_ref.clone()),
        )
        .await?;

        // ── Phase 2: advance the parent, fast-forward only ──
        let tip = match worktree::resolve_commit(repo, target_branch) {
            Ok(tip) => tip,
            Err(error) => {
                let fix = format!("Vérifie que la branche `{target_branch}` pointe sur un commit.");
                return hold_integration(db, exec_id, generic, error, &fix).await;
            }
        };
        let tip = if tip != candidate.target_sha {
            tip
        } else {
            if let Some(reason) =
                busy_target_refusal(db, repo, target_branch, exec_id, waiting_runs).await
            {
                let fix = format!(
                    "Attends que le run qui utilise le worktree de `{target_branch}` se termine, puis relance l'intégration."
                );
                return hold_integration(db, exec_id, generic, reason, &fix).await;
            }
            match worktree::fast_forward_target_to(
                repo,
                target_branch,
                &candidate.target_sha,
                &candidate.merge_sha,
            ) {
                Ok(integrated) => {
                    return land_integration(db, execution, run, repo, child_path, integrated).await
                }
                // A concurrent integration can still land between the check and the merge.
                Err(error) => match worktree::resolve_commit(repo, target_branch) {
                    Ok(moved) if moved != candidate.target_sha => moved,
                    _ => {
                        let (code, fix) = apply_refusal_fix(repo, target_branch);
                        return hold_integration(db, exec_id, code, error, &fix).await;
                    }
                },
            }
        };

        // The target advanced under the candidate: rebuild on the tip it now has.
        if rebuilds == MAX_INTEGRATION_REBUILDS {
            let reason = format!(
                "target `{target_branch}` advanced during validation {} times in a row (now at {tip})",
                rebuilds + 1
            );
            let fix = format!(
                "Attends que les intégrations concurrentes vers `{target_branch}` se terminent."
            );
            return hold_integration(
                db,
                exec_id,
                BlockedReasonCode::IntegrationTargetDrifted,
                reason,
                &fix,
            )
            .await;
        }
        rebuilds += 1;
        // Checked while still Applying, where a refusal parks in Blocked.
        if let Err(error) = worktree::resolve_commit(child, "HEAD") {
            return hold_integration(
                db,
                exec_id,
                generic,
                format!("managed worktree is unusable: {error}"),
                CHILD_WORKTREE_FIX,
            )
            .await;
        }
        let backup_ref = match worktree::write_backup_ref(repo, &backup_slug(execution), &tip) {
            Ok(backup) => backup,
            Err(error) => {
                return hold_integration(db, exec_id, generic, error, BACKUP_REF_FIX).await
            }
        };
        reanchor_candidate(db, exec_id, &tip, "target_advanced").await?;
        let hooks = worker_commit_hooks_of(db, run.project_id.as_deref()).await;
        let build = match worktree::build_candidate_with_hooks(child, &tip, hooks) {
            Ok(build) => build,
            Err(error) => {
                return hold_integration(db, exec_id, generic, error, CHILD_WORKTREE_FIX).await
            }
        };
        let merge_sha = match build.outcome {
            worktree::CandidateOutcome::Built { sha } => sha,
            worktree::CandidateOutcome::HookRefused { hook, output } => {
                return send_back_hook_refusal(db, exec_id, hook, output).await;
            }
            worktree::CandidateOutcome::Conflict { files } => {
                let reason = conflict_reason(
                    "merge conflict after the target advanced",
                    &files,
                    build.detail.as_deref(),
                );
                send_back(db, exec_id, &ReworkReport::conflict(reason.clone())).await?;
                return Ok(IntegrationOutcome::SentBack { reason });
            }
        };
        note_citations_settled(db, exec_id, &merge_sha, build.settled.as_ref()).await;
        checkpoint(db, exec_id, CheckpointStep::Built(merge_sha.clone())).await?;
        checkpoint(db, exec_id, CheckpointStep::Validating).await?;
        candidate = AnchoredCandidate {
            target_sha: tip,
            merge_sha,
            backup_ref,
        };
    }
}

/// Drop a stale candidate and pin the integration on `target_sha` (→ `Integrating`).
async fn reanchor_candidate(
    db: &Database,
    exec_id: &str,
    target_sha: &str,
    cause: &'static str,
) -> Result<(), ProvisionError> {
    let id = exec_id.to_string();
    let anchor = target_sha.to_string();
    let moved = db
        .with_conn(move |conn| {
            crate::db::orchestration::resume_rebuild_candidate(
                conn,
                &id,
                &anchor,
                cause,
                &backend_actor(),
            )
        })
        .await
        .map_err(|error| ProvisionError::Internal(error.to_string()))?;
    if moved {
        Ok(())
    } else {
        Err(ProvisionError::CheckpointRefused(
            "the integration was re-anchored by another caller".into(),
        ))
    }
}

/// Record the landed integration, then clean up and let the campaign continue.
/// Git has already moved, so a failed checkpoint parks the row for the
/// idempotent close instead of leaving it `Applying`.
async fn land_integration(
    db: &Database,
    execution: &TaskExecution,
    run: &crate::models::OrchestrationRun,
    repo: &std::path::Path,
    child_path: &str,
    integrated: String,
) -> Result<IntegrationOutcome, ProvisionError> {
    // KT-1519 — only the default branch completes the task; a campaign or
    // feature branch leaves it integrated, awaiting the merge.
    let feature_branch = run
        .target_branch
        .clone()
        .filter(|branch| !crate::core::project_profile::is_default_branch(repo, branch));
    let into_default = feature_branch.is_none();
    let step = match feature_branch {
        Some(branch) => CheckpointStep::IntegratedAwaitingMerge(integrated.clone(), branch),
        None => CheckpointStep::Integrated(integrated.clone()),
    };
    if let Err(error) = checkpoint(db, &execution.id, step).await {
        let reason = format!("integrated Git state could not be checkpointed: {error:?}");
        if let Err(interrupt_error) = interrupt_recovered_apply(db, &execution.id, &reason).await {
            tracing::warn!(
                execution_id = %execution.id,
                error = ?interrupt_error,
                "could not park an apply after its integration checkpoint failed"
            );
        }
        return Err(error);
    }
    // The delivery is accepted once it has landed, not when it was approved: a
    // bounced or refused integration must never leave an "accepted" report
    // behind (KT-1316). Publication is total and idempotent.
    publish_accepted_delivery_or_warn(db, execution).await;
    cleanup_integrated_execution(db, execution, repo, child_path, &integrated).await?;
    if into_default {
        // Tasks integrated earlier into a feature branch may have reached the
        // default branch with this landing.
        if let Some(project_id) = integration_project_id(db, execution, run).await {
            close_merged_awaiting_tasks(db, &project_id, repo, false).await;
        }
    }
    advance_campaign_after_integration(db, run).await;
    Ok(IntegrationOutcome::Integrated { sha: integrated })
}

/// The project an integration landed in: the run's, else the one the task
/// belongs to.
async fn integration_project_id(
    db: &Database,
    execution: &TaskExecution,
    run: &crate::models::OrchestrationRun,
) -> Option<String> {
    if let Some(project_id) = run.project_id.clone() {
        return Some(project_id);
    }
    let task_id = execution.task_id.clone();
    db.with_conn(move |conn| {
        Ok(crate::db::planning::get_task(conn, &task_id)?
            .and_then(|task| task.summary.project_ids.into_iter().next()))
    })
    .await
    .ok()
    .flatten()
}

/// How the sweep asks GitHub about a task's issue or pull request.
pub struct GithubLookup {
    pub api_base: String,
    pub token: zeroize::Zeroizing<String>,
}

/// KT-1519 — close every task of `project_id` integrated into a feature branch
/// whose change the default branch now holds. Two signals, because a squash
/// merge (how this repository merges) leaves the integrated commit unreachable:
/// the commit is an ancestor of the default branch (local or fetched
/// `origin/<default>`), or GitHub reports the task's own issue (`github-N` tag)
/// closed or the pull request it links merged from the task's branch. Best effort
/// and idempotent: a task GitHub cannot be asked about stays awaiting.
///
/// `refresh_origin` first fetches the default branch from `origin`: after a merge
/// on the remote nothing else moves `refs/remotes/origin/<default>` in the host
/// checkout. The landing path passes `false` (its own ref has just moved).
pub async fn close_merged_awaiting_tasks(
    db: &Database,
    project_id: &str,
    repo: &std::path::Path,
    refresh_origin: bool,
) {
    let pid = project_id.to_string();
    let awaiting = match db
        .with_conn(move |conn| crate::db::planning::list_awaiting_merge(conn, &pid))
        .await
    {
        Ok(awaiting) => awaiting,
        Err(error) => {
            tracing::warn!(%error, "could not list tasks awaiting a merge");
            return;
        }
    };
    if awaiting.is_empty() {
        return;
    }
    if refresh_origin {
        fetch_default_branch(repo).await;
    }
    let github = if awaiting
        .iter()
        .any(|task| !task.tags.is_empty() || !task.link_urls.is_empty())
    {
        crate::core::github_connection::machine_token()
            .await
            .map(|(token, _)| GithubLookup {
                api_base: crate::core::github_connection::api_base(),
                token,
            })
    } else {
        None
    };
    close_merged_awaiting_tasks_with(db, repo, awaiting, github.as_ref()).await;
}

const DEFAULT_BRANCH_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Bring `refs/remotes/origin/<default>` up to date. A repository without
/// `origin`, or offline, keeps what it has: the sweep is best effort.
async fn fetch_default_branch(repo: &std::path::Path) {
    let probe_repo = repo.to_path_buf();
    let name = tokio::task::spawn_blocking(move || {
        crate::core::project_profile::default_branch_refs(&probe_repo)
            .into_iter()
            .next()
            .map(|full| {
                full.strip_prefix("refs/heads/")
                    .or_else(|| full.strip_prefix("refs/remotes/origin/"))
                    .unwrap_or(&full)
                    .to_string()
            })
    })
    .await
    .ok()
    .flatten();
    let Some(name) = name else {
        return;
    };
    if let Err(reason) = crate::workflows::workspace::fetch_remote_branch(
        repo,
        "origin",
        &name,
        DEFAULT_BRANCH_FETCH_TIMEOUT,
    )
    .await
    {
        tracing::debug!(%reason, "default branch not refreshed before the merge sweep");
    }
}

pub async fn close_merged_awaiting_tasks_with(
    db: &Database,
    repo: &std::path::Path,
    awaiting: Vec<crate::db::planning::AwaitingMergeTask>,
    github: Option<&GithubLookup>,
) {
    // Git is synchronous: keep it off the async executor.
    let probe_repo = repo.to_path_buf();
    let probe = awaiting.clone();
    let probed = tokio::task::spawn_blocking(move || {
        let refs = crate::core::project_profile::default_branch_refs(&probe_repo);
        let reached: Vec<bool> = probe
            .iter()
            .map(|task| {
                refs.iter().any(|default_ref| {
                    worktree::is_ancestor(&probe_repo, &task.sha, default_ref).unwrap_or(false)
                })
            })
            .collect();
        let slug = crate::core::github_connection::git_remote_urls(&probe_repo)
            .iter()
            .find_map(|url| crate::core::github_connection::github_repo_slug(url));
        (reached, slug)
    })
    .await;
    let Ok((reached, slug)) = probed else {
        tracing::warn!("merge probe of the awaiting tasks did not complete");
        return;
    };
    let mut merged: Vec<String> = Vec::new();
    for (task, reached) in awaiting.iter().zip(reached) {
        if reached {
            merged.push(task.task_id.clone());
            continue;
        }
        let (Some(github), Some(slug)) = (github, slug.as_deref()) else {
            continue;
        };
        if github_says_landed(github, slug, task).await {
            merged.push(task.task_id.clone());
        }
    }
    if merged.is_empty() {
        return;
    }
    let result = db
        .with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            for task_id in &merged {
                crate::db::planning::close_awaiting_merge_within_tx(
                    &tx,
                    task_id,
                    &backend_actor(),
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await;
    if let Err(error) = result {
        tracing::warn!(%error, "could not close tasks merged into the default branch");
    }
}

/// Whether GitHub reports `task` shipped: the issue its `github-N` tag claims is
/// closed (or that pull request merged), or a pull request it links was merged
/// from the very branch the task was integrated into. Any other number the task
/// mentions is a reference and proves nothing.
async fn github_says_landed(
    github: &GithubLookup,
    slug: &str,
    task: &crate::db::planning::AwaitingMergeTask,
) -> bool {
    for number in crate::core::github_connection::tagged_work_item_numbers(&task.tags) {
        let landed = crate::core::github_connection::work_item_landed(
            &github.api_base,
            &github.token,
            slug,
            number,
        )
        .await;
        if landed == Some(true) {
            return true;
        }
    }
    for number in crate::core::github_connection::linked_pull_numbers(slug, &task.link_urls) {
        let landed = crate::core::github_connection::pull_request_landed(
            &github.api_base,
            &github.token,
            slug,
            number,
            &task.branch,
        )
        .await;
        if landed == Some(true) {
            return true;
        }
    }
    false
}

const MERGE_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// KT-1519 — runtime counterpart of the boot sweep: a feature branch merged on
/// GitHub (a squash merge never reaches Kronn's own integration path) closes its
/// awaiting tasks within minutes, not at the next restart.
pub fn start_merge_sweep(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(MERGE_SWEEP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The first tick is immediate: the boot reconciliation only looks at
        // local refs, this sweep also refreshes `origin` (in the background).
        loop {
            interval.tick().await;
            let projects = match state.db.with_conn(crate::db::projects::list_projects).await {
                Ok(projects) => projects,
                Err(error) => {
                    tracing::warn!(%error, "merge sweep could not list projects");
                    continue;
                }
            };
            for project in projects {
                let repo = scanner::resolve_host_path(&project.path);
                close_merged_awaiting_tasks(&state.db, &project.id, &repo, true).await;
            }
        }
    });
}

/// Consume one durable boot-recovery decision for the integration saga. Every
/// path re-checks the real target ref immediately before mutation; a recorded
/// checkpoint is evidence, never permission to replay Git blindly.
pub async fn resume_recovered_integration(
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
    let generic = BlockedReasonCode::IntegrationRefused;

    // Refusals are checked before the row leaves its hold: an interrupted row
    // keeps its pending decision, a claimed Applying one parks in Blocked.
    let backup_ref = match action {
        ExecutionRecoveryAction::RebuildCandidate => {
            let target_sha = worktree::resolve_commit(&repo, target_branch)
                .map_err(|error| internal(format!("cannot resolve recovery target: {error}")))?;
            if let Err(error) = worktree::resolve_commit(child, "HEAD") {
                return hold_integration(
                    db,
                    exec_id,
                    generic,
                    format!("managed worktree is unusable: {error}"),
                    CHILD_WORKTREE_FIX,
                )
                .await;
            }
            let backup_ref =
                match worktree::write_backup_ref(&repo, &backup_slug(&execution), &target_sha) {
                    Ok(backup) => backup,
                    Err(error) => {
                        return hold_integration(db, exec_id, generic, error, BACKUP_REF_FIX).await
                    }
                };
            reanchor_candidate(db, exec_id, &target_sha, "rebuild_candidate").await?;
            let hooks = worker_commit_hooks_of(db, run.project_id.as_deref()).await;
            let build = match worktree::build_candidate_with_hooks(child, &target_sha, hooks) {
                Ok(build) => build,
                Err(error) => {
                    return hold_integration(db, exec_id, generic, error, CHILD_WORKTREE_FIX).await
                }
            };
            let merge_sha = match build.outcome {
                worktree::CandidateOutcome::Built { sha } => sha,
                worktree::CandidateOutcome::HookRefused { hook, output } => {
                    return send_back_hook_refusal(db, exec_id, hook, output).await;
                }
                worktree::CandidateOutcome::Conflict { files } => {
                    let reason = conflict_reason(
                        "merge conflict after recovery",
                        &files,
                        build.detail.as_deref(),
                    );
                    send_back(db, exec_id, &ReworkReport::conflict(reason.clone())).await?;
                    return Ok(IntegrationOutcome::SentBack { reason });
                }
            };
            note_citations_settled(db, exec_id, &merge_sha, build.settled.as_ref()).await;
            checkpoint(db, exec_id, CheckpointStep::Built(merge_sha)).await?;
            checkpoint(db, exec_id, CheckpointStep::Validating).await?;
            backup_ref
        }
        ExecutionRecoveryAction::RunValidations => {
            let target_sha = execution
                .candidate_target_sha
                .as_deref()
                .ok_or_else(|| internal("candidate target SHA is missing".into()))?;
            let backup_ref =
                match worktree::write_backup_ref(&repo, &backup_slug(&execution), target_sha) {
                    Ok(backup) => backup,
                    Err(error) => {
                        return hold_integration(db, exec_id, generic, error, BACKUP_REF_FIX).await
                    }
                };
            let id = exec_id.to_string();
            let claimed = db
                .with_conn(move |conn| {
                    crate::db::orchestration::transition_execution(
                        conn,
                        &id,
                        TaskExecutionStatus::Validating,
                        &backend_actor(),
                        serde_json::json!({ "recovery": "run_validations" }),
                    )
                })
                .await
                .map_err(|error| internal(error.to_string()))?;
            require_recovered_apply_claim(claimed)?;
            backup_ref
        }
        ExecutionRecoveryAction::ApplyFastForward | ExecutionRecoveryAction::IdempotentClose => {
            return finish_recovered_apply(db, &execution, &run, &repo, child_path).await;
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
    };

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
        send_back(db, exec_id, &ReworkReport::conflict(reason.clone())).await?;
        return Ok(IntegrationOutcome::SentBack { reason });
    }
    let target_sha = recovered_execution
        .candidate_target_sha
        .clone()
        .ok_or_else(|| internal("candidate target SHA is missing".into()))?;
    validate_and_apply(
        db,
        &recovered_execution,
        &run,
        &repo,
        child_path,
        &[],
        AnchoredCandidate {
            target_sha,
            merge_sha,
            backup_ref,
        },
    )
    .await
}

/// Replay a validated, armed apply, or close one that already landed. The row
/// is `Interrupted` or `Blocked` on entry and claims `Applying` only after the
/// real target was re-checked.
pub async fn finish_recovered_apply(
    db: &Database,
    execution: &TaskExecution,
    run: &crate::models::OrchestrationRun,
    repo: &std::path::Path,
    child_path: &str,
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
    let (target_shape, real_tip) =
        match worktree::integration_target(repo, target_branch).and_then(|shape| {
            worktree::integration_target_tip(repo, target_branch).map(|tip| (shape, tip))
        }) {
            Ok(target) => target,
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
    // Git already landed before the old process died, possibly with more work on
    // top since. Do not replay it.
    let already_applied = real_tip == merge_sha
        || worktree::is_ancestor(repo, merge_sha, &real_tip)
            .map_err(|error| internal(format!("cannot compare target with candidate: {error}")))?;
    let integrated = if already_applied {
        claim_recovered_apply(db, &execution.id, "idempotent_close_recheck_complete").await?;
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
        let dirty = worktree::integration_target_dirty_files(&target_shape)
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
        if let Some(reason) = busy_target_refusal(db, repo, target_branch, &execution.id, &[]).await
        {
            // Parked again, like a dirty target: the next resume asks again.
            let id = execution.id.clone();
            let durable_reason = reason.clone();
            db.with_conn(move |conn| {
                crate::db::orchestration::block_execution(
                    conn,
                    &id,
                    &backend_actor(),
                    &durable_reason,
                    None,
                )?;
                Ok(())
            })
            .await
            .map_err(|error| internal(error.to_string()))?;
            return Ok(IntegrationOutcome::Refused { reason });
        }
        claim_recovered_apply(db, &execution.id, "guarded_apply_recheck_complete").await?;
        match worktree::fast_forward_target_to(repo, target_branch, target_sha, merge_sha) {
            Ok(sha) => sha,
            Err(error) => {
                let reason = format!("recovered fast-forward refused: {error}");
                interrupt_recovered_apply(db, &execution.id, &reason).await?;
                return Err(internal(reason));
            }
        }
    };
    land_integration(db, execution, run, repo, child_path, integrated).await
}

/// Win the durable Blocked/Interrupted -> Applying claim of a recovered apply.
async fn claim_recovered_apply(
    db: &Database,
    exec_id: &str,
    recovery: &'static str,
) -> Result<(), ProvisionError> {
    let id = exec_id.to_string();
    let claimed = db
        .with_conn(move |conn| {
            crate::db::orchestration::transition_execution(
                conn,
                &id,
                TaskExecutionStatus::Applying,
                &backend_actor(),
                serde_json::json!({ "recovery": recovery }),
            )
        })
        .await
        .map_err(|error| ProvisionError::Internal(error.to_string()))?;
    require_recovered_apply_claim(claimed)
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
    .map_err(|error| ProvisionError::Internal(error.to_string()))?;
    // The pending decision predates this interruption; re-read it from Git.
    if let Err(error) = refresh_integration_recovery(db, exec_id).await {
        tracing::warn!(execution_id = %exec_id, %error, "recovery decision not refreshed");
    }
    Ok(())
}

/// Continue a campaign only after a clean success and only while no human gate
/// exists. A launch failure never rewrites the already-integrated result: it
/// parks the campaign durably for inspection instead.
async fn advance_campaign_after_integration(db: &Database, run: &crate::models::OrchestrationRun) {
    use crate::db::orchestration::CampaignParkCause;
    use crate::models::{OrchestrationControlState, OrchestrationRunKind};

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
            let all_active_done = plan
                .active
                .iter()
                .all(|relation| relation.task.satisfies_dependents());
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
            CampaignParkCause::ChildDecision,
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
                CampaignParkCause::LaunchFailed,
                &format!("automatic next-task launch failed: {error:?}"),
            )
            .await;
        }
        return;
    }
    if attention.active_executions > 0 {
        return;
    }
    if !all_active_done {
        // Parked on a human like the two cases above, so it leaves the same
        // durable card (KT-650).
        park_campaign_for_human(
            db,
            &current.id,
            CampaignParkCause::PlanBlocked,
            "no launchable task remains; the plan is blocked or needs correction",
        )
        .await;
        return;
    }
    let id = current.id.clone();
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::orchestration::set_orchestration_control_state(
                conn,
                &id,
                OrchestrationControlState::Completed,
                Some("all active plan tasks are complete"),
                &backend_actor(),
            )?;
            Ok(())
        })
        .await
    {
        tracing::error!(run_id = %current.id, %error, "could not close campaign after integration");
    }
}

async fn park_campaign_for_human(
    db: &Database,
    run_id: &str,
    cause: crate::db::orchestration::CampaignParkCause,
    reason: &str,
) {
    let id = run_id.to_string();
    let reason = reason.to_string();
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::orchestration::park_campaign_awaiting_human(
                conn,
                &id,
                cause,
                &reason,
                None,
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
pub async fn cleanup_integrated_execution(
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
            // KT-1256 — the lane target lives outside the worktree and is named
            // after it, so the removal above never took it. Before the intent
            // is retired: the audit finds its execution by the checkout path.
            reclaim_lane_target(db, repo_path, child_path).await;
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

/// Remove the lane target (`<repo>/target/wt-<lane>`) of a task worktree that
/// was just removed clean (KT-1256).
///
/// Never fails the caller, for the reason [`reclaim_preserved_worktree_artifacts`]
/// gives; a refusal (a build still holding the lock, a symlink) is logged and
/// audited, and the orphan sweep picks the directory up later.
async fn reclaim_lane_target(db: &Database, repo_path: &std::path::Path, child_path: &str) {
    let Some((lane_target, outcome)) = remove_lane_target_outcome(repo_path, child_path) else {
        return;
    };
    let canonical = child_path.to_string();
    let target = lane_target.to_string_lossy().to_string();
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::orchestration::record_lane_target_reclaim(conn, &canonical, &target, outcome)
        })
        .await
    {
        tracing::warn!(worktree = %child_path, "lane target reclaim audit failed: {error}");
    }
}

/// The removal half of [`reclaim_lane_target`]: `None` when the worktree names
/// no lane, else the lane target and what removing it did.
fn remove_lane_target_outcome(repo_path: &std::path::Path, child_path: &str) -> LaneRemoval {
    let lane_target =
        match worktree::lane_target_for_worktree(repo_path, std::path::Path::new(child_path)) {
            Ok(path) => path,
            Err(reason) => {
                tracing::warn!(worktree = %child_path, "lane target reclaim skipped: {reason}");
                return None;
            }
        };
    let outcome = worktree::remove_lane_target(repo_path, &lane_target)
        .map(|report| (report.bytes_reclaimed, report.bytes_are_partial));
    if let Err(reason) = &outcome {
        tracing::warn!(lane_target = %lane_target.display(), "lane target kept: {reason}");
    }
    Some((lane_target, outcome))
}

/// What [`reclaim_clean_finished_checkout`] did with one finished execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanCheckoutReclaim {
    /// No managed checkout, the execution is not finished and unused, or another
    /// release already holds it.
    NotApplicable,
    /// The clean checkout (or its leftover admin entry) is gone; the branch is kept.
    Released,
    /// A finished execution's checkout Kronn must not remove (dirty, on another
    /// branch, unreadable). `name` is its directory name, never a path.
    Kept { name: String, reason: String },
}

/// A finished execution's checkout a release has claimed (KT-1646): everything
/// the removal and the recording need, read once under the connection so the
/// removal itself holds none.
pub struct ReleaseCandidate {
    /// Held until the release has recorded its outcome.
    _claim: crate::core::checkout_release::ReleaseClaim,
    pub exec_id: String,
    repo: std::path::PathBuf,
    /// The checkout on this host.
    checkout: String,
    /// The path as the database stores it: the lane audit and the event key on it.
    db_path: String,
    branch: String,
    head_sha: Option<String>,
    /// The checkout's directory name, never a path.
    name: String,
}

/// What removing a claimed checkout did to its lane build dir: `None` when the
/// worktree names no lane.
type LaneRemoval = Option<(std::path::PathBuf, Result<(u64, bool), String>)>;

/// Step 1, under the connection: pick the execution if it is finished and
/// unused, and claim its checkout so no other release (and no reassign, resume
/// or review-round extension) takes it until the outcome is recorded.
/// `Ok(None)` is an execution with nothing to release, or one already claimed.
pub fn claim_finished_checkout(
    conn: &rusqlite::Connection,
    exec_id: &str,
) -> Result<Option<ReleaseCandidate>> {
    if crate::db::orchestration::get_task_execution(conn, exec_id)?.is_none() {
        return Ok(None);
    }
    let Some(workspace) =
        crate::db::discussion_workspaces::get_managed_for_execution(conn, exec_id)?
    else {
        return Ok(None);
    };
    let Some(path) = workspace.canonical_path.clone() else {
        return Ok(None);
    };
    // A finished execution, or (KT-1676) an unfinished one whose task was
    // closed and in whose checkout nothing works any more.
    if !matches!(
        crate::db::orchestration::worktree_release_liveness(conn, &path)?,
        worktree::ExecutionLiveness::Terminal
    ) {
        return Ok(None);
    }
    let Some(project) = crate::db::projects::get_project(conn, &workspace.project_id)? else {
        return Ok(None);
    };
    let Some(claim) = crate::core::checkout_release::ReleaseClaim::take(exec_id) else {
        return Ok(None);
    };
    let checkout = scanner::resolve_host_path(&path);
    let name = checkout
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed worktree".to_string());
    Ok(Some(ReleaseCandidate {
        _claim: claim,
        exec_id: exec_id.to_string(),
        repo: scanner::resolve_host_path(&project.path),
        checkout: checkout.to_string_lossy().to_string(),
        db_path: path,
        branch: workspace.branch,
        head_sha: workspace.head_sha,
        name,
    }))
}

/// Step 2, with no connection held: the Git checks (branch, `git status`) and
/// the removal, then the lane build dir. `Err` is the reason the checkout was
/// kept, with nothing removed.
pub fn remove_finished_checkout(candidate: &ReleaseCandidate) -> Result<LaneRemoval, String> {
    worktree::remove_cancelled_task_worktree(
        &candidate.repo,
        &candidate.checkout,
        &candidate.branch,
    )?;
    Ok(remove_lane_target_outcome(
        &candidate.repo,
        &candidate.checkout,
    ))
}

/// Step 3, under the connection again: record what step 2 did. A removed
/// checkout retires its workspace row and journals the release; a kept one is
/// reported back, its reason journaled by the caller.
pub fn settle_finished_checkout(
    conn: &rusqlite::Connection,
    candidate: &ReleaseCandidate,
    removal: Result<LaneRemoval, String>,
) -> Result<CleanCheckoutReclaim> {
    let exec_id = candidate.exec_id.as_str();
    match removal {
        Ok(lane) => {
            if let Some((lane_target, outcome)) = lane {
                let target = lane_target.to_string_lossy().to_string();
                if let Err(error) = crate::db::orchestration::record_lane_target_reclaim(
                    conn,
                    &candidate.db_path,
                    &target,
                    outcome,
                ) {
                    tracing::warn!(
                        worktree = %candidate.db_path,
                        "lane target reclaim audit failed: {error}"
                    );
                }
            }
            let tx = conn.unchecked_transaction()?;
            crate::db::discussion_workspaces::retire_managed_for_execution(
                &tx,
                exec_id,
                candidate.head_sha.as_deref(),
            )?;
            crate::db::orchestration::record_execution_event(
                &tx,
                exec_id,
                "worktree_released",
                None,
                None,
                &backend_actor(),
                serde_json::json!({
                    "path": candidate.db_path,
                    "branch": candidate.branch,
                    "clean_only": true,
                }),
            )?;
            tx.commit()?;
            Ok(CleanCheckoutReclaim::Released)
        }
        // Recording the reason is the caller's: the sweep leaves a dirty checkout
        // to its idle-aware pass, which journals its own reason, and the launch
        // journals it through `record_release_kept`.
        Err(reason) => Ok(CleanCheckoutReclaim::Kept {
            name: candidate.name.clone(),
            reason,
        }),
    }
}

/// Remove the managed checkout of one finished execution when its tree is clean
/// (KT-1646). The branch is kept; a dirty tree, a foreign branch or any other
/// refusal keeps the checkout, is reported as [`CleanCheckoutReclaim::Kept`] and
/// is recorded once as a `worktree_release_kept` event. Unlike the idle-aware
/// [`release_terminal_worktrees`] this needs no idle window: a clean tree holds
/// nothing a removal can lose, and only a durably terminal execution with no
/// live session or lease is touched. "Clean" is `git status` clean: ignored
/// build output goes with the worktree.
///
/// The connection is held for the two short database steps only; the Git checks
/// and the removal run on a blocking thread in between, so a slow disk never
/// freezes the API (see [`claim_finished_checkout`]).
pub async fn reclaim_clean_finished_checkout(
    db: &Database,
    exec_id: &str,
) -> Result<CleanCheckoutReclaim> {
    reclaim_clean_finished_checkout_with(db, exec_id, remove_finished_checkout).await
}

/// [`reclaim_clean_finished_checkout`] with the removal step injected: the
/// tests observe the moment between the claim and the recording.
pub async fn reclaim_clean_finished_checkout_with<R>(
    db: &Database,
    exec_id: &str,
    remove: R,
) -> Result<CleanCheckoutReclaim>
where
    R: FnOnce(&ReleaseCandidate) -> Result<LaneRemoval, String> + Send + 'static,
{
    let id = exec_id.to_string();
    let Some(candidate) = db
        .with_conn(move |conn| claim_finished_checkout(conn, &id))
        .await?
    else {
        return Ok(CleanCheckoutReclaim::NotApplicable);
    };
    let (candidate, removal) = tokio::task::spawn_blocking(move || {
        let removal = remove(&candidate);
        (candidate, removal)
    })
    .await
    .map_err(|error| anyhow::anyhow!("worktree release failed: {error}"))?;
    // The claim is released when `candidate` drops, after the outcome is recorded.
    db.with_conn(move |conn| settle_finished_checkout(conn, &candidate, removal))
        .await
}

/// What a reclaim over one project's finished executions did.
#[derive(Debug, Default)]
pub struct FinishedWorktreeReclaim {
    pub released: usize,
    /// Directory names of the finished executions' worktrees that were kept.
    pub kept: Vec<String>,
}

/// The finished executions of `project_id` that still hold a managed checkout:
/// the terminal ones and (KT-1676) those of a task closed by hand, whose
/// execution never reached a terminal state.
fn finished_checkout_executions(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> Result<Vec<String>> {
    let terminal = crate::db::orchestration::terminal_executions_with_checkout(conn)?
        .into_iter()
        .map(|(exec_id, _)| exec_id);
    let closed = crate::db::orchestration::closed_task_executions_with_checkout(conn)?;
    let mut ids = Vec::new();
    for exec_id in terminal.chain(closed) {
        let belongs = crate::db::discussion_workspaces::get_managed_for_execution(conn, &exec_id)?
            .is_some_and(|workspace| workspace.project_id == project_id);
        if belongs {
            ids.push(exec_id);
        }
    }
    Ok(ids)
}

/// The directory names of the finished executions' worktrees still on disk for
/// `project_id`: what a human can review and remove (database only, no Git).
fn finished_checkout_names(conn: &rusqlite::Connection, project_id: &str) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for exec_id in finished_checkout_executions(conn, project_id)? {
        let path = crate::db::discussion_workspaces::get_managed_for_execution(conn, &exec_id)?
            .and_then(|workspace| workspace.canonical_path);
        if let Some(name) = path.as_deref().and_then(|path| {
            scanner::resolve_host_path(path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        }) {
            names.push(name);
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// The worktrees that keep `project`'s catalogue over its cap after the reclaim,
/// each as `name (reason)` (KT-1676): the finished tasks' checkouts Kronn kept,
/// the checkouts of tasks still running, those a CLI session attached, and the
/// ones Kronn never created. Directory names only, never a path. `git worktree
/// list` is read-only; where it fails the finished ones are still named from
/// the database.
fn catalogue_blockers(
    conn: &rusqlite::Connection,
    project: &crate::models::Project,
) -> Vec<String> {
    let dir_of = |path: &str| {
        std::path::Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
    };
    let mut blockers = std::collections::BTreeMap::new();
    for name in finished_checkout_names(conn, &project.id).unwrap_or_default() {
        blockers.insert(
            name,
            "finished task, not clean: uncommitted changes, another branch or unreadable",
        );
    }
    let known: std::collections::HashMap<String, bool> = conn
        .prepare(
            "SELECT canonical_path, ownership = 'managed' FROM discussion_workspaces
              WHERE project_id = ?1 AND canonical_path IS NOT NULL",
        )
        .and_then(|mut statement| {
            statement
                .query_map([&project.id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(path, managed)| dir_of(&path).map(|name| (name, managed)))
        .collect();
    for path in crate::agents::runner::claude_task_worker_catalogue_worktrees(project) {
        let Some(name) = dir_of(&path.to_string_lossy()) else {
            continue;
        };
        blockers
            .entry(name.clone())
            .or_insert(match known.get(&name) {
                Some(true) => "task still running",
                Some(false) => "attached by a CLI session, not managed by Kronn",
                None => "not created by Kronn",
            });
    }
    blockers
        .into_iter()
        .map(|(name, reason)| format!("{name} ({reason})"))
        .collect()
}

/// Reclaim the clean checkouts of every finished execution of `project_id`
/// (KT-1646). One execution failing never stops the others.
pub async fn reclaim_clean_finished_checkouts(
    db: &Database,
    project_id: &str,
) -> Result<FinishedWorktreeReclaim> {
    let pid = project_id.to_string();
    let candidates = db
        .with_conn(move |conn| finished_checkout_executions(conn, &pid))
        .await?;
    Ok(reclaim_clean_checkouts(db, candidates).await)
}

/// Reclaim the clean checkout of each of `candidates`, each judged on its own:
/// one that is not finished is skipped, a dirty one is kept and reported.
async fn reclaim_clean_checkouts(
    db: &Database,
    candidates: Vec<String>,
) -> FinishedWorktreeReclaim {
    let mut report = FinishedWorktreeReclaim::default();
    for exec_id in candidates {
        match reclaim_clean_finished_checkout(db, &exec_id).await {
            Ok(CleanCheckoutReclaim::Released) => report.released += 1,
            Ok(CleanCheckoutReclaim::Kept { name, reason }) => {
                tracing::info!(execution_id = %exec_id, "finished worktree kept: {reason}");
                record_release_kept(db, &exec_id, &reason).await;
                report.kept.push(name);
            }
            Ok(CleanCheckoutReclaim::NotApplicable) => {}
            Err(error) => {
                tracing::warn!(execution_id = %exec_id, "finished worktree reclaim failed: {error}");
            }
        }
    }
    report.kept.sort();
    report.kept.dedup();
    report
}

/// Closing a task (`done` or `archived`) releases the managed checkouts of its
/// executions (KT-1676): a clean one is removed, its branch kept; a dirty one,
/// one on another branch or one a worker still uses is kept, and the first two
/// are reported by name. Unlike the cancel with `preserve`, which keeps a
/// checkout on purpose, a closed task has nothing left to preserve it for.
///
/// Never fails the caller: the task is already closed, and what stays is the
/// periodic sweep's to take.
pub async fn release_closed_task_checkouts(
    db: &Database,
    task_id: &str,
) -> FinishedWorktreeReclaim {
    let task = task_id.to_string();
    let candidates = match db
        .with_conn(move |conn| {
            crate::db::orchestration::executions_of_task_with_checkout(conn, &task)
        })
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(task = %task_id, "closed task checkouts unreadable: {error}");
            return FinishedWorktreeReclaim::default();
        }
    };
    let report = reclaim_clean_checkouts(db, candidates).await;
    if report.released + report.kept.len() > 0 {
        tracing::info!(
            task = %task_id,
            released = report.released,
            kept = ?report.kept,
            "task closed: released its managed worktrees"
        );
    }
    report
}

/// How a launch measures the Claude sandbox catalogue of a project: injected so
/// the reclaim is exercised on any host (the real measure only exists on macOS).
type CatalogueMeasure =
    fn(
        &crate::models::Project,
    ) -> Result<Option<crate::agents::runner::ClaudeSandboxCatalogueReceipt>, String>;

/// Before a Claude launch is judged (KT-1646): when the project's worktree
/// catalogue is over a cap, reclaim the clean worktrees of its finished
/// executions. Runs with no connection held, before the synchronous verdict
/// [`claude_catalogue_verdict`] re-measures. The cap itself protects the sandbox
/// and is never raised.
pub async fn reclaim_over_catalogue_cap(
    db: &Database,
    project: &crate::models::Project,
    measure: CatalogueMeasure,
) {
    let measured = {
        let project = project.clone();
        tokio::task::spawn_blocking(move || measure(&project)).await
    };
    // A measure that fails or does not apply is the verdict's to report.
    let Ok(Ok(Some(receipt))) = measured else {
        return;
    };
    if receipt.within_caps() {
        return;
    }
    // KT-1676 — a worktree removed outside Kronn still counts until it is pruned.
    if let Err(error) = reconcile_missing_managed_checkouts(db).await {
        tracing::warn!(project = %project.id, "managed checkout reconcile failed: {error}");
    }
    let reclaim = match reclaim_clean_finished_checkouts(db, &project.id).await {
        Ok(reclaim) => reclaim,
        Err(error) => {
            tracing::warn!(project = %project.id, "finished worktree reclaim failed: {error}");
            return;
        }
    };
    tracing::info!(
        project = %project.id,
        released = reclaim.released,
        kept = reclaim.kept.len(),
        "worktree catalogue over its cap: reclaimed finished worktrees before refusing"
    );
}

/// [`reclaim_over_catalogue_cap`] for the launch and preparation boundaries
/// (KT-1646), which only know the task and the worker: a no-op unless the worker
/// is a Claude agent and the task has exactly one project.
pub async fn reclaim_finished_worktrees_for_launch(
    db: &Database,
    task_reference: &str,
    worker: &MessageTarget,
) {
    if worker.kind != MessageTargetKind::Agent || worker.agent_type != AgentType::ClaudeCode {
        return;
    }
    let task_reference = task_reference.trim().to_string();
    let project = db
        .with_conn(move |conn| {
            let Some(task) = crate::db::planning::get_task(conn, &task_reference)? else {
                return Ok(None);
            };
            match task.summary.project_ids.as_slice() {
                [project_id] => crate::db::projects::get_project(conn, project_id),
                _ => Ok(None),
            }
        })
        .await;
    if let Ok(Some(project)) = project {
        reclaim_over_catalogue_cap(
            db,
            &project,
            crate::agents::runner::claude_task_worker_catalogue_measure,
        )
        .await;
    }
}

/// The Claude sandbox catalogue verdict for a launch (KT-1646): measure, and when
/// over a cap refuse, naming the worktrees that block (KT-1676) so a human knows
/// what to review and remove. Whatever could be reclaimed already was, by
/// [`reclaim_over_catalogue_cap`], before the connection was taken. The only Git
/// that runs here is the read-only listing of the registered worktrees.
pub fn claude_catalogue_verdict(
    conn: &rusqlite::Connection,
    project: &crate::models::Project,
    measure: CatalogueMeasure,
) -> Result<(), String> {
    let Some(receipt) = measure(project)? else {
        return Ok(());
    };
    if receipt.within_caps() {
        return Ok(());
    }
    receipt.validate_listing(&catalogue_blockers(conn, project))
}

/// Where the evidence of one execution's released checkout is kept.
pub fn terminal_evidence_dir(exec_id: &str) -> Result<std::path::PathBuf, String> {
    let safe = !exec_id.is_empty()
        && exec_id.len() <= 128
        && exec_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !safe {
        return Err(format!(
            "execution id {exec_id:?} cannot name an evidence directory"
        ));
    }
    let root = crate::core::config::config_dir().map_err(|error| error.to_string())?;
    Ok(root.join("task-evidence").join(exec_id))
}

/// Release the checkout of an execution that is already terminal (KT-1397):
/// uncommitted changes are saved as a patch under the execution's evidence
/// directory, the checkout is removed with its branch kept, its lane build dir
/// goes with it, and the workspace row is retired with its lineage.
///
/// The caller proves the execution is terminal (and, for a sweep, that no
/// session still holds the checkout). Returns a one-line outcome; any `Err`
/// leaves the checkout exactly as it was.
async fn release_terminal_checkout(
    db: &Database,
    exec_id: &str,
    repo: &std::path::Path,
    checkout_path: &str,
    branch: &str,
    final_head: Option<String>,
) -> Result<String, String> {
    let evidence = terminal_evidence_dir(exec_id)?;
    let evidence_dir = evidence.clone();
    let repo_owned = repo.to_path_buf();
    let path_owned = scanner::resolve_host_path(checkout_path)
        .to_string_lossy()
        .to_string();
    let branch_owned = branch.to_string();
    let released = tokio::task::spawn_blocking(move || {
        worktree::release_terminal_task_worktree(&repo_owned, &path_owned, &branch_owned, &evidence)
    })
    .await
    .map_err(|error| format!("worktree release failed: {error}"))??;
    // The DB's own path string: the lane audit looks the row up by that value.
    reclaim_lane_target(db, repo, checkout_path).await;
    let patch = match &released {
        worktree::TerminalWorktreeRelease::Removed { patch } => patch.clone(),
        worktree::TerminalWorktreeRelease::AlreadyGone => None,
    };
    let id = exec_id.to_string();
    let staged_patch =
        std::path::Path::new(&evidence_dir).join(worktree::RELEASED_WORKTREE_STAGED_PATCH);
    let staged_patch = (patch.is_some() && staged_patch.exists())
        .then(|| staged_patch.to_string_lossy().to_string());
    let event = serde_json::json!({
        "path": checkout_path,
        "branch": branch,
        "already_gone": released == worktree::TerminalWorktreeRelease::AlreadyGone,
        "uncommitted_patch": patch.as_ref().map(|p| p.to_string_lossy().to_string()),
        "staged_patch": staged_patch,
    });
    db.with_conn(move |conn| {
        let tx = conn.unchecked_transaction()?;
        crate::db::discussion_workspaces::retire_managed_for_execution(
            &tx,
            &id,
            final_head.as_deref(),
        )?;
        crate::db::orchestration::record_execution_event(
            &tx,
            &id,
            "worktree_released",
            None,
            None,
            &backend_actor(),
            event,
        )?;
        tx.commit()?;
        Ok(())
    })
    .await
    .map_err(|error| format!("checkout removed but DB cleanup failed: {error}"))?;
    Ok(match patch {
        Some(patch) => format!(
            "checkout removed, branch kept, uncommitted changes saved to {}",
            patch.display()
        ),
        None => "clean checkout removed, provenance preserved".to_string(),
    })
}

/// Release the managed checkout of one finished execution found by a sweep.
/// Unlike a cancel that just signalled its worker, a sweep proves nothing
/// still uses the checkout: a live session or lease keeps it. `Ok(None)` is
/// an execution with nothing to release.
async fn release_finished_execution_checkout(
    db: &Database,
    exec_id: &str,
    min_idle: std::time::Duration,
) -> Result<Option<String>, String> {
    let id = exec_id.to_string();
    let snapshot = db
        .with_conn(move |conn| {
            let Some(execution) = crate::db::orchestration::get_task_execution(conn, &id)? else {
                return Ok(None);
            };
            let Some(workspace) =
                crate::db::discussion_workspaces::get_managed_for_execution(conn, &id)?
            else {
                return Ok(None);
            };
            let Some(path) = workspace.canonical_path.clone() else {
                return Ok(None);
            };
            let liveness = crate::db::orchestration::worktree_cleanup_liveness(conn, &path)?;
            let project_path = crate::db::projects::get_project(conn, &workspace.project_id)?
                .map(|project| project.path);
            Ok(Some((execution, workspace, path, liveness, project_path)))
        })
        .await
        .map_err(|error| format!("execution {exec_id} unreadable: {error}"))?;
    let Some((execution, workspace, path, liveness, project_path)) = snapshot else {
        return Ok(None);
    };
    if !execution.status.is_terminal() {
        return Err(format!("execution {exec_id} is not finished"));
    }
    match liveness {
        worktree::ExecutionLiveness::Terminal => {}
        worktree::ExecutionLiveness::Active(reason)
        | worktree::ExecutionLiveness::Unknown(reason) => return Err(reason),
    }
    let project_path =
        project_path.ok_or_else(|| format!("project {} is unavailable", workspace.project_id))?;
    let repo = scanner::resolve_host_path(&project_path);
    // `finished_at` and the session state say the execution is over, not that
    // nobody writes in the checkout any more (a human may be working in it).
    // The newest write in the tree restarts the inactivity window; an activity
    // that cannot be read is never idle.
    if let Some(reason) = recent_checkout_activity(&path, min_idle).await {
        record_release_kept(db, exec_id, &reason).await;
        return Err(reason);
    }
    match release_terminal_checkout(
        db,
        exec_id,
        &repo,
        &path,
        &workspace.branch,
        workspace.head_sha.clone(),
    )
    .await
    {
        Ok(outcome) => Ok(Some(outcome)),
        Err(reason) => {
            // A refusal that rests on the checkout itself (an evidence cap, a
            // diff that cannot be saved, a foreign branch) is the human's to
            // see: recorded once per distinct reason, not once per sweep.
            record_release_kept(db, exec_id, &reason).await;
            Err(reason)
        }
    }
}

/// Why a checkout is not idle yet: `Some(reason)` when something in it was
/// written within `min_idle` or its age cannot be established. The reason is
/// the same text on every sweep so the event log records it once.
async fn recent_checkout_activity(
    checkout_path: &str,
    min_idle: std::time::Duration,
) -> Option<String> {
    let path = scanner::resolve_host_path(checkout_path);
    let walked = tokio::task::spawn_blocking(move || {
        if !path.exists() {
            return Ok(None);
        }
        let cutoff = std::time::SystemTime::now()
            .checked_sub(min_idle)
            .unwrap_or(std::time::UNIX_EPOCH);
        worktree::checkout_written_since(&path, cutoff)
    })
    .await;
    match walked {
        Ok(Ok(None)) => None,
        Ok(Ok(Some(newest))) => {
            tracing::debug!(newest = %newest.display(), "checkout written within the idle window");
            Some(format!(
                "the checkout was written within the last {} s (server.disk_sweep_idle_hours) — keeping it",
                min_idle.as_secs()
            ))
        }
        Ok(Err(error)) => Some(format!(
            "the checkout's last write cannot be established ({error}) — keeping it"
        )),
        Err(error) => Some(format!(
            "the checkout activity scan failed: {error} — keeping it"
        )),
    }
}

/// Record, once per distinct reason, that the release kept a finished
/// execution's checkout.
async fn record_release_kept(db: &Database, exec_id: &str, reason: &str) {
    let id = exec_id.to_string();
    let recorded = reason.to_string();
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::orchestration::record_release_kept_once(
                conn,
                &id,
                &recorded,
                &backend_actor(),
            )
        })
        .await
    {
        tracing::warn!(execution_id = %exec_id, "kept-worktree reason not recorded: {error}");
    }
}

/// The periodic counterpart of the boot pass (KT-1397): Kronn stays up for days,
/// and a Failed or Cancelled execution reaches its terminal state at runtime.
/// Uses the configured `server.disk_sweep_idle_hours`. Returns the checkouts
/// released and kept, and logs what it released.
pub async fn release_finished_worktrees(db: &Database) -> (usize, usize) {
    let mut report = OrchestrationBootReport::default();
    release_terminal_worktrees(db, crate::core::disk_usage::sweep_idle(), &mut report).await;
    for error in &report.errors {
        tracing::warn!("periodic worktree release: {error}");
    }
    for line in &report.lane_target_report {
        tracing::info!("periodic worktree release: {line}");
    }
    (
        report.terminal_worktrees_released,
        report.terminal_worktrees_kept,
    )
}

/// The checkouts of executions that never finished although their task was
/// closed by hand (KT-1676): a clean one goes at once, a dirty one is kept and
/// its reason recorded once. Executions that are really finished are the
/// terminal pass's.
async fn release_closed_task_executions(db: &Database, report: &mut OrchestrationBootReport) {
    let candidates = match db
        .with_conn(crate::db::orchestration::closed_task_executions_with_checkout)
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            report
                .errors
                .push(format!("closed task worktree scan: {error}"));
            return;
        }
    };
    for exec_id in candidates {
        match reclaim_clean_finished_checkout(db, &exec_id).await {
            Ok(CleanCheckoutReclaim::Released) => {
                report.terminal_worktrees_released += 1;
                report.lane_target_report.push(format!(
                    "released execution {exec_id} of a closed task: clean checkout removed, branch kept"
                ));
            }
            Ok(CleanCheckoutReclaim::Kept { name, reason }) => {
                tracing::info!(execution_id = %exec_id, worktree = %name, "closed task worktree kept: {reason}");
                record_release_kept(db, &exec_id, &reason).await;
                report.terminal_worktrees_kept += 1;
            }
            Ok(CleanCheckoutReclaim::NotApplicable) => {}
            Err(error) => report.errors.push(format!(
                "execution {exec_id} closed task checkout reclaim: {error}"
            )),
        }
    }
}

/// Find the managed checkouts Kronn still records as `attached` although their
/// directory was removed outside Kronn (`git worktree remove`, `rm -rf`), and
/// make the record true (KT-1676). Returns how many were gone.
///
/// A finished execution's (or closed task's) checkout is released as usual,
/// which retires the record. One a worker may still use is marked `missing`: the
/// row, its branch and its lineage stay, so the execution remains resumable,
/// and the event says what happened. Either way the dead Git entry is pruned, so
/// it stops counting against the worktree cap. A project whose repository
/// cannot be found is left alone: an unmounted disk is not a removal.
pub async fn reconcile_missing_managed_checkouts(db: &Database) -> Result<usize> {
    let workspaces = db
        .with_conn(|conn| {
            let mut gone = Vec::new();
            for workspace in
                crate::db::discussion_workspaces::list_attached_managed_with_path(conn)?
            {
                let Some(project) = crate::db::projects::get_project(conn, &workspace.project_id)?
                else {
                    continue;
                };
                gone.push((workspace, project.path));
            }
            Ok(gone)
        })
        .await?;
    let missing = tokio::task::spawn_blocking(move || {
        workspaces
            .into_iter()
            .filter(|(workspace, project_path)| {
                let checkout = workspace
                    .canonical_path
                    .as_deref()
                    .map(scanner::resolve_host_path);
                let repo = scanner::resolve_host_path(project_path);
                matches!(repo.try_exists(), Ok(true))
                    && checkout.is_some_and(|path| matches!(path.try_exists(), Ok(false)))
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|error| anyhow::anyhow!("managed checkout scan failed: {error}"))?;
    let mut gone = 0;
    for (workspace, project_path) in missing {
        let Some(exec_id) = workspace.task_execution_id.clone() else {
            continue;
        };
        if crate::core::checkout_release::is_claimed(&exec_id) {
            continue;
        }
        gone += 1;
        tracing::info!(execution_id = %exec_id, "managed checkout is gone from disk");
        if reclaim_clean_finished_checkout(db, &exec_id).await? == CleanCheckoutReclaim::Released {
            continue;
        }
        let path = workspace.canonical_path.clone();
        let branch = workspace.branch.clone();
        let id = exec_id.clone();
        db.with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            crate::db::discussion_workspaces::mark_missing_for_execution(&tx, &id)?;
            crate::db::orchestration::record_execution_event(
                &tx,
                &id,
                "worktree_missing",
                None,
                None,
                &backend_actor(),
                serde_json::json!({ "path": path, "branch": branch }),
            )?;
            tx.commit()?;
            Ok(())
        })
        .await?;
        let repo = scanner::resolve_host_path(&project_path);
        let _ = tokio::task::spawn_blocking(move || {
            crate::core::cmd::git_cmd()
                .args(["worktree", "prune"])
                .current_dir(repo)
                .output()
        })
        .await;
    }
    Ok(gone)
}

/// One pass over the finished executions that still hold a managed checkout
/// (KT-1397). Only an execution durable state proves terminal, finished at
/// least `min_idle` ago, whose workspace is Kronn's and unused, is released;
/// anything else is counted as kept and left alone.
pub async fn release_terminal_worktrees(
    db: &Database,
    min_idle: std::time::Duration,
    report: &mut OrchestrationBootReport,
) {
    // KT-1676 — first make the records true: a checkout removed outside Kronn
    // must not stay `attached`, nor count against the worktree cap.
    match reconcile_missing_managed_checkouts(db).await {
        Ok(gone) if gone > 0 => report.lane_target_report.push(format!(
            "{gone} managed checkout(s) removed outside Kronn: workspace record(s) updated"
        )),
        Ok(_) => {}
        Err(error) => report
            .errors
            .push(format!("managed checkout reconcile: {error}")),
    }
    release_closed_task_executions(db, report).await;
    let candidates = match db
        .with_conn(crate::db::orchestration::terminal_executions_with_checkout)
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            report
                .errors
                .push(format!("terminal worktree scan: {error}"));
            return;
        }
    };
    let now = chrono::Utc::now();
    for (exec_id, finished_at) in candidates {
        // KT-1646 — a clean checkout of a finished execution goes at once; the
        // idle window below only guards the dirty ones, whose diff is saved first.
        match reclaim_clean_finished_checkout(db, &exec_id).await {
            Ok(CleanCheckoutReclaim::Released) => {
                report.terminal_worktrees_released += 1;
                report.lane_target_report.push(format!(
                    "released finished execution {exec_id}: clean checkout removed, branch kept"
                ));
                continue;
            }
            Ok(CleanCheckoutReclaim::Kept { name, reason }) => {
                tracing::info!(execution_id = %exec_id, worktree = %name, "finished worktree kept: {reason}");
            }
            Ok(CleanCheckoutReclaim::NotApplicable) => {}
            Err(error) => report.errors.push(format!(
                "execution {exec_id} clean checkout reclaim: {error}"
            )),
        }
        let recent = finished_at.is_some_and(|at| {
            now.signed_duration_since(at)
                .to_std()
                .map(|age| age < min_idle)
                .unwrap_or(true)
        });
        if recent {
            report.terminal_worktrees_kept += 1;
            continue;
        }
        match release_finished_execution_checkout(db, &exec_id, min_idle).await {
            Ok(Some(outcome)) => {
                report.terminal_worktrees_released += 1;
                report
                    .lane_target_report
                    .push(format!("released finished execution {exec_id}: {outcome}"));
            }
            Ok(None) => {}
            Err(reason) => {
                report.terminal_worktrees_kept += 1;
                tracing::warn!(execution_id = %exec_id, "finished worktree kept: {reason}");
            }
        }
    }
}

/// What a lane-target sweep needs from durable state: for each lane Kronn can
/// attribute, why it must stay (absent = its execution is provably finished),
/// plus the lanes durable lineage attributes to an execution although no marker
/// says so (KT-1397).
pub async fn lane_target_claims(
    db: &Database,
    project_id: &str,
    repo_path: &std::path::Path,
) -> Result<
    (
        std::collections::HashMap<String, String>,
        worktree::LaneAttributions,
    ),
    String,
> {
    let repo = repo_path.to_path_buf();
    let marked = tokio::task::spawn_blocking(move || worktree::lane_target_owners(&repo))
        .await
        .map_err(|error| format!("lane target scan failed: {error}"))?;
    let pid = project_id.to_string();
    db.with_conn(move |conn| {
        let attributions = crate::db::orchestration::lane_attributions(conn, &pid)?;
        let mut owners = marked;
        owners.extend(attributions.values().cloned());
        let claims = crate::db::orchestration::lane_target_claims(conn, &pid, &owners)?;
        Ok((claims, attributions))
    })
    .await
    .map_err(|error| format!("durable state unreadable: {error}"))
}

/// Record, per lane target, what a sweep removed (an execution's event log
/// outlives the log line that would otherwise be the only trace).
async fn audit_lane_target_sweep(db: &Database, sweep: &worktree::LaneTargetSweep) {
    for (owner, report) in &sweep.removed {
        let execution_id = owner.execution_id.clone();
        let target = report.target_path.to_string_lossy().to_string();
        let outcome = Ok((report.bytes_reclaimed, report.bytes_are_partial));
        if let Err(error) = db
            .with_conn(move |conn| {
                crate::db::orchestration::record_lane_target_reclaim_for_execution(
                    conn,
                    &execution_id,
                    &target,
                    outcome,
                )
            })
            .await
        {
            tracing::warn!(lane = %owner.lane, "lane target sweep audit failed: {error}");
        }
    }
}

/// One pass over a project's `target/wt-*` directories (KT-1256).
///
/// Only a lane carrying Kronn's ownership marker is ever a candidate, and only
/// when durable state proves its execution finished and its worktree is gone
/// (or clean, with the wider `scope`). Everything else is reported and left as
/// it is: `scope` [`worktree::LaneSweepScope::ReportOnly`] is the dry run.
pub async fn sweep_project_lane_targets(
    db: &Database,
    project_id: &str,
    repo_path: &std::path::Path,
    scope: worktree::LaneSweepScope,
) -> Result<worktree::LaneTargetSweep, String> {
    sweep_project_lane_targets_idle(db, project_id, repo_path, scope, std::time::Duration::ZERO)
        .await
}

/// [`sweep_project_lane_targets`] that also leaves every lane written within
/// `min_idle` (KT-1318): the boot, periodic and pressure sweeps all use the
/// configured `server.disk_sweep_idle_hours`.
pub async fn sweep_project_lane_targets_idle(
    db: &Database,
    project_id: &str,
    repo_path: &std::path::Path,
    scope: worktree::LaneSweepScope,
    min_idle: std::time::Duration,
) -> Result<worktree::LaneTargetSweep, String> {
    let (claims, attributions) = lane_target_claims(db, project_id, repo_path).await?;
    let repo = repo_path.to_path_buf();
    let sweep = tokio::task::spawn_blocking(move || {
        let judge = |owner: &worktree::LaneOwner| match claims.get(&owner.lane) {
            Some(reason) => Err(reason.clone()),
            None => Ok(()),
        };
        worktree::sweep_lane_targets_attributed(&repo, &judge, scope, min_idle, &attributions)
    })
    .await
    .map_err(|error| format!("lane target sweep failed: {error}"))?;
    audit_lane_target_sweep(db, &sweep).await;
    Ok(sweep)
}

/// Reclaim exactly the named lane targets (KT-1256), each re-judged now: the
/// dry run informs the human, it never authorises the deletion. The result is
/// the sweep outcome per named path, in the order given.
pub async fn reclaim_named_lane_targets(
    db: &Database,
    project_id: &str,
    repo_path: &std::path::Path,
    targets: Vec<std::path::PathBuf>,
) -> Result<Vec<(std::path::PathBuf, worktree::LaneTargetOutcome)>, String> {
    let (claims, attributions) = lane_target_claims(db, project_id, repo_path).await?;
    let repo = repo_path.to_path_buf();
    let outcomes = tokio::task::spawn_blocking(move || {
        let judge = |owner: &worktree::LaneOwner| match claims.get(&owner.lane) {
            Some(reason) => Err(reason.clone()),
            None => Ok(()),
        };
        targets
            .into_iter()
            .map(|target| {
                let outcome = worktree::process_lane_target_attributed(
                    &repo,
                    &target,
                    &judge,
                    worktree::LaneSweepScope::WorktreeGoneOrClean,
                    std::time::Duration::ZERO,
                    &attributions,
                );
                (target, outcome)
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|error| format!("lane target reclaim failed: {error}"))?;
    let mut removed = worktree::LaneTargetSweep::default();
    for (_, outcome) in &outcomes {
        if let worktree::LaneTargetOutcome::Removed(owner, report) = outcome {
            removed.removed.push((owner.clone(), report.clone()));
        }
    }
    audit_lane_target_sweep(db, &removed).await;
    Ok(outcomes)
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

pub enum CheckpointStep {
    Anchored(String),
    Built(String),
    Validating,
    Armed(String),
    Integrated(String),
    /// KT-1519 — landed on this non-default branch: the task awaits the merge.
    IntegratedAwaitingMerge(String, String),
}

/// Commit one saga checkpoint, mapping a lost race to an internal error: the caller
/// is mid-integration and must not carry on against a row that moved.
pub async fn checkpoint(
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
                CheckpointStep::IntegratedAwaitingMerge(sha, branch) => {
                    IntegrationStep::IntegratedAwaitingMerge {
                        integrated_sha: sha,
                        branch,
                    }
                }
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

/// Why the integration handed an approved delivery back.
struct ReworkReport {
    /// One line naming the cause; it is also the `SentBack` reason.
    reason: String,
    /// The validation that went red, when that is the cause.
    failure: Option<ValidationFailure>,
}

impl ReworkReport {
    fn conflict(reason: String) -> Self {
        Self {
            reason,
            failure: None,
        }
    }

    fn validation(failure: ValidationFailure) -> Self {
        Self {
            reason: format!("validation failed: {}", failure.command),
            failure: Some(failure),
        }
    }
}

/// How much of a failing validation's output travels in a message. The end is
/// kept, because a compiler or a test runner prints its verdict last, and so is
/// the start (KT-1438): the first error (a Twig exception, the first failing
/// test) is what a worker needs, and a wrapper's last lines are only its own
/// `make: Error 1`.
pub const REWORK_OUTPUT_LIMIT: usize = 4_000;
const REWORK_OUTPUT_HEAD: usize = 1_500;

/// A fenced block that the text itself cannot close early.
pub fn fenced_output(output: &str, lang: RoomLang) -> String {
    // Redacted whole, before the cut: a credential split by it would escape the
    // patterns, and the head kept since KT-1438 is where errors print theirs.
    let output = crate::core::redact::redact_stored_text(output);
    let output = output.as_str();
    let total = output.chars().count();
    let shown: String = if total > REWORK_OUTPUT_LIMIT {
        let tail_len = REWORK_OUTPUT_LIMIT - REWORK_OUTPUT_HEAD;
        let head: String = output.chars().take(REWORK_OUTPUT_HEAD).collect();
        let tail: String = output.chars().skip(total - tail_len).collect();
        let omitted = total - REWORK_OUTPUT_LIMIT;
        match lang {
            RoomLang::Fr => format!("{head}\n[… {omitted} caractères omis]\n{tail}"),
            RoomLang::En => format!("{head}\n[… {omitted} characters omitted]\n{tail}"),
        }
    } else {
        output.to_string()
    };
    let longest_run = shown
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let fence = "`".repeat((longest_run + 1).max(3));
    format!("{fence}text\n{}\n{fence}", shown.trim_end())
}

/// The evidence block both messages carry: the command, how it ended and what it
/// printed, or the bare cause when no validation is involved (a merge conflict).
fn rework_evidence(report: &ReworkReport, lang: RoomLang) -> String {
    let Some(failure) = &report.failure else {
        return format!("**{}** {}", lang.pick("Cause :", "Cause:"), report.reason);
    };
    let code = failure
        .exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| {
            lang.pick(
                "aucun (le processus n'a pas abouti)",
                "none (the process did not complete)",
            )
            .to_string()
        });
    let output = if failure.output.trim().is_empty() {
        lang.pick(
            "_(la commande n'a rien affiché)_",
            "_(the command printed nothing)_",
        )
        .to_string()
    } else {
        fenced_output(&failure.output, lang)
    };
    match lang {
        RoomLang::Fr => format!(
            "**Commande :** `{}`\n**Code de sortie :** {code}\n\n**Sortie :**\n{output}",
            failure.command
        ),
        RoomLang::En => format!(
            "**Command:** `{}`\n**Exit code:** {code}\n\n**Output:**\n{output}",
            failure.command
        ),
    }
}

/// What the worker reads in its own room when the integration sends its approved
/// delivery back: the cause, the command and its output, and the way forward.
/// Deterministic id per `(exec, attempt)` so a resume never double-posts.
fn build_integration_rework_message(
    exec_id: &str,
    next_attempt: u32,
    task_reference: &str,
    task_title: &str,
    target_branch: Option<&str>,
    report: &ReworkReport,
    lang: RoomLang,
) -> DiscussionMessage {
    let reason = &report.reason;
    let evidence = rework_evidence(report, lang);
    let content = match lang {
        RoomLang::Fr => {
            let way_forward = match (&report.failure, target_branch) {
                (Some(_), _) => "Reproduis l'échec, corrige-le **dans ce worktree**".to_string(),
                (None, Some(target)) => format!(
                    "Corrige la cause ci-dessus **dans ce worktree** (pour un conflit : intègre \
                     `{target}` dans ta branche et résous-le)"
                ),
                (None, None) => "Corrige la cause ci-dessus **dans ce worktree**".to_string(),
            };
            format!(
                "**Intégration refusée — {task_reference} : {task_title}**\n\n\
                 Ta livraison a été approuvée, mais l'intégration protégée l'a renvoyée : \
                 {reason}. La branche cible n'a pas bougé.\n\n\
                 {evidence}\n\n\
                 {way_forward} (il est conservé, comme cette sous-discussion), puis re-livre un \
                 nouveau DeliveryManifest via `task_exec_deliver`. Si l'échec ne vient pas de ton \
                 changement, dis-le dans le résumé de ta livraison."
            )
        }
        RoomLang::En => {
            let way_forward = match (&report.failure, target_branch) {
                (Some(_), _) => "Reproduce the failure, fix it **in this worktree**".to_string(),
                (None, Some(target)) => format!(
                    "Fix the cause above **in this worktree** (for a conflict: integrate \
                     `{target}` into your branch and resolve it)"
                ),
                (None, None) => "Fix the cause above **in this worktree**".to_string(),
            };
            format!(
                "**Integration refused — {task_reference}: {task_title}**\n\n\
                 Your delivery was approved, but the protected integration sent it back: \
                 {reason}. The target branch did not move.\n\n\
                 {evidence}\n\n\
                 {way_forward} (it is kept, like this sub-discussion), then re-deliver a new \
                 DeliveryManifest through `task_exec_deliver`. If the failure does not come from \
                 your change, say so in your delivery summary."
            )
        }
    };
    orchestrator_message(
        format!("orch-integration-rework:{exec_id}:{next_attempt}"),
        content,
    )
}

/// What the principal that approved reads in the parent room when the
/// integration sends the delivery back: the same evidence the worker got, and the
/// two ways forward. Deterministic id per `(exec, attempt)`.
fn build_integration_rework_notice(
    exec_id: &str,
    next_attempt: u32,
    task_reference: &str,
    task_title: &str,
    child_discussion_id: &str,
    report: &ReworkReport,
    lang: RoomLang,
) -> DiscussionMessage {
    let reason = &report.reason;
    let evidence = rework_evidence(report, lang);
    let content = match lang {
        RoomLang::Fr => format!(
            "**Intégration renvoyée au worker — {task_reference}**\n\n\
             L'exécution `{exec_id}` (**{task_title}**) était approuvée, mais l'intégration l'a \
             renvoyée : {reason}. La branche cible n'a pas bougé.\n\n\
             {evidence}\n\n\
             Le worker est relancé avec cette sortie dans la sous-discussion \
             `{child_discussion_id}` (tentative {next_attempt}). Deux issues :\n\
             - l'échec vient de la livraison : attends la re-livraison, puis relis-la avec \
             `task_exec_review` ;\n\
             - l'échec ne vient pas de la livraison (test instable, environnement) : relance les \
             mêmes validations sur le même candidat, sans nouvelle livraison, avec \
             `task_exec_resume`."
        ),
        RoomLang::En => format!(
            "**Integration sent back to the worker — {task_reference}**\n\n\
             Execution `{exec_id}` (**{task_title}**) was approved, but the integration sent it \
             back: {reason}. The target branch did not move.\n\n\
             {evidence}\n\n\
             The worker is relaunched with this output in the sub-discussion \
             `{child_discussion_id}` (attempt {next_attempt}). Two outcomes:\n\
             - the failure comes from the delivery: wait for the re-delivery, then review it \
             again with `task_exec_review`;\n\
             - the failure does not come from the delivery (flaky test, environment): rerun the \
             same validations on the same candidate, with no new delivery, using \
             `task_exec_resume`."
        ),
    };
    orchestrator_message(
        format!("orch-integration-reworked:{exec_id}:{next_attempt}"),
        content,
    )
}

/// Hand the work back to the worker without touching branch, worktree or task.
///
/// One durable checkpoint sends the failure to the worker in its room, wakes the
/// worker for the next attempt (a joined CLI through a control offer, a native
/// worker through a dispatch — the way a reviewer's `request_changes` does) and
/// tells the principal that approved. A send-back that only flipped the status
/// left the execution `ChangesRequested` with nobody working it and nobody told.
async fn send_back(
    db: &Database,
    exec_id: &str,
    report: &ReworkReport,
) -> Result<(), ProvisionError> {
    let internal = |e: String| ProvisionError::Internal(e);
    let (execution, task, target_branch, head_sha) = {
        let id = exec_id.to_string();
        let (execution, task, run, workspace) = db
            .with_conn(move |conn| {
                let execution = crate::db::orchestration::get_task_execution(conn, &id)?
                    .context("execution vanished before integration rework")?;
                let task = crate::db::planning::get_task(conn, &execution.task_id)?
                    .context("task vanished before integration rework")?;
                let run = crate::db::orchestration::get_orchestration_run(
                    conn,
                    &execution.orchestration_run_id,
                )?;
                let workspace =
                    crate::db::discussion_workspaces::get_managed_for_execution(conn, &id)?;
                Ok((execution, task, run, workspace))
            })
            .await
            .map_err(|e| internal(e.to_string()))?;
        let head_sha = workspace
            .and_then(|workspace| workspace.canonical_path)
            .and_then(|path| worktree::resolve_commit(std::path::Path::new(&path), "HEAD").ok());
        (
            execution,
            task,
            run.and_then(|run| run.target_branch),
            head_sha,
        )
    };
    let child = execution.sub_discussion_id.clone().ok_or_else(|| {
        internal("execution has no sub-discussion to send the work back to".into())
    })?;
    let worker_target =
        worker_target_from_execution(&execution).map_err(|e| internal(e.to_string()))?;
    let next_attempt = execution.attempt_no + 1;
    let reference = task.summary.reference.as_str();
    let title = task.summary.title.as_str();
    let lang = room_lang(db, &execution.parent_discussion_id).await;
    let worker_message = build_integration_rework_message(
        &execution.id,
        next_attempt,
        reference,
        title,
        target_branch.as_deref(),
        report,
        lang,
    );
    let principal_message = build_integration_rework_notice(
        &execution.id,
        next_attempt,
        reference,
        title,
        &child,
        report,
        lang,
    );
    // The same two re-activation shapes a reviewer's `request_changes` uses: a
    // joined CLI re-accepts a control offer, a native worker gets a fresh dispatch.
    let control_offer = execution.worker_cli_session_id.map(|_| {
        let offer_id = Uuid::new_v4().to_string();
        let message = build_control_offer_message(
            &execution.id,
            next_attempt,
            &offer_id,
            reference,
            title,
            &child,
            lang,
        );
        (offer_id, message)
    });
    let native_dispatch = execution.worker_cli_session_id.is_none().then(|| {
        (
            Uuid::new_v4().to_string(),
            format!("orch-rework:{}:{next_attempt}", execution.id),
        )
    });

    let reason = report.reason.clone();
    let outcome = db
        .with_conn(move |conn| {
            let reactivation = match (&control_offer, worker_target.cli_session_id) {
                (Some((offer_id, control_message)), Some(session)) => {
                    Some(crate::db::orchestration::ReworkReoffer {
                        offer_id,
                        new_attempt_no: next_attempt,
                        target_cli_session_id: session,
                        origin_discussion_id: &execution.parent_discussion_id,
                        child_discussion_id: &child,
                        control_message,
                        control_target: &worker_target,
                    })
                }
                _ => None,
            };
            let native_dispatch = native_dispatch.as_ref().map(|(job_id, dedupe_key)| {
                crate::db::orchestration::NativeReworkDispatch { job_id, dedupe_key }
            });
            crate::db::orchestration::commit_integration_rework(
                conn,
                &crate::db::orchestration::IntegrationRework {
                    exec_id: &execution.id,
                    from_attempt: execution.attempt_no,
                    reason: &reason,
                    head_sha: head_sha.as_deref(),
                    child_discussion_id: &child,
                    worker_message: &worker_message,
                    worker_target: &worker_target,
                    principal_message: &principal_message,
                    reactivation,
                    native_dispatch,
                    actor: &backend_actor(),
                },
            )
        })
        .await
        .map_err(|e| internal(e.to_string()))?;
    match outcome {
        crate::db::orchestration::IntegrationReworkOutcome::Reworked => Ok(()),
        crate::db::orchestration::IntegrationReworkOutcome::ExecutionRaced => {
            Err(ProvisionError::CheckpointRefused(
                "the execution moved while the integration sent it back".into(),
            ))
        }
    }
}

/// How far one run's validations reach beyond the lane sandbox (KT-1433): the
/// project's setting, widened to Docker by a grant on the run's discussion.
struct ValidationReach {
    access: crate::models::ValidationsAccess,
    /// Resolved only when the access includes Docker.
    socket: Option<Result<std::path::PathBuf, String>>,
    /// KT-1492: the reach above was widened by a trust level, not by a human:
    /// the validation runs under the level's sandbox, away from Kronn's data.
    by_trust: Option<crate::models::TrustApproval>,
}

/// The project's access, widened to Docker by a grant on the discussion; a grant
/// never narrows what the project already has.
pub fn widened_access(
    project: crate::models::ValidationsAccess,
    discussion_docker: bool,
) -> crate::models::ValidationsAccess {
    use crate::models::ValidationsAccess;
    match project {
        ValidationsAccess::Sandbox if discussion_docker => ValidationsAccess::Docker,
        access => access,
    }
}

/// KT-1492 — the access a trust level opens on its own, and the approval to
/// journal when it widened what the project and the discussion already gave:
/// Trusted reaches Docker, Full access drops the sandbox. Never narrows.
pub fn widened_by_trust(
    access: crate::models::ValidationsAccess,
    level: crate::models::TrustLevel,
) -> (
    crate::models::ValidationsAccess,
    Option<crate::models::TrustApproval>,
) {
    use crate::core::trust::grants;
    use crate::models::{TrustApproval, ValidationsAccess};
    if access != ValidationsAccess::Full && grants(level, TrustApproval::ValidationsUnconfined) {
        return (
            ValidationsAccess::Full,
            Some(TrustApproval::ValidationsUnconfined),
        );
    }
    if access == ValidationsAccess::Sandbox && grants(level, TrustApproval::ValidationsDocker) {
        return (
            ValidationsAccess::Docker,
            Some(TrustApproval::ValidationsDocker),
        );
    }
    (access, None)
}

/// What a validation is launched with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationReachLaunch {
    Sandbox,
    Docker(std::path::PathBuf),
    /// No lane sandbox: a human chose full access for the project.
    Host,
    /// KT-1492: Docker, reached because a trust level said so. The lane sandbox
    /// stays, with Kronn's data directory closed to it.
    TrustDocker(std::path::PathBuf),
    /// KT-1492: the host, reached because a trust level said so (Full access).
    /// The command runs as the user, with Kronn's data directory closed to it
    /// (`core::trust_sandbox`) when the host has a sandbox and the owner kept
    /// the vault protection on.
    TrustHost,
}

/// Why a validation could not run, as the human fixes it.
pub struct EnvironmentCause {
    pub fix: String,
    /// The sandbox kept Docker closed: an elevation card can open it.
    pub docker_elevation: bool,
}

/// The capability an elevation card grants a discussion's runs.
const DOCKER_CAPABILITY: &str = "validations_docker";

impl ValidationReach {
    async fn of_run(db: &Database, project_id: Option<&str>, discussion_id: &str) -> Self {
        use crate::models::ValidationsAccess;
        let project = project_id.map(str::to_string);
        let discussion = discussion_id.to_string();
        let (access, by_trust) = db
            .with_conn(move |conn| {
                let access = match &project {
                    Some(id) => crate::db::projects::validations_access(conn, id)?,
                    None => ValidationsAccess::Sandbox,
                };
                let granted = crate::db::projects::discussion_capability_granted(
                    conn,
                    &discussion,
                    DOCKER_CAPABILITY,
                )?;
                let access = widened_access(access, granted);
                // KT-1492: the room's trust level, journalled when it widens.
                let origin = crate::db::trust::TrustOrigin::room(&discussion);
                let trust = crate::db::trust::effective(conn, &origin)?;
                let (widened, approval) = widened_by_trust(access, trust.level);
                let Some(approval) = approval else {
                    return Ok((access, None));
                };
                // Fails closed: with no sandbox on this host the level grants
                // nothing, and the reach stays what the project gave.
                let granted = crate::db::trust::grant(
                    conn,
                    &origin,
                    approval,
                    "Integration validations",
                    project.as_deref(),
                    None,
                )?;
                Ok(match granted {
                    Some(_) => (widened, Some(approval)),
                    None => (access, None),
                })
            })
            .await
            .unwrap_or((ValidationsAccess::Sandbox, None));
        let socket = if access == ValidationsAccess::Docker {
            Some(
                tokio::task::spawn_blocking(crate::core::task_worker_sandbox::host_docker_socket)
                    .await
                    .unwrap_or_else(|error| Err(error.to_string())),
            )
        } else {
            None
        };
        Self {
            access,
            socket,
            by_trust,
        }
    }

    fn launch(&self) -> ValidationReachLaunch {
        use crate::models::TrustApproval;
        use crate::models::ValidationsAccess;
        match (self.access, &self.socket, self.by_trust) {
            (ValidationsAccess::Full, _, Some(TrustApproval::ValidationsUnconfined)) => {
                ValidationReachLaunch::TrustHost
            }
            (ValidationsAccess::Full, _, _) => ValidationReachLaunch::Host,
            (ValidationsAccess::Docker, Some(Ok(path)), Some(TrustApproval::ValidationsDocker)) => {
                ValidationReachLaunch::TrustDocker(path.clone())
            }
            (ValidationsAccess::Docker, Some(Ok(path)), _) => {
                ValidationReachLaunch::Docker(path.clone())
            }
            _ => ValidationReachLaunch::Sandbox,
        }
    }

    fn environment_cause(&self, failure: &ValidationFailure) -> Option<EnvironmentCause> {
        environment_cause(self.access, self.socket.as_ref(), failure)
    }
}

/// How many of the last non-empty output lines may carry Docker's refusal.
const DOCKER_REFUSAL_TAIL_LINES: usize = 6;

/// How the Docker CLI says it could not reach its daemon (lower-cased).
const DOCKER_REFUSALS: &[&str] = &[
    "permission denied while trying to connect to the docker api",
    "permission denied while trying to connect to the docker daemon",
    "failed to connect to the docker api",
    "cannot connect to the docker daemon",
];

/// Output of a test runner that ran and failed: the worker's to fix, even when
/// its log also mentions Docker (lower-cased).
const TEST_FAILURE_MARKERS: &[&str] = &[
    "test result: failed",
    "assertionerror",
    "tests failed",
    "failures:",
    "--- fail:",
    // PHPUnit's summary lines (failed assertions, errors).
    "failures!",
    "errors!",
];

/// A runner's own per-file failure line: vitest and jest print `FAIL path`.
fn reports_failed_file(output_lower: &str) -> bool {
    output_lower
        .lines()
        .any(|line| line.trim_start().starts_with("fail "))
}

/// KT-1753 review — did a test runner run and report a failure? The markers
/// above, vitest / jest's `FAIL path`, pytest's `FAILED|ERROR path::test` and
/// its `=== 1 failed in 0.1s ===` summary, mocha's `N failing` and jest's
/// `Tests: 1 failed` summary. `output` is as printed (pytest's prefixes are
/// upper-case), `lower` the same lower-cased.
fn test_runner_failed(output: &str, lower: &str) -> bool {
    TEST_FAILURE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
        || reports_failed_file(lower)
        || output.lines().any(|line| {
            let line = line.trim_start();
            (line.starts_with("FAILED ") || line.starts_with("ERROR ")) && line.contains("::")
        })
        || lower.lines().any(|line| {
            let line = line.trim();
            let pytest_summary = line.starts_with('=')
                && line.contains(" in ")
                && (line.contains(" failed") || line.contains(" error"));
            let mocha = line.strip_suffix(" failing").is_some_and(|count| {
                !count.is_empty() && count.chars().all(|c| c.is_ascii_digit())
            });
            let jest = line.starts_with("tests:") && line.contains(" failed");
            pytest_summary || mocha || jest
        })
        || unittest_failed(lower)
}

/// Python unittest / Django: `Ran N test(s) in …`, then `FAILED (errors=1)`.
fn unittest_failed(lower: &str) -> bool {
    let mut ran = false;
    lower.lines().any(|line| {
        let line = line.trim();
        if line
            .strip_prefix("ran ")
            .and_then(|rest| rest.split_whitespace().next())
            .is_some_and(|count| !count.is_empty() && count.chars().all(|c| c.is_ascii_digit()))
            && line.contains(" test")
        {
            ran = true;
        }
        ran && line.starts_with("failed (")
    })
}

/// `Some` when `failure` comes from the host rather than the worker's code:
/// Docker unreachable, or a launch the sandbox refused. `None` means the
/// command ran and failed: the worker's to fix.
pub fn environment_cause(
    access: crate::models::ValidationsAccess,
    socket: Option<&Result<std::path::PathBuf, String>>,
    failure: &ValidationFailure,
) -> Option<EnvironmentCause> {
    use crate::models::ValidationsAccess;
    let output = failure.output.to_ascii_lowercase();
    if failure.exit_code.is_none() && output.trim_start().starts_with("refused:") {
        return Some(EnvironmentCause {
            fix: format!(
                "Kronn n'a pas pu lancer la validation : {}. Corrige ce point sur cette machine, puis reprends l'intégration.",
                first_line(&failure.output)
            ),
            docker_elevation: false,
        });
    }
    // The Docker CLI's own refusal, as the last thing the command printed: a
    // test log that merely quotes it, then fails on an assertion, stays red.
    let tail: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .rev()
        .take(DOCKER_REFUSAL_TAIL_LINES)
        .collect();
    let test_failed = test_runner_failed(&failure.output, &output);
    let Some(refusal) = tail
        .iter()
        .find(|line| DOCKER_REFUSALS.iter().any(|refusal| line.contains(refusal)))
    else {
        return lane_environment_cause(failure.exit_code, &tail, test_failed);
    };
    if test_failed {
        return None;
    }
    let denied =
        refusal.contains("permission denied") || refusal.contains("operation not permitted");
    Some(match (access, socket) {
        (ValidationsAccess::Sandbox, _) if denied => EnvironmentCause {
            fix: "Le bac à sable des validations garde Docker fermé pour ce projet. Autorise Docker sur la carte d'élévation (pour cette discussion ou pour le projet) : l'intégration reprend seule.".to_string(),
            docker_elevation: true,
        },
        (ValidationsAccess::Docker, Some(Ok(socket))) if denied => EnvironmentCause {
            fix: format!(
                "Docker est ouvert aux validations, mais le socket {} a refusé la connexion. Vérifie qu'il appartient à ton utilisateur (redémarre Docker au besoin), puis reprends l'intégration.",
                socket.display()
            ),
            docker_elevation: false,
        },
        (ValidationsAccess::Docker, Some(Err(reason))) => EnvironmentCause {
            fix: format!(
                "Docker est ouvert aux validations, mais aucun socket n'est utilisable : {reason}. Démarre Docker, puis reprends l'intégration."
            ),
            docker_elevation: false,
        },
        _ => EnvironmentCause {
            fix: "Docker ne répond pas. Démarre-le (OrbStack, Docker Desktop ou Colima), vérifie `docker info`, puis reprends l'intégration.".to_string(),
            docker_elevation: false,
        },
    })
}

/// What the OS sandbox of a lane prints when it refuses an operation (lower-cased).
const SANDBOX_REFUSALS: &[&str] = &[
    "operation not permitted",
    "deny file-write",
    "read-only file system",
];

/// What a tool prints when the network is not there (lower-cased): the lane's
/// sandbox opens no outbound connection.
const OFFLINE_MARKERS: &[&str] = &[
    "could not resolve host",
    "temporary failure in name resolution",
    "getaddrinfo eai_again",
    "getaddrinfo enotfound",
    "failed to lookup address information",
    "name or service not known",
    "network is unreachable",
];

/// What `docker compose exec` / `run` prints when the service it targets is not
/// up (lower-cased).
const COMPOSE_MARKERS: &[&str] = &[
    "no such service",
    "no such container",
    "no configuration file provided",
];

/// The programs whose own `<name>: …` line reports a command they could not
/// find: the shells, and `env` of a `#!/usr/bin/env tool` script.
const NOT_FOUND_REPORTERS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "fish", "env"];

/// KT-1753 review — a shell's own report of a missing command, by its shape
/// (lower-cased line): bash `bash: line 1: x: command not found`, zsh
/// `zsh: command not found: x`, dash `/bin/sh: 1: x: not found`, env
/// `env: 'x': no such file or directory`. A test's own `Not Found` (an HTTP
/// 404, a missing route) is not one: its line does not start with a shell.
fn shell_reported_not_found(line: &str) -> bool {
    let Some((head, rest)) = line.split_once(':') else {
        return false;
    };
    let program = head.rsplit('/').next().unwrap_or(head).trim();
    if !NOT_FOUND_REPORTERS.contains(&program) {
        return false;
    }
    let rest = rest.trim_end();
    rest.contains("command not found")
        || rest.ends_with(": not found")
        || (program == "env" && rest.ends_with("no such file or directory"))
}

/// GNU make's own report of a recipe that exited 127 (`make: *** [t] Error 127`),
/// not `Error 1270`.
fn make_reported_127(line: &str) -> bool {
    line.match_indices("error 127").any(|(at, found)| {
        !line[at + found.len()..]
            .chars()
            .next()
            .is_some_and(|next| next.is_ascii_digit())
    })
}

/// KT-1753 — a failure the lane's environment caused, for any project: the
/// command is not installed (exit 127), the OS sandbox refused an operation, the
/// network is not there, or a compose service is down. Judged from the last
/// lines of the output and never when a test runner reported a failure, so a
/// log that merely quotes such a message and then fails on an assertion stays
/// the worker's. `tail` is the last non-empty output lines, lower-cased, last
/// first.
fn lane_environment_cause(
    exit_code: Option<i32>,
    tail: &[&str],
    test_failed: bool,
) -> Option<EnvironmentCause> {
    if test_failed {
        return None;
    }
    let any = |markers: &[&str]| {
        tail.iter()
            .any(|line| markers.iter().any(|marker| line.contains(marker)))
    };
    // A shell exits 127 for a command it cannot find; GNU make exits 2 and says
    // `Error 127`, and a runner may exit 1 after `command not found`.
    let missing_tool = exit_code == Some(127)
        || tail
            .iter()
            .any(|line| shell_reported_not_found(line) || make_reported_127(line));
    let fix = if missing_tool {
        "La commande de la validation (code 127) n'existe pas sur cette machine ou dans le PATH de la lane : installe l'outil (ou donne-le à la lane), puis reprends l'intégration."
    } else if any(SANDBOX_REFUSALS) {
        "Le bac à sable de la lane a refusé une opération (écriture hors de ses racines, par exemple). Ce n'est pas une erreur du code livré : ouvre ce chemin au profil du projet ou lance la validation sur l'hôte, puis reprends l'intégration."
    } else if any(OFFLINE_MARKERS) {
        "La validation a besoin du réseau, que la lane n'a pas (dépendances à récupérer, hôte à résoudre). Mets les dépendances en cache ou dans le dépôt, ou lance la validation sur l'hôte, puis reprends l'intégration."
    } else if any(COMPOSE_MARKERS)
        || tail
            .iter()
            .any(|line| line.contains("service \"") && line.contains("is not running"))
    {
        "La validation exécute une commande dans un service compose qui n'est pas démarré. Démarre la pile compose du projet, puis reprends l'intégration."
    } else {
        return None;
    };
    Some(EnvironmentCause {
        fix: fix.to_string(),
        docker_elevation: false,
    })
}

/// Ask the human, on a card Kronn posts, to open Docker to the validations of
/// this execution's discussion or project. Best effort: the refusal message
/// already names the fix.
async fn post_docker_elevation(db: &Database, exec_id: &str) {
    let id = exec_id.to_string();
    if let Err(error) = db
        .with_conn(move |conn| crate::db::orchestration::post_docker_elevation_card(conn, &id))
        .await
    {
        tracing::warn!(execution_id = %exec_id, %error, "Docker elevation card not posted");
    }
}

/// The first non-empty line of `text`, bounded.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    line.chars().take(240).collect()
}

/// The validation that went red on a candidate, as the worker and the principal
/// need it: the command, how it ended and what it printed.
#[derive(Debug, PartialEq, Eq)]
pub struct ValidationFailure {
    pub command: String,
    pub exit_code: Option<i32>,
    pub output: String,
}

/// Execute only validations without an already-persisted success for this
/// candidate. This makes a resumed `Validating` checkpoint idempotent even if
/// the preceding process stopped between two validation commands.
pub async fn run_pending_validations(
    db: &Database,
    exec_id: &str,
    validations: &[crate::models::ValidationSpec],
    child: &std::path::Path,
    merge_sha: &str,
    reach: ValidationReachLaunch,
) -> Result<Option<ValidationFailure>, ProvisionError> {
    for spec in validations {
        let id = exec_id.to_string();
        let candidate = merge_sha.to_string();
        let spec_for_lookup = spec.clone();
        let already_passed = db
            .with_conn(move |conn| {
                crate::db::orchestration::has_passing_validation_run(
                    conn,
                    &id,
                    &candidate,
                    &spec_for_lookup,
                )
            })
            .await
            .map_err(|error| ProvisionError::Internal(error.to_string()))?;
        if already_passed {
            continue;
        }

        // The validation builds in the worker's lane whatever the worker kind
        // is (a Codex or a joined CLI worker never got the Claude launch's
        // stamp): the lane it creates here is Kronn's, so it is marked first.
        mark_lane_target_for_worker(exec_id, child).await;
        let (code, duration, summary) =
            run_one_validation_reaching(spec, child, reach.clone()).await;
        let id = exec_id.to_string();
        let spec = spec.clone();
        let command = spec.command.clone();
        let candidate = merge_sha.to_string();
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
            .map_err(|error| ProvisionError::Internal(error.to_string()))?;
        if !recorded.passed() {
            return Ok(Some(ValidationFailure {
                command,
                exit_code: recorded.exit_code,
                output: recorded.output.unwrap_or_default(),
            }));
        }
    }
    Ok(None)
}

/// Run one declared validation inside the child worktree, through the same
/// allowlist every other Kronn-run command goes through.
///
/// The tree is the worker's: its `build.rs`, proc-macros, tests and
/// `.cargo/config.toml` are code. Every command of the validation — Cargo's
/// target resolution included — therefore runs under the lane's sandbox
/// profile (KT-1268), and a host that has none refuses the validation.
#[cfg(any(test, feature = "test-support"))]
pub async fn run_one_validation(
    spec: &crate::models::ValidationSpec,
    cwd: &std::path::Path,
) -> (Option<i32>, i64, String) {
    run_one_validation_reaching(spec, cwd, ValidationReachLaunch::Sandbox).await
}

/// [`run_one_validation`] at the reach a human gave the run's project or
/// discussion (KT-1433): the sandbox, the sandbox plus the Docker socket, or
/// the backend's own rights.
async fn run_one_validation_reaching(
    spec: &crate::models::ValidationSpec,
    cwd: &std::path::Path,
    reach: ValidationReachLaunch,
) -> (Option<i32>, i64, String) {
    // Probing the launcher and provisioning the lane start a process and touch
    // the disk: off the async worker.
    let launch_cwd = cwd.to_path_buf();
    let reach_for_guard = reach.clone();
    let launch = tokio::task::spawn_blocking(move || launch_for_reach(reach, &launch_cwd))
        .await
        .unwrap_or_else(|error| Err(format!("the sandbox check did not complete: {error}")));
    if matches!(&reach_for_guard, ValidationReachLaunch::TrustDocker(_))
        && crate::core::trust_sandbox::protect_vault_setting()
    {
        // A Docker volume naming Kronn's data directory (or a parent) would hand
        // the daemon's root view of it to a container: refused before it runs.
        let argv: Vec<String> = spec.command.split_whitespace().map(String::from).collect();
        if let Some(arg) = crate::core::trust_sandbox::argv_reaches_protected(
            &argv,
            &crate::core::trust_sandbox::protected_dirs(),
            cwd,
        ) {
            return (
                None,
                0,
                format!(
                    "refused: `{arg}` would mount Kronn's data directory (or a directory containing it) \
                     into a container; a validation a trust level reached Docker for never does"
                ),
            );
        }
    }
    match launch {
        Ok(launch) => run_one_validation_with(spec, cwd, &launch).await,
        // Recorded like any failed validation: recovery cannot advance the
        // candidate as though the command had run.
        Err(reason) => (None, 0, format!("refused: {reason}")),
    }
}

/// How a validation is started at `reach`. A reach a trust level opened is
/// never started unconfined: it runs under the level's sandbox (Full access) or
/// keeps the lane sandbox with Kronn's data directory closed (Docker), and a
/// host with no sandbox refuses it with the reason.
fn launch_for_reach(
    reach: ValidationReachLaunch,
    cwd: &std::path::Path,
) -> Result<ValidationLaunch, String> {
    match reach {
        ValidationReachLaunch::Host => Ok(ValidationLaunch::Unconfined),
        ValidationReachLaunch::Docker(socket) => {
            validation_launch(cwd).map(|launch| launch.reaching(socket))
        }
        // As the user. Under the vault sandbox when the host has one and the
        // owner kept the protection on, otherwise with the backend's rights.
        ValidationReachLaunch::TrustHost => Ok(crate::core::trust_sandbox::confinement()
            .map_or(ValidationLaunch::Unconfined, ValidationLaunch::Level)),
        ValidationReachLaunch::TrustDocker(socket) => validation_launch(cwd)
            .map(|launch| launch.reaching(socket))
            .map(ValidationLaunch::hiding_kronn),
        ValidationReachLaunch::Sandbox => validation_launch(cwd),
    }
}

async fn run_one_validation_with(
    spec: &crate::models::ValidationSpec,
    cwd: &std::path::Path,
    launch: &ValidationLaunch,
) -> (Option<i32>, i64, String) {
    let build_path = match validation_build_path(spec, cwd, launch).await {
        Ok(path) => path,
        Err(reason) => return (None, 0, format!("refused: {reason}")),
    };
    run_one_validation_checked(
        spec,
        cwd,
        &build_path,
        launch,
        worktree::ensure_build_disk_headroom,
    )
    .await
}

/// How the commands of one validation are started (KT-1268).
pub enum ValidationLaunch {
    /// Under the lane's sandbox.
    Confined(crate::core::task_worker_sandbox::Confinement),
    /// With the backend's rights: a project whose human chose full access
    /// (KT-1433), and tests that validate a scripted command in a temporary
    /// directory that is not a lane.
    Unconfined,
    /// KT-1492: as the user, under the sandbox of what a trust level grants
    /// (Kronn's data directory closed, the rest open).
    Level(crate::core::trust_sandbox::LevelConfinement),
}

impl ValidationLaunch {
    /// This launch with the Docker socket reachable, when one is given.
    fn reaching(self, socket: std::path::PathBuf) -> Self {
        match self {
            Self::Confined(confinement) => Self::Confined(confinement.with_docker_socket(socket)),
            launch => launch,
        }
    }

    /// KT-1492: this launch with Kronn's data directory closed to it, unless
    /// the owner turned "Protect the Kronn vault from agents" off. A launch with
    /// no sandbox at all has nothing to hide it with and is left as it is.
    fn hiding_kronn(self) -> Self {
        match self {
            Self::Confined(confinement) if crate::core::trust_sandbox::protect_vault_setting() => {
                Self::Confined(confinement.hiding(crate::core::trust_sandbox::protected_dirs()))
            }
            launch => launch,
        }
    }

    pub fn confinement(&self) -> Option<&dyn crate::core::task_worker_sandbox::CommandConfinement> {
        match self {
            Self::Confined(confinement) => Some(confinement),
            Self::Level(level) => Some(level),
            Self::Unconfined => None,
        }
    }
}

/// The confinement of the tree `cwd` on this host, or why none exists: the
/// shipped launch of a validation, `Confined` or a refusal, never anything else.
pub fn host_validation_launch(cwd: &std::path::Path) -> Result<ValidationLaunch, String> {
    crate::core::task_worker_sandbox::Confinement::for_worktree(cwd).map(ValidationLaunch::Confined)
}

pub fn validation_launch(cwd: &std::path::Path) -> Result<ValidationLaunch, String> {
    #[cfg(any(test, feature = "test-support"))]
    if crate::core::test_mode::on() {
        return test_validation_launch(cwd);
    }
    host_validation_launch(cwd)
}

/// Tests choose, per directory, the mechanism the host "has"
/// (`Mechanism::of_host()` for the real one, `None` for a host without any);
/// a directory they did not register runs unconfined.
#[cfg(any(test, feature = "test-support"))]
fn test_validation_launch(cwd: &std::path::Path) -> Result<ValidationLaunch, String> {
    let registered = test_validation_mechanisms()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(cwd)
        .copied();
    match registered {
        Some(mechanism) => {
            crate::core::task_worker_sandbox::Confinement::with_mechanism(cwd, mechanism)
                .map(ValidationLaunch::Confined)
        }
        None => Ok(ValidationLaunch::Unconfined),
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn test_validation_mechanisms() -> &'static std::sync::Mutex<
    std::collections::HashMap<
        std::path::PathBuf,
        Option<crate::core::task_worker_sandbox::Mechanism>,
    >,
> {
    static MECHANISMS: std::sync::OnceLock<
        std::sync::Mutex<
            std::collections::HashMap<
                std::path::PathBuf,
                Option<crate::core::task_worker_sandbox::Mechanism>,
            >,
        >,
    > = std::sync::OnceLock::new();
    MECHANISMS.get_or_init(Default::default)
}

/// Resolve Cargo's own effective target directory before a validation build.
///
/// `cargo metadata` does not compile a crate, but it is the authority for
/// workspace and configuration layering.  Checking `cwd` would be wrong when
/// a project routes its target to another volume; checking an inferred
/// `target/` would be wrong for an explicit `--target-dir`.  Non-Cargo
/// validations do not create Cargo artefacts, so their declared working
/// directory remains the measured filesystem.
pub async fn validation_build_path(
    spec: &crate::models::ValidationSpec,
    cwd: &std::path::Path,
    launch: &ValidationLaunch,
) -> Result<std::path::PathBuf, String> {
    const METADATA_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
    let Some(words) = validation_cargo_argv(&spec.command)? else {
        return Ok(cwd.to_path_buf());
    };

    let mut metadata = validation_metadata_command(&words, cwd, launch)?;
    let mut child = metadata.spawn().map_err(|error| {
        format!(
            "cannot resolve Cargo target for validation in {}: {error}",
            cwd.display()
        )
    })?;
    let output =
        wait_for_metadata_output(&mut child, METADATA_TIMEOUT, launch.confinement().is_some())
            .await
            .map_err(|reason| {
                format!(
                    "cannot resolve Cargo target for validation in {}: {reason}",
                    cwd.display(),
                )
            })?;
    if !output.status.success() {
        return Err(format!(
            "cannot resolve Cargo target for validation in {}: {}",
            cwd.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "Cargo returned invalid metadata for validation in {}: {error}",
            cwd.display()
        )
    })?;
    let target = metadata
        .get("target_directory")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            format!(
                "Cargo metadata has no target_directory for {}",
                cwd.display()
            )
        })?;
    Ok(std::path::PathBuf::from(target))
}

/// The `cargo metadata` process [`validation_build_path`] starts for a
/// validation's Cargo argv (`words`, without `cargo`).
///
/// `cargo metadata` runs `rustc -vV` through the worker's own
/// `.cargo/config.toml` (`build.rustc-wrapper`): it is worker-written code like
/// the validation, and starts under the same confinement (KT-1268).
pub fn validation_metadata_command(
    words: &[&str],
    cwd: &std::path::Path,
    launch: &ValidationLaunch,
) -> Result<tokio::process::Command, String> {
    use crate::core::task_worker_env::ProbeCommand;
    // Cargo reads the repository's `.cargo/config.toml`: the Quick Exec
    // environment, as for the validation itself (KT-1006).
    let route = crate::core::child_env::ChildRoute::QuickExec;
    let mut probe = ProbeCommand {
        program: "cargo".to_string(),
        // Resolution must neither fetch dependencies nor wait forever before a
        // validation can be refused. `metadata` only needs the local manifest
        // graph to return Cargo's effective target directory.
        args: [
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--offline",
        ]
        .map(String::from)
        .to_vec(),
        cwd: cwd.to_path_buf(),
        env: Vec::new(),
        timeout: std::time::Duration::from_secs(15),
        // A bounded target-dir lookup that gates a validation's refusal.
        priority: crate::core::background_priority::Priority::Normal,
        clean_env: false,
    };
    // Preserve the validation's manifest and configuration options. Validations
    // are normally launched from the repository root, including commands such
    // as `cargo test --manifest-path backend/Cargo.toml`; resolving metadata in
    // cwd alone would reject that normal form before Quick Exec can run it.
    // `cargo metadata` has no `--target-dir`, so its equivalent environment
    // override retains the effective output directory for that one invocation.
    // Cargo's validation subcommand is ignored; only its global resolution
    // options are forwarded to metadata, wherever the literal argv placed them.
    // A later `--target-dir` replaces an earlier one, as it does for Cargo.
    fn set_target_dir(probe: &mut ProbeCommand, value: &str) {
        probe.env.retain(|(name, _)| name != "CARGO_TARGET_DIR");
        probe
            .env
            .push(("CARGO_TARGET_DIR".to_string(), value.into()));
    }
    let mut index = 0;
    while index < words.len() {
        let word = words[index];
        if word == "--target-dir" {
            let value = words.get(index + 1).ok_or_else(|| {
                "validation cargo command has --target-dir without a directory".to_string()
            })?;
            set_target_dir(&mut probe, value);
            index += 2;
            continue;
        } else if let Some(value) = word.strip_prefix("--target-dir=") {
            set_target_dir(&mut probe, value);
        } else if word == "--manifest-path" || word == "--config" {
            let value = words
                .get(index + 1)
                .ok_or_else(|| format!("validation cargo command has {word} without a value"))?;
            probe.args.extend([word.to_string(), value.to_string()]);
            index += 2;
            continue;
        } else if let Some(value) = word.strip_prefix("--manifest-path=") {
            probe
                .args
                .extend(["--manifest-path".to_string(), value.to_string()]);
        } else if let Some(value) = word.strip_prefix("--config=") {
            probe
                .args
                .extend(["--config".to_string(), value.to_string()]);
        }
        index += 1;
    }
    let mut metadata = match launch.confinement() {
        Some(confinement) => confinement.command(&probe, route),
        None => {
            let mut command = crate::core::cmd::async_cmd(&probe.program, route);
            command.args(&probe.args).current_dir(&probe.cwd);
            for (name, value) in &probe.env {
                command.env(name, value);
            }
            command
        }
    };
    crate::core::child_env::seal(metadata.as_std_mut(), route, &[]);
    metadata
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // `wait_for_metadata_output` owns timeout cleanup and reaps the child.
        .kill_on_drop(true);
    Ok(metadata)
}

/// Return Cargo's literal argv for the forms Quick Exec already launches.
///
/// `ValidationSpec::command` is deliberately split exactly as the runner splits
/// it below; this is not a shell parser and it does not make shell syntax valid.
pub fn validation_cargo_argv(command: &str) -> Result<Option<Vec<&str>>, String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let cargo = match words.as_slice() {
        ["cargo", rest @ ..] => rest,
        ["rtk", "cargo", rest @ ..] => rest,
        ["rtk", "proxy", "cargo", rest @ ..] => rest,
        _ => return Ok(None),
    };
    if cargo.is_empty() {
        return Err("validation cargo command has no Cargo arguments".into());
    }
    Ok(Some(cargo.to_vec()))
}

/// Collect the output of the metadata `child` within `timeout`.
///
/// `leads_group` says the child leads a process group of its own (a confined
/// launch, `Confinement::command`): a timeout or a read error then takes the
/// whole group down, as `quick_exec::run_confined` does, so a grandchild of
/// the worker's `rustc` wrapper cannot outlive the resolution.
pub async fn wait_for_metadata_output(
    child: &mut tokio::process::Child,
    timeout: std::time::Duration,
    leads_group: bool,
) -> Result<std::process::Output, String> {
    use tokio::io::AsyncReadExt;

    #[cfg(unix)]
    let group = child.id().filter(|_| leads_group);
    #[cfg(not(unix))]
    let _ = leads_group;
    let kill = |child: &mut tokio::process::Child| {
        #[cfg(unix)]
        if let Some(group) = group {
            crate::core::quick_exec::kill_process_group(group);
        }
        // The leader is killed on its own as well: a child that leads no
        // group, or a host without process groups.
        child.start_kill()
    };
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let captured = {
        let output = async {
            let read_stdout = async {
                let mut bytes = Vec::new();
                if let Some(pipe) = stdout.as_mut() {
                    pipe.read_to_end(&mut bytes).await?;
                }
                Ok::<_, std::io::Error>(bytes)
            };
            let read_stderr = async {
                let mut bytes = Vec::new();
                if let Some(pipe) = stderr.as_mut() {
                    pipe.read_to_end(&mut bytes).await?;
                }
                Ok::<_, std::io::Error>(bytes)
            };
            let (status, stdout, stderr) =
                tokio::try_join!(child.wait(), read_stdout, read_stderr)?;
            Ok::<_, std::io::Error>(std::process::Output {
                status,
                stdout,
                stderr,
            })
        };
        tokio::time::timeout(timeout, output).await
    };
    match captured {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => {
            let _ = kill(child);
            let _ = child.wait().await;
            Err(error.to_string())
        }
        Err(_) => {
            // Waiting for collection, so a timed-out metadata process cannot
            // survive and contend with the following validation.
            let _ = kill(child);
            let _ = child.wait().await;
            Err(format!(
                "Cargo target resolution timed out after {} seconds",
                timeout.as_secs()
            ))
        }
    }
}

pub async fn run_one_validation_checked(
    spec: &crate::models::ValidationSpec,
    cwd: &std::path::Path,
    build_path: &std::path::Path,
    launch: &ValidationLaunch,
    check_headroom: impl FnOnce(&std::path::Path) -> Result<(), String>,
) -> (Option<i32>, i64, String) {
    run_one_validation_checked_with_runner(
        spec,
        cwd,
        build_path,
        check_headroom,
        |validated| async move {
            let cancel = tokio_util::sync::CancellationToken::new();
            match launch.confinement() {
                Some(confinement) => {
                    crate::core::quick_exec::run_confined(
                        &validated,
                        None,
                        &cancel,
                        confinement,
                        crate::core::background_priority::Priority::Background,
                    )
                    .await
                }
                None => crate::core::quick_exec::run_background(&validated, None, &cancel).await,
            }
        },
    )
    .await
}

/// The runner seam is deliberately narrower than the validation contract: it
/// receives only an already-validated literal argv. Production always calls
/// Quick Exec above; tests use this seam to prove a headroom refusal cannot
/// reach the spawn operation without depending on timing or a real full disk.
pub async fn run_one_validation_checked_with_runner<F, Fut>(
    spec: &crate::models::ValidationSpec,
    cwd: &std::path::Path,
    build_path: &std::path::Path,
    check_headroom: impl FnOnce(&std::path::Path) -> Result<(), String>,
    run: F,
) -> (Option<i32>, i64, String)
where
    F: FnOnce(crate::core::quick_exec::ValidatedSpec) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<crate::core::quick_exec::QuickExecResult>>,
{
    let mut parts = spec.command.split_whitespace();
    let Some(binary) = parts.next() else {
        return (None, 0, "empty validation command".into());
    };
    if let Err(reason) = check_headroom(build_path) {
        // A low-space refusal is recorded like every other validation failure,
        // so recovery cannot advance the candidate as though the command ran.
        return (None, 0, format!("refused: {reason}"));
    }
    use crate::core::quick_exec;
    let requested_timeout = spec.timeout_secs.map(u64::from);
    let effective_timeout =
        requested_timeout.map(|timeout| timeout.min(quick_exec::MAX_TIMEOUT_SECS));
    let legacy_timeout_notice = requested_timeout
        .filter(|timeout| *timeout > quick_exec::MAX_TIMEOUT_SECS)
        .map(|timeout| {
            format!(
                "legacy validation timeout {timeout}s clamped to {}s",
                quick_exec::MAX_TIMEOUT_SECS
            )
        });
    let quick = quick_exec::QuickExecSpec {
        binary: binary.to_string(),
        argv: parts.map(str::to_string).collect(),
        cwd: cwd.to_path_buf(),
        timeout_secs: effective_timeout,
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
    // KT-1751: what the project of this tree lists for its validations.
    let project_executables =
        crate::core::task_worker_lane_settings::validation_executables_for(cwd);
    let validated =
        match quick_exec::validate_with(&quick, &[cwd.to_path_buf()], &project_executables) {
            Ok(v) => v,
            // A refused command is a failed validation, never a silent skip.
            Err(rejection) => return (None, 0, format!("refused: {rejection}")),
        };
    let (code, duration, summary) = match run(validated).await {
        Ok(result) => (result.exit_code, result.duration_ms as i64, result.summary),
        Err(e) => (None, 0, format!("validation could not run: {e}")),
    };
    let summary = match legacy_timeout_notice {
        Some(notice) => format!("{notice}\n{summary}"),
        None => summary,
    };
    (code, duration, summary)
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
                        crate::db::discussions::room_lang(&tx, &origin),
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

/// The verdict of a CLI worker accepting its control offer without leaving its
/// principal discussion.
#[derive(Debug)]
// Outcome enum: the success payload travels inline rather than boxed, so the
// nominal path pays no allocation.
#[allow(clippy::large_enum_variant)]
pub enum AcceptAttachOutcome {
    /// The exact target session accepted. Worker authority is carried by the execution's
    /// pinned session id while the session's durable binding and live membership stay in
    /// the principal room. The child remains the durable task/evidence room.
    Attached {
        child_discussion_id: String,
        execution: TaskExecution,
        worker_instructions: String,
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

/// Accept a CLI worker control offer without transferring the caller's room ownership.
/// Both identities are derived by the trusted bridge:
/// the live `(source_agent, source_session_id)` resolves the exact target session,
/// while `source_binding_session_id` names its separate reload-stable room binding.
/// The model supplies neither. Two durable steps are idempotent:
///   1. stage the accept (CAS `pending → accepting`, commit 1) — reversible, no external
///      effect yet;
///   2. verify that the execution still owns the exact session, then commit the brief in
///      the child, `Working`, task `InProgress`, offer `accepted`, and an activation notice
///      in the origin. The brief content is also returned directly to the accepting CLI.
///
/// Keeping room ownership unchanged is what lets a CLI principal continue polling its
/// room and authorizing the other executions it coordinates while this execution grants
/// it the additional worker role.
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

    // ── Rework re-accept fast path (KT-319 tranche 3b, DoD-9). A re-offer advances the
    // attempt number; the task is already `InProgress`. Route straight to the rework checkpoint
    // (`Blocked → Provisioning → Working` + settle the offer), skipping the provisioning
    // task-CAS. This works for both legacy child-bound workers and principal-room workers.
    // Idempotent: a resumed already-`accepted` offer converges here
    // to Attached. ──
    if offer.attempt_no > 0 && offer.reason.as_deref() != Some("cli_reassignment") {
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
                let (eid, child_id) = (exec_id.clone(), child.clone());
                let (execution, worker_instructions) = db
                    .with_conn(move |conn| {
                        let execution = crate::db::orchestration::get_task_execution(conn, &eid)?
                            .context("execution vanished right after the rework checkpoint")?;
                        let instructions = conn
                            .query_row(
                                "SELECT m.content FROM messages m \
                                 JOIN message_targets mt ON mt.message_id = m.id \
                                 WHERE m.discussion_id = ?1 AND mt.target_kind = 'cli' \
                                   AND mt.cli_session_id = ?2 \
                                 ORDER BY m.sort_order DESC LIMIT 1",
                                rusqlite::params![child_id, session_pk],
                                |row| row.get::<_, String>(0),
                            )
                            .optional()?
                            .unwrap_or_else(|| {
                                "Rework accepted. Read the execution detail and continue the current attempt."
                                    .to_string()
                            });
                        Ok((execution, instructions))
                    })
                    .await
                    .map_err(|e| ProvisionError::Internal(e.to_string()))?;
                Ok(AcceptAttachOutcome::Attached {
                    child_discussion_id: child,
                    execution,
                    worker_instructions,
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

    // ── 2. Revalidate the execution-scoped worker grant. Acceptance must never move the
    // live session or its reload-stable source binding: the caller may simultaneously be
    // the principal of `origin`, and that room ownership is what keeps its waits and other
    // principal actions alive. The exact `(execution, session_pk)` pair is the worker role.
    let execution_id = exec_id.clone();
    let agent = source_agent.to_string();
    db.with_conn(move |conn| {
        let current = crate::db::orchestration::get_task_execution(conn, &execution_id)?
            .context("worker acceptance execution vanished")?;
        if current.status.is_terminal()
            || current.worker_target_kind != Some(MessageTargetKind::Cli)
            || current.worker_cli_session_id != Some(session_pk)
            || current.worker_agent_type.as_deref() != Some(agent.as_str())
        {
            anyhow::bail!(
                "worker acceptance refused: execution no longer owns this CLI assignment"
            );
        }
        Ok(())
    })
    .await
    .map_err(|e| ProvisionError::Internal(e.to_string()))?;

    // CLI reassignment uses the same durable offer + exact execution/session grant as
    // initial provisioning, but the plan task is already InProgress. Settle a
    // bounded handoff without replaying the task CAS or reposting the original
    // brief.
    let reassignment = {
        let eid = exec_id.clone();
        db.with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &eid)?
                .context("execution vanished after reassignment acceptance")?;
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
        let generation = recovery.assignment_generation;
        let reason = &recovery.recovery_reason;
        let handoff = orchestrator_message(
            format!("orch-reassign-handoff:{exec_id}:{generation}"),
            match room_lang(db, &reassignment.0.parent_discussion_id).await {
                RoomLang::Fr => format!(
                    "**Réassignation acceptée — génération {generation}**\n\n\
                     Reprends exactement l'étape inachevée. Cette sous-discussion, son worktree, \
                     les manifests, constats, revues et SHA existants restent la source de vérité. \
                     Motif : {reason}"
                ),
                RoomLang::En => format!(
                    "**Reassignment accepted — generation {generation}**\n\n\
                     Resume exactly the unfinished step. This sub-discussion, its worktree, \
                     the existing manifests, findings, reviews and SHAs remain the source of \
                     truth. Reason: {reason}"
                ),
            },
        );
        let worker_instructions = handoff.content.clone();
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
                    worker_instructions,
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

    let lang = room_lang(db, &execution.parent_discussion_id).await;
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
        lang,
    );
    let worker_instructions = brief.content.clone();
    let notice = build_attach_notice(
        &exec_id,
        execution.attempt_no,
        &task.summary.reference,
        &task.summary.title,
        &child,
        alias.as_deref(),
        lang,
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
                worker_instructions,
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
    deliver_worker_manifest_as(
        db,
        task_execution_id,
        source_agent,
        source_session_id,
        manifest_json,
        None,
    )
    .await
}

/// [`deliver_worker_manifest`] for a caller that may hold a bridge token: the
/// worker session it names is its own only if its grant can be that worker's
/// (KT-1962).
pub async fn deliver_worker_manifest_as(
    db: &Database,
    task_execution_id: &str,
    source_agent: &str,
    source_session_id: &str,
    manifest_json: &str,
    bridge: Option<&crate::core::bridge_token::BridgeCaller>,
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
            Ok((exec, session.map(|s| (s.id, s.disc_id))))
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?
    };
    let (caller_session_id, session_room) = match caller_session_id {
        Some((id, room)) => (Some(id), room),
        None => (None, String::new()),
    };

    // ── 2. Authorize: caller must be this execution's exact CLI worker. An unknown
    // execution and a wrong caller FUSE into NotAddressed (anti-oracle). ──
    let Some(exec) = exec else {
        return Ok(DeliverOutcome::NotAddressed);
    };
    let authorized = matches!(
        (exec.worker_cli_session_id, caller_session_id),
        (Some(worker), Some(caller)) if worker == caller
    ) && bridge_may_be_execution_worker(bridge, &exec, &session_room);
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
        crate::db::orchestration::DeliveryDispatchExpectation::Unconstrained,
        manifest_json,
        DeliveryManifestAuthorship::Full,
    )
    .await
}

#[derive(Clone, Copy)]
pub enum DeliveryManifestAuthorship {
    Full,
    NativeProjection,
}

/// Trusted native identity assembled by Kronn's executor, never from model tool
/// arguments. The dispatch trigger pins a particular run even when two agents
/// of the same provider coexist in one room.
#[derive(Clone, Copy)]
pub struct NativeExecutionCaller<'a> {
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

/// A spawned CLI worker additionally carries the immutable dispatch id that
/// launched its process. A trigger alone is not sufficient after rework: a
/// current execution can have multiple historical dispatches with related
/// recovery lineage. Require the current attached dispatch before exposing a
/// commit, status, or delivery surface to the spawned process.
async fn spawned_native_worker_execution_for_caller(
    db: &Database,
    task_execution_id: &str,
    dispatch_job_id: &str,
    caller: NativeExecutionCaller<'_>,
    bridge: Option<&crate::core::bridge_token::BridgeCaller>,
) -> Result<Option<TaskExecution>, ProvisionError> {
    let Some(execution) = native_worker_execution_for_caller(db, task_execution_id, caller).await?
    else {
        return Ok(None);
    };
    // The dispatch and its message are readable in the room, and a spawned
    // worker's launch is always minted for its execution: any other grant,
    // one that runs in the worker's room included, is not this worker.
    Ok(
        (execution.dispatch_job_id.as_deref() == Some(dispatch_job_id)
            && bridge.is_none_or(|bridge| bridge.own_execution.as_deref() == Some(&*execution.id)))
        .then_some(execution),
    )
}

/// Native HTTP agents do not own a CLI session. Their trusted executor supplies the
/// current discussion, typed provider and dispatch trigger, so the backend can
/// authenticate the worker without accepting identity from model arguments.
pub async fn deliver_native_worker_manifest(
    db: &Database,
    task_execution_id: &str,
    caller: NativeExecutionCaller<'_>,
    expected_dispatch_job_id: &str,
    manifest_json: &str,
) -> Result<DeliverOutcome, ProvisionError> {
    let Some(execution) = spawned_native_worker_execution_for_caller(
        db,
        task_execution_id,
        expected_dispatch_job_id,
        caller,
        None,
    )
    .await?
    else {
        return Ok(DeliverOutcome::NotAddressed);
    };
    deliver_authorized_worker_manifest(
        db,
        execution,
        caller.alias,
        caller.actor_session_id.map(str::to_string),
        crate::db::orchestration::DeliveryDispatchExpectation::Exact(expected_dispatch_job_id),
        manifest_json,
        DeliveryManifestAuthorship::NativeProjection,
    )
    .await
}

/// KT-1402 — every refusal is journaled on the execution: the worker reads it
/// as a tool error inside its own turn, and the principal's status/escalation
/// must be able to say why nothing was delivered.
pub async fn deliver_authorized_worker_manifest(
    db: &Database,
    exec: TaskExecution,
    alias: &str,
    actor_session_id: Option<String>,
    dispatch_expectation: crate::db::orchestration::DeliveryDispatchExpectation<'_>,
    manifest_json: &str,
    authorship: DeliveryManifestAuthorship,
) -> Result<DeliverOutcome, ProvisionError> {
    let (exec_id, attempt_no) = (exec.id.clone(), exec.attempt_no);
    let actor = agent_actor(alias, actor_session_id.as_deref());
    let outcome = deliver_authorized_worker_manifest_inner(
        db,
        exec,
        alias,
        actor_session_id,
        dispatch_expectation,
        manifest_json,
        authorship,
    )
    .await?;
    if let DeliverOutcome::InvalidManifest(detail) = &outcome {
        let detail = detail.clone();
        let recorded = db
            .with_conn(move |conn| {
                crate::db::orchestration::record_delivery_refusal(
                    conn, &exec_id, attempt_no, &detail, &actor,
                )
            })
            .await;
        if let Err(error) = recorded {
            tracing::warn!("could not journal the delivery refusal: {error}");
        }
    }
    Ok(outcome)
}

async fn deliver_authorized_worker_manifest_inner(
    db: &Database,
    exec: TaskExecution,
    alias: &str,
    actor_session_id: Option<String>,
    dispatch_expectation: crate::db::orchestration::DeliveryDispatchExpectation<'_>,
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
    if let Some(dispatch_id) = exec.dispatch_job_id.clone() {
        db.with_conn(move |conn| {
            crate::db::agent_dispatch::mark_progress(conn, &dispatch_id, "delivering", None)?;
            Ok(())
        })
        .await
        .map_err(|error| ProvisionError::Internal(error.to_string()))?;
    }
    // ── 3. Resolve the task + the parent's principal AFTER authz. A stranger
    // still gets no task, manifest or worktree validation oracle. ──
    let (task, principal_target) = {
        let (tid, parent) = (exec.task_id.clone(), exec.parent_discussion_id.clone());
        let eid = exec.id.clone();
        db.with_conn(move |conn| {
            let task = crate::db::planning::get_task(conn, &tid)?
                .context("task vanished before delivery")?;
            let parent = crate::db::discussions::get_discussion(conn, &parent)?
                .context("parent discussion vanished before delivery")?;
            let target =
                crate::db::orchestration::principal_notice_target(conn, &eid, parent.agent)?;
            Ok((task, target))
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
                        format!(
                            "task Definition of Done changed since this execution was launched and this execution did not adopt it; the task now has {} item(s). Cancel it and launch a fresh execution (or reassign it, which adopts the current Definition of Done) before delivering ordered native assertions",
                            current_dod_ids.len()
                        ),
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
    if let Err(detail) = validate_manifest_claims(&task.definition_of_done, None, &manifest) {
        return Ok(DeliverOutcome::InvalidManifest(detail));
    }
    let git_validation = match git_facts.as_ref() {
        Some(facts) => validate_delivery_git_facts(facts, &manifest).await,
        None => validate_delivery_git_state(db, &exec, &manifest).await,
    };
    if let Err(detail) = git_validation {
        return Ok(DeliverOutcome::InvalidManifest(detail));
    }
    let child = exec.sub_discussion_id.clone().unwrap_or_default();
    let provider_changes = {
        let exec_id = exec.id.clone();
        db.with_read_conn(move |conn| crate::db::orchestration::provider_changes(conn, &exec_id))
            .await
            .unwrap_or_else(|error| {
                // The delivery itself must not fail on a missing annotation.
                tracing::warn!("provider changes unreadable for the review card: {error}");
                Vec::new()
            })
    };
    let lang = room_lang(db, &exec.parent_discussion_id).await;
    let mut review_request = build_review_request_message(
        &exec.id,
        exec.attempt_no,
        &task.summary.reference,
        &task.summary.title,
        &child,
        &manifest.head_sha,
        &manifest.summary,
        &provider_changes,
        lang,
    );
    if let Some(warning) = delivery_lint_gate_warning(&manifest, lang) {
        review_request.content.push_str("\n\n");
        review_request.content.push_str(&warning);
    }
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
        let expected_dispatch_job_id = match dispatch_expectation {
            crate::db::orchestration::DeliveryDispatchExpectation::Unconstrained => None,
            crate::db::orchestration::DeliveryDispatchExpectation::Exact(dispatch_job_id) => {
                Some(dispatch_job_id.to_string())
            }
        };
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
                    dispatch_expectation: match expected_dispatch_job_id.as_deref() {
                        Some(dispatch_job_id) => {
                            crate::db::orchestration::DeliveryDispatchExpectation::Exact(
                                dispatch_job_id,
                            )
                        }
                        None => {
                            crate::db::orchestration::DeliveryDispatchExpectation::Unconstrained
                        }
                    },
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
pub struct DeliveryGitFacts {
    repo: std::path::PathBuf,
    base_sha: String,
    head_sha: String,
    pub files_touched: Vec<ManifestFile>,
    /// Where "already upstream" is decided for this execution; `None` when the
    /// run names no project, and every commit is then the worker's.
    upstream: Option<UpstreamSource>,
}

/// What a delivery needs to tell the commits it merged in from upstream (KT-1498).
/// Read from the server's own records (run, project row), never from the
/// worker's worktree nor from the checkout's git config, which every worktree
/// shares.
#[derive(Debug, Clone)]
pub struct UpstreamSource {
    project_id: String,
    /// The project's `repo_url` in Kronn's database: the only remote asked.
    /// `None` when the project has none: every commit is then inspected.
    repo_url: Option<String>,
    /// The run's integration target branch, fetched from that remote when it
    /// has it (a PR base other than the default branch).
    target_branch: Option<String>,
}

/// Expand a native HTTP worker's semantic assertions into the same durable
/// DeliveryManifest v1 used by CLI workers. The model never copies opaque ids
/// or Git facts: their sole authority is the authorized task execution.
pub fn prepare_native_worker_manifest(
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
        "tests_executed",
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
    // KT-1246 — a spawned worker must say outright whether it ran tests.
    if !object.contains_key("tests_executed") {
        refusals.push(
            "native delivery must state `tests_executed` ({executed: bool, which: [commands]}); \
             use {executed: false, which: []} when no test ran"
                .to_string(),
        );
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
    validate_delivery_git_facts(&facts, manifest).await
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
    let upstream = execution_upstream_source(db, exec).await;
    delivery_git_facts_from_repo(repo, base_rev, upstream)
}

/// The project of the execution's run, with its `repo_url` and the run's
/// integration target; `None` when the run has no project.
async fn execution_upstream_source(db: &Database, exec: &TaskExecution) -> Option<UpstreamSource> {
    let run_id = exec.orchestration_run_id.clone();
    let (run, project) = db
        .with_conn(move |conn| {
            let Some(run) = crate::db::orchestration::get_orchestration_run(conn, &run_id)? else {
                return Ok((None, None));
            };
            let project = match run.project_id.as_deref() {
                Some(id) => crate::db::projects::get_project(conn, id)?,
                None => None,
            };
            Ok((Some(run), project))
        })
        .await
        .ok()?;
    let (run, project) = (run?, project?);
    Some(UpstreamSource {
        project_id: project.id,
        repo_url: project
            .repo_url
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty()),
        target_branch: run.target_branch,
    })
}

pub fn delivery_git_facts_from_repo(
    repo: std::path::PathBuf,
    base_rev: &str,
    upstream: Option<UpstreamSource>,
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
                // `T` (a file turned into a symlink, or back) is the same path
                // with other content: the manifest's closed model calls it modified.
                'M' | 'T' => FileChangeKind::Modified,
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
        upstream,
    })
}

async fn validate_delivery_git_facts(
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
    validate_committed_file_inventory(facts, manifest)?;
    validate_delivery_identity_trailers(facts).await
}

/// A CLI worker runs `git commit` itself, so nothing strips an identity
/// trailer the model wrote; every one must name the repository's git identity.
///
/// Commits the worker merged in from upstream (`origin/main`, the integration
/// target) are not its own: they are left out, but only once a foreign trailer
/// has been found among all the delivered commits, so a delivery without one
/// never waits on the remote.
async fn validate_delivery_identity_trailers(facts: &DeliveryGitFacts) -> Result<(), String> {
    let all = delivered_commit_messages(facts).await?;
    if foreign_identity_trailers(facts, &all)?.is_none() {
        return Ok(());
    }
    let own = commits_outside_upstream(facts, all).await?;
    let Some((identity, foreign)) = foreign_identity_trailers(facts, &own)? else {
        return Ok(());
    };
    Err(format!(
        "delivered commits carry identity trailers that do not match this repository's git identity `{identity}`: {}. \
         Remove these lines and let `git commit -s` add the sign-off; never write an identity trailer by hand \
         (for example `git reset --soft {}` then `git commit -s -m \"<message>\"`), then deliver the new HEAD.",
        foreign.join(", "),
        short_sha(&facts.base_sha),
    ))
}

/// Every commit of the delivery, read in a git directory of Kronn's own: the
/// shared repository's `shallow` file, grafts and replace refs are writable by
/// the worker and would hide commits from a scan run inside it.
async fn delivered_commit_messages(
    facts: &DeliveryGitFacts,
) -> Result<Vec<(String, String)>, String> {
    let data_dir = crate::core::config::config_dir()
        .map_err(|error| format!("cannot inspect the delivered commit messages: {error}"))?;
    let scan = crate::core::upstream_mirror::scan_dir(&data_dir);
    let (repo, base, head) = (
        facts.repo.clone(),
        facts.base_sha.clone(),
        facts.head_sha.clone(),
    );
    tokio::task::spawn_blocking(move || {
        crate::core::upstream_mirror::commit_messages_private(&repo, &scan, &base, &head)
    })
    .await
    .map_err(|error| format!("cannot inspect the delivered commit messages: {error}"))?
    .map_err(|error| format!("cannot inspect the delivered commit messages: {error}"))
}

/// `(repository identity, foreign trailers)` of `commits`; `None` when every
/// identity trailer names the repository's identity.
fn foreign_identity_trailers(
    facts: &DeliveryGitFacts,
    commits: &[(String, String)],
) -> Result<Option<(String, Vec<String>)>, String> {
    let trailers = commits
        .iter()
        .flat_map(|(sha, message)| {
            message.lines().filter_map(move |line| {
                crate::api::agent_workspace_tools::identity_trailer(line)
                    .map(|(key, value)| (sha.as_str(), key, value))
            })
        })
        .collect::<Vec<_>>();
    if trailers.is_empty() {
        return Ok(None);
    }
    // Either git's own identity (a worker that runs `git commit -s` itself) or
    // the project's commit identity (a commit Kronn made for it, KT-1750).
    let project = crate::core::commit_policy::CommitPolicy::for_repo(&facts.repo)
        .identity
        .display();
    let identity = match worktree::committer_identity(&facts.repo) {
        Ok(identity) => identity,
        Err(_) => project.clone(),
    };
    let foreign = trailers
        .into_iter()
        .filter(|(_, _, value)| {
            !same_git_identity(value, &identity) && !same_git_identity(value, &project)
        })
        .map(|(sha, key, value)| format!("{} `{key}: {value}`", short_sha(sha)))
        .collect::<Vec<_>>();
    Ok((!foreign.is_empty()).then_some((identity, foreign)))
}

/// The delivered commits (`all`) that are not already upstream (KT-1498).
///
/// "Upstream" is decided here, at delivery time, from the project's `repo_url`
/// in Kronn's database: its default branch (and the integration target, when
/// that remote has it) is fetched into a private mirror under the Kronn data
/// directory, and only the commits reachable from the fetched tips are left
/// out. No ref or config of the checkout is read (`refs/*` and `remote.*` are
/// shared by every worktree), and no local branch is an exemption. Without a
/// `repo_url`, or when the fetch fails (offline), every commit is returned:
/// fail closed.
async fn commits_outside_upstream(
    facts: &DeliveryGitFacts,
    all: Vec<(String, String)>,
) -> Result<Vec<(String, String)>, String> {
    let Some(source) = facts.upstream.as_ref() else {
        return Ok(all);
    };
    let (mirror, excluded) = match fetch_upstream_mirror(source).await {
        Ok(fetched) => fetched,
        Err(error) => {
            tracing::warn!(
                "delivery identity check: upstream not fetched, inspecting every commit: {error}"
            );
            return Ok(all);
        }
    };
    let (repo, base, head) = (
        facts.repo.clone(),
        facts.base_sha.clone(),
        facts.head_sha.clone(),
    );
    tokio::task::spawn_blocking(move || {
        crate::core::upstream_mirror::commit_messages_outside(
            &repo, &mirror, &base, &head, &excluded,
        )
    })
    .await
    .map_err(|error| format!("cannot inspect the delivered commit messages: {error}"))?
    .map_err(|error| format!("cannot inspect the delivered commit messages: {error}"))
}

/// Fetch the project's remote into its private mirror: `(mirror, tips)`.
async fn fetch_upstream_mirror(
    source: &UpstreamSource,
) -> Result<(std::path::PathBuf, Vec<String>), String> {
    let url = source
        .repo_url
        .clone()
        .ok_or_else(|| "the project has no repo_url".to_string())?;
    let data_dir = crate::core::config::config_dir().map_err(|error| error.to_string())?;
    let mirror = crate::core::upstream_mirror::mirror_dir(&data_dir, &source.project_id, &url);
    let branches = source.target_branch.iter().cloned().collect::<Vec<_>>();
    let dir = mirror.clone();
    let tips = tokio::task::spawn_blocking(move || {
        crate::core::upstream_mirror::refresh(&dir, &url, &branches)
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok((mirror, tips))
}

pub fn short_sha(sha: &str) -> String {
    sha.chars().take(10).collect()
}

/// Compare `Name <email>` identities: exact name, case-insensitive email.
pub fn same_git_identity(trailer: &str, identity: &str) -> bool {
    fn parts(ident: &str) -> Option<(String, String)> {
        let (name, rest) = ident.trim().rsplit_once('<')?;
        let email = rest.strip_suffix('>')?.trim().to_ascii_lowercase();
        Some((name.split_whitespace().collect::<Vec<_>>().join(" "), email))
    }
    matches!((parts(trailer), parts(identity)), (Some(a), Some(b)) if a == b)
}

/// `worker_dod_ids` is the execution's launch-time DoD snapshot, passed when a
/// delivered manifest is judged AFTER the task's DoD may have changed (KT-1291):
/// the manifest pinned those ids, so one the principal since removed or carried
/// to another task is a known id, not a foreign one. Coverage is still owed for
/// every id the task holds NOW.
pub fn validate_manifest_claims(
    task_dod: &[PlanningDodItem],
    worker_dod_ids: Option<&[String]>,
    manifest: &DeliveryManifestV1,
) -> Result<(), String> {
    let expected = task_dod
        .iter()
        .map(|item| item.id.as_str())
        .chain(worker_dod_ids.into_iter().flatten().map(String::as_str))
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
    /// A delivered commit carries a `Signed-off-by`/`Co-Authored-By`-style
    /// trailer naming someone other than the repository's git identity.
    ForeignIdentityTrailers(String),
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
    /// request_changes was refused: the delivered file no longer fits the prelocalized
    /// range, and replaying its line numbers would edit moved content.
    StaleScope(String),
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
    decide_review_as(
        db,
        task_execution_id,
        decision_json,
        source_agent,
        source_session_id,
        None,
    )
    .await
}

/// [`decide_review`] for a caller that may hold a bridge token: the session it
/// names makes it a party only in the role its grant can hold (KT-1962).
pub async fn decide_review_as(
    db: &Database,
    task_execution_id: &str,
    decision_json: &str,
    source_agent: &str,
    source_session_id: &str,
    bridge: Option<&crate::core::bridge_token::BridgeCaller>,
) -> Result<ReviewOutcome, ProvisionError> {
    // ── 1. Resolve the execution, the caller's session and the run, server-side. ──
    let (exec, caller, run, shared_room) = {
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
            let shared_room = match &exec {
                Some(e) => worker_shares_the_principal_room(conn, e)?,
                None => false,
            };
            Ok((exec, caller, run, shared_room))
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
    // A session id is declared by the caller: with a bridge token it counts
    // only in the role the grant itself can hold.
    let is_worker = matches!(exec.worker_cli_session_id, Some(w) if w == caller.id)
        && bridge_may_be_execution_worker(bridge, &exec, &caller.disc_id);
    // When the worker is a CLI of the principal's own room, a grant of that
    // room may be the worker's: the principal's session must be one this
    // very launch joined or resumed.
    let is_principal = caller.disc_id == exec.parent_discussion_id
        && bridge.is_none_or(|bridge| {
            bridge_is_execution_principal(bridge, &exec)
                && (!shared_room
                    || (bridge.own_sessions.contains(&caller.id)
                        && !grant_holds_the_worker_session(bridge, &exec)))
        });
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

    // KT-837 keeps an execution-scoped worker in its principal room. That dual room role
    // must restore review reachability without letting the worker manufacture the separate
    // principal-only evidence that can override an unmet delivery DoD.
    decide_authorized_review(
        db,
        exec,
        &alias,
        Some(source_session_id.to_string()),
        is_principal && !is_worker,
        decision_json,
    )
    .await
}

/// Review entry point for a native HTTP agent. The executor, not the model,
/// supplies the current room and typed provider. Parent-room callers are principals;
/// child-room callers must match the execution's native worker provider exactly.
pub async fn decide_native_review(
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

/// Review entry point for the room's own native agent (the identity that
/// `task_exec_launch` accepts as principal). It is authorized only during its
/// running turn in the execution's parent room, and a dispatch that belongs to
/// an execution is refused there, so this caller is never the worker.
pub async fn decide_room_agent_review(
    db: &Database,
    task_execution_id: &str,
    decision_json: &str,
    room: SpawnedAgentCaller,
    bridge: Option<&crate::core::bridge_token::BridgeCaller>,
) -> Result<ReviewOutcome, ProvisionError> {
    let caller = PrincipalCaller::RoomAgent(room.clone());
    let Some(execution) =
        authorize_execution_principal(db, task_execution_id, &caller, bridge).await
    else {
        return Ok(ReviewOutcome::NotAddressed);
    };
    decide_authorized_review(
        db,
        execution,
        room.agent_type.trim(),
        Some(room_agent_actor_session(&room)),
        true,
        decision_json,
    )
    .await
}

/// Review entry point for a `DelegateSubtasks` workflow step (KT-909). The step
/// owns its principal room, so it decides as that room's principal, with no session.
pub async fn decide_review_as_delegation_step(
    db: &Database,
    task_execution_id: &str,
    decision_json: &str,
    alias: &str,
) -> Result<ReviewOutcome, ProvisionError> {
    let execution_id = task_execution_id.to_string();
    let execution = db
        .with_conn(move |conn| crate::db::orchestration::get_task_execution(conn, &execution_id))
        .await
        .map_err(|error| ProvisionError::Internal(error.to_string()))?;
    let Some(execution) = execution else {
        return Ok(ReviewOutcome::NotAddressed);
    };
    if !crate::workflows::delegate_subtasks_step::is_delegation_discussion(
        &execution.parent_discussion_id,
    ) {
        return Ok(ReviewOutcome::NotAddressed);
    }
    decide_authorized_review(db, execution, alias, None, true, decision_json).await
}

/// Pin a native worker call to the exact durable dispatch that launched it.
/// Room + provider alone are not identities: another run of the same provider
/// can coexist in a child room. The trusted executor carries the dispatch
/// trigger message outside model-controlled arguments, and the execution pins
/// the corresponding job id across retries/restarts.
pub fn native_worker_dispatch_matches(
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
    if job.discussion_id == sub_discussion_id && job.trigger_message_id == source_message_id {
        return Ok(true);
    }

    // Backend recovery may replace the dispatch row while the already-running
    // worker still holds the trusted trigger from the dispatch that launched it.
    // Keep that caller valid only inside the current assignment generation: the
    // latest assignment event is the hard boundary that makes every prior
    // worker stale, including a previous worker of the same provider.
    let current_assignment_started_at: Option<String> = conn
        .query_row(
            "SELECT created_at FROM task_execution_assignment_events \
             WHERE task_execution_id = ?1 ORDER BY generation DESC LIMIT 1",
            [execution.id.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    let Some(current_assignment_started_at) = current_assignment_started_at else {
        return Ok(false);
    };
    let reassignment_dedupe = conn
        .query_row(
            "SELECT generation FROM task_execution_assignment_events \
             WHERE task_execution_id = ?1 ORDER BY generation DESC LIMIT 1",
            [execution.id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(|generation| format!("orch-reassign:{}:{generation}", execution.id));
    let resume_dedupe = format!(
        "orch-resume-worker:{}:{}",
        execution.id, execution.attempt_no
    );
    let belongs_to_current_assignment: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_dispatch_jobs \
         WHERE discussion_id = ?1 AND trigger_message_id = ?2 \
           AND created_at >= ?3 \
           AND (dedupe_key = ?4 OR dedupe_key = ?5 OR dedupe_key LIKE ?6))",
        rusqlite::params![
            sub_discussion_id,
            source_message_id,
            current_assignment_started_at,
            reassignment_dedupe,
            resume_dedupe,
            format!("{}:retry:%", resume_dedupe),
        ],
        |row| row.get(0),
    )?;
    Ok(belongs_to_current_assignment)
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
            // No report here: it is published when the integration lands, and
            // this retry is what resumes that integration.
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
        // KT-1254 — persist the live checks this review declares, so integration
        // (and the plan) see them pending. Guards ran: the reviewer is the
        // principal, every id is a known DoD of this task.
        let declared = decision
            .dod_verifications
            .iter()
            .filter(|verification| {
                verification.verification == Some(PlanningDodVerification::PostIntegration)
            })
            .map(|verification| verification.dod_id.clone())
            .collect::<Vec<_>>();
        if reviewer_is_principal && !declared.is_empty() {
            let task_id = exec.task_id.clone();
            let actor = agent_actor(alias, actor_session_id.as_deref());
            db.with_conn(move |conn| {
                crate::db::planning::mark_dod_post_integration(conn, &task_id, &declared, &actor)
            })
            .await
            .map_err(|e| ProvisionError::Internal(e.to_string()))?;
        }
    }

    // KT-1291 — items carried to another ticket leave this task INSIDE the review
    // checkpoint transaction, with an event naming the carrier: a refused or failed
    // checkpoint detaches nothing, and integration neither waits on them nor ticks
    // them.
    let carried_owned = if decision.decision == ReviewVerdict::Approve && reviewer_is_principal {
        decision
            .dod_verifications
            .iter()
            .filter_map(|verification| {
                verification.carried_to.as_ref().map(|carrier| {
                    (
                        verification.dod_id.clone(),
                        carrier.clone(),
                        verification.evidence.clone(),
                    )
                })
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

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
        let (
            findings_owned,
            escalation_owned,
            token_escalation_owned,
            reactivation_owned,
            native_dispatch_owned,
        ) = if verdict == ReviewVerdict::RequestChanges {
            let (parent, tid) = (exec.parent_discussion_id.clone(), exec.task_id.clone());
            let eid = exec.id.clone();
            let (principal_target, task) = db
                .with_conn(move |conn| {
                    let parent = crate::db::discussions::get_discussion(conn, &parent)?
                        .context("parent discussion vanished before review")?;
                    let task = crate::db::planning::get_task(conn, &tid)?
                        .context("task vanished before review")?;
                    let target = crate::db::orchestration::principal_notice_target(
                        conn,
                        &eid,
                        parent.agent,
                    )?;
                    Ok((target, task))
                })
                .await
                .map_err(|e| ProvisionError::Internal(e.to_string()))?;
            let child = exec.sub_discussion_id.clone().unwrap_or_default();
            let worker_target = worker_target_from_execution(&exec)
                .map_err(|e| ProvisionError::Internal(e.to_string()))?;

            // A rework replays the prelocalized range on the delivered file; an
            // exhausted budget escalates instead, so it needs no re-anchoring.
            let reworks = exec.status == TaskExecutionStatus::AwaitingReview
                && exec.review_rounds < exec.max_review_rounds;
            let reanchored = if reworks {
                match reanchor_execution_scope(db, &exec).await {
                    Ok(reanchored) => reanchored,
                    Err(error) => {
                        return match error.downcast::<StaleWorkerScope>() {
                            Ok(stale) => Ok(ReviewOutcome::StaleScope(stale.0)),
                            Err(error) => Err(ProvisionError::Internal(error.to_string())),
                        }
                    }
                }
            } else {
                None
            };

            // (a) findings → the worker in the child (DoD-4).
            let lang = room_lang(db, &exec.parent_discussion_id).await;
            let mut findings_msg =
                build_review_findings_message(&exec.id, exec.attempt_no, &decision, &child, lang);
            if let Some(reanchored) = reanchored.as_ref() {
                findings_msg
                    .content
                    .push_str(&reanchored_scope_note(reanchored, lang));
            }
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
                lang,
            );
            let token_escalation_msg = build_token_budget_escalation_message(
                &exec.id,
                exec.attempt_no,
                &task.summary.reference,
                &task.summary.title,
                &child,
                lang,
            );
            let token_escalation = (
                exec.parent_discussion_id.clone(),
                token_escalation_msg,
                principal_target.clone(),
            );
            let escalation = (
                exec.parent_discussion_id.clone(),
                escalation_msg,
                principal_target,
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
                    lang,
                );
                Some((
                    offer_id,
                    new_attempt,
                    worker_target.clone(),
                    exec.parent_discussion_id.clone(),
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
                Some(token_escalation),
                reactivation,
                native_dispatch,
            )
        } else {
            (None, None, None, None, None)
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
            let token_escalation = token_escalation_owned
                .as_ref()
                .map(
                    |(parent, msg, target)| crate::db::orchestration::EscalationDelivery {
                        parent_discussion_id: parent,
                        message: msg,
                        principal_target: target,
                    },
                );
            let reactivation = reactivation_owned.as_ref().map(
                |(offer_id, new_attempt, target, origin, child, msg)| {
                    crate::db::orchestration::ReworkReoffer {
                        offer_id,
                        new_attempt_no: *new_attempt,
                        target_cli_session_id: target
                            .cli_session_id
                            .expect("a CLI worker target carries a session pk"),
                        origin_discussion_id: origin,
                        child_discussion_id: child,
                        control_message: msg,
                        control_target: target,
                    }
                },
            );
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
                    token_escalation,
                    reactivation,
                    native_dispatch,
                    carried_dod: &carried_owned,
                    actor: &actor,
                },
            )
        })
        .await
        .map_err(|e| ProvisionError::Internal(e.to_string()))?
    };

    use crate::db::orchestration::ReviewCheckpointOutcome;
    match outcome {
        ReviewCheckpointOutcome::DodCarryRefused(detail) => Ok(ReviewOutcome::ApproveBlocked {
            reason: ApproveBlockReason::ReviewEvidenceInvalid(detail),
        }),
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
pub async fn continue_approved_review(
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

/// Queue an approved checkpoint on a server-owned task. The execution and its
/// integration checkpoints are the durable job record; boot reconciliation can
/// resume them after a server stop. The spawned task is deliberately detached
/// from the HTTP/MCP request, so dropping that transport cannot cancel a long
/// validation command. Concurrent enqueues are safe: the first task commits
/// the `Approved -> Integrating` candidate-anchor checkpoint, and every later
/// task loses that durable status gate before it can build, validate, or apply.
fn enqueue_approved_integration(state: &AppState, outcome: ReviewOutcome) -> ReviewOutcome {
    let ReviewOutcome::Reviewed {
        verdict: ReviewVerdict::Approve,
        execution,
    } = &outcome
    else {
        return outcome;
    };
    let db = state.db.clone();
    let execution_id = execution.id.clone();
    tokio::spawn(async move {
        match run_integration(&db, &execution_id).await {
            Ok(IntegrationOutcome::Refused { reason }) => {
                tracing::warn!(execution_id, %reason, "protected integration was refused after approval")
            }
            // The worker was re-activated and the principal told inside the
            // send-back checkpoint; this only leaves a trace in the server log.
            Ok(IntegrationOutcome::SentBack { reason }) => {
                tracing::info!(execution_id, %reason, "approved delivery sent back to its worker")
            }
            Ok(_) => {}
            Err(error) => tracing::error!(execution_id, ?error, "protected integration job failed"),
        }
    });
    outcome
}

/// The DoD-5 approve guard. Returns `Some(reason)` if approve must be refused, `None` if the
/// delivery is present, its reported DoD are all met, and the worktree HEAD has NOT drifted
/// from the delivered `head_sha`. Both shas pass through `resolve_commit`, so an abbreviated
/// delivered sha and the full worktree HEAD compare equal — the abbreviated case never
/// false-refuses (the one case that distinguishes normalization from a raw string compare).
/// Publish the accepted report, logging rather than failing.
///
/// The landing is already durable when this runs. Turning a publication
/// failure into an error would fail an integration that HAPPENED. The record is
/// idempotent and self-repairing, so a later landing replay finishes the job.
async fn publish_accepted_delivery_or_warn(db: &Database, execution: &TaskExecution) {
    match crate::api::delivery_publication::publish_accepted_delivery(db, execution).await {
        Ok(Some(outcome)) => tracing::info!(
            execution = %execution.id,
            attempt = execution.attempt_no,
            ?outcome,
            "accepted delivery report published"
        ),
        Ok(None) => tracing::debug!(
            execution = %execution.id,
            "approved execution carries no manifest — nothing to report"
        ),
        Err(error) => tracing::warn!(
            execution = %execution.id,
            attempt = execution.attempt_no,
            error = %error,
            "accepted delivery report not published — a later approve replay repairs it"
        ),
    }
}

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
    // KT-1291 — judged against the launch-time DoD snapshot: an item the
    // principal removed or carried since the delivery is not a foreign id.
    if let Err(detail) = validate_manifest_claims(
        &task.definition_of_done,
        exec.worker_dod_ids.as_deref(),
        &manifest,
    ) {
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
    // A carried item may already have left this task: its id is then the
    // snapshot's.
    let known_dod_ids = task_dod_ids
        .iter()
        .copied()
        .chain(exec.worker_dod_ids.iter().flatten().map(String::as_str))
        .collect::<std::collections::HashSet<_>>();
    if let Some(foreign) = decision.dod_verifications.iter().find(|verification| {
        let known = if verification.carried_to.is_some() {
            &known_dod_ids
        } else {
            &task_dod_ids
        };
        !known.contains(verification.dod_id.as_str())
    }) {
        return Ok(Some(ApproveBlockReason::ReviewEvidenceInvalid(format!(
            "review evidence references unknown DoD id `{}`",
            foreign.dod_id
        ))));
    }
    // KT-1291 — an item carried to another ticket is explicit: unmet, not a
    // live check, and its carrier exists, is open and holds the requirement.
    let carried = decision
        .dod_verifications
        .iter()
        .filter(|verification| verification.carried_to.is_some())
        .collect::<Vec<_>>();
    for verification in &carried {
        let carrier = verification.carried_to.as_deref().unwrap_or_default();
        if verification.met || verification.verification.is_some() {
            return Ok(Some(ApproveBlockReason::ReviewEvidenceInvalid(format!(
                "DoD `{}` carried to `{carrier}` must be `met: false` with no `verification`",
                verification.dod_id
            ))));
        }
        let (task_id, execution_id, dod_id, carrier_owned) = (
            exec.task_id.clone(),
            exec.id.clone(),
            verification.dod_id.clone(),
            carrier.to_string(),
        );
        let checked = db
            .with_conn(move |conn| {
                crate::db::planning::check_dod_carrier(
                    conn,
                    &task_id,
                    &execution_id,
                    &dod_id,
                    &carrier_owned,
                )
            })
            .await
            .map_err(|e| ProvisionError::Internal(e.to_string()))?;
        if let crate::db::planning::DodCarrierCheck::Refused(detail) = checked {
            return Ok(Some(ApproveBlockReason::ReviewEvidenceInvalid(detail)));
        }
    }
    let carried_ids = carried
        .iter()
        .map(|verification| verification.dod_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    // KT-1254 — items the principal checks live AFTER integration: declared on
    // the task, or by this very review. They never block approve; the task
    // stays open on them until they are ticked with evidence.
    let declared_now = decision
        .dod_verifications
        .iter()
        .filter(|verification| {
            verification.verification == Some(PlanningDodVerification::PostIntegration)
        })
        .collect::<Vec<_>>();
    if let Some(contradiction) = declared_now.iter().find(|verification| verification.met) {
        return Ok(Some(ApproveBlockReason::ReviewEvidenceInvalid(format!(
            "DoD `{}` is declared post_integration, so it cannot be met yet; \
             tick it with evidence after integration",
            contradiction.dod_id
        ))));
    }
    let post_integration = task
        .definition_of_done
        .iter()
        .filter(|item| item.verification == PlanningDodVerification::PostIntegration)
        .map(|item| item.id.as_str())
        .chain(declared_now.iter().map(|v| v.dod_id.as_str()))
        .collect::<std::collections::HashSet<_>>();
    if reviewer_is_principal {
        let submitted = decision
            .dod_verifications
            .iter()
            .map(|verification| verification.dod_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let missing = task
            .definition_of_done
            .iter()
            .filter(|item| {
                !submitted.contains(item.id.as_str())
                    && !post_integration.contains(item.id.as_str())
            })
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
            .filter(|verification| {
                !verification.met
                    && !post_integration.contains(verification.dod_id.as_str())
                    && !carried_ids.contains(verification.dod_id.as_str())
            })
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
        .filter(|status| {
            !status.met
                && !verified.contains(status.dod_id.as_str())
                && !post_integration.contains(status.dod_id.as_str())
                && !carried_ids.contains(status.dod_id.as_str())
        })
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
    let upstream = execution_upstream_source(db, exec).await;
    let facts = match delivery_git_facts_from_repo(repo.to_path_buf(), base, upstream) {
        Ok(facts) => facts,
        Err(error) => {
            return Ok(Some(ApproveBlockReason::WorktreeUnavailable(error)));
        }
    };
    if let Err(detail) = validate_committed_file_inventory(&facts, &manifest) {
        return Ok(Some(ApproveBlockReason::ManifestDiffMismatch(detail)));
    }
    // Also guards deliveries accepted before this check existed.
    if let Err(detail) = validate_delivery_identity_trailers(&facts).await {
        return Ok(Some(ApproveBlockReason::ForeignIdentityTrailers(detail)));
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
    lang: RoomLang,
) -> DiscussionMessage {
    let comment = decision.comment.as_deref().unwrap_or("").trim();
    let findings = if decision.findings.is_empty() {
        lang.pick(
            "_(aucun point structuré — voir le commentaire)_",
            "_(no structured point — see the comment)_",
        )
        .to_string()
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
    let content = match lang {
        RoomLang::Fr => {
            let comment = if comment.is_empty() {
                "_(voir les points ci-dessous)_"
            } else {
                comment
            };
            format!(
                "**Changements demandés — revue du tour livré**\n\n\
                 Le principal a renvoyé ton travail avec `request_changes`. Corrige les points \
                 ci-dessous **dans ce worktree** (il est conservé, comme cette sous-discussion), \
                 puis re-livre un nouveau DeliveryManifest via `task_exec_deliver`.\n\n\
                 ## Commentaire\n{comment}\n\n\
                 ## Points à traiter\n{findings}"
            )
        }
        RoomLang::En => {
            let comment = if comment.is_empty() {
                "_(see the points below)_"
            } else {
                comment
            };
            format!(
                "**Changes requested — review of the delivered round**\n\n\
                 The principal sent your work back with `request_changes`. Fix the points \
                 below **in this worktree** (it is kept, like this sub-discussion), \
                 then re-deliver a new DeliveryManifest through `task_exec_deliver`.\n\n\
                 ## Comment\n{comment}\n\n\
                 ## Points to address\n{findings}"
            )
        }
    };
    orchestrator_message(
        format!("orch-review-findings:{exec_id}:{attempt_no}"),
        content + crate::core::ui_captures::retake_hint(lang),
    )
}

/// KT-1688 — the integration branch a launch will target, or the refusal text.
///
/// An explicit `base_rev` wins; without one the room's active campaign declares
/// it; with neither there is no target, never `main` by default. The branch must
/// be a local one, and a checkout holding it must be Kronn-owned: integration
/// writes into that checkout, so the owner's working checkout (or any foreign
/// worktree) is refused here, before anything is created.
pub fn resolve_launch_target(
    conn: &rusqlite::Connection,
    parent_discussion_id: &str,
    repo_path: &std::path::Path,
    project_roots: &[std::path::PathBuf],
    base_rev: Option<&str>,
) -> Result<Result<String, String>> {
    let requested = base_rev.map(str::trim).filter(|rev| !rev.is_empty());
    let declared = match requested {
        Some(rev) => Some(rev.to_string()),
        None => crate::db::orchestration::get_active_campaign_for_discussion(
            conn,
            parent_discussion_id,
        )?
        .and_then(|run| run.target_branch)
        .filter(|branch| !branch.trim().is_empty()),
    };
    let Some(declared) = declared else {
        return Ok(Err(
            worktree::IntegrationTargetRefusal::NoneDeclared.message()
        ));
    };
    let branch = match worktree::resolve_local_branch(repo_path, &declared) {
        Ok(branch) => branch,
        Err(reason) => return Ok(Err(reason)),
    };
    match worktree::integration_target_ownership(repo_path, &branch, project_roots, &|path| {
        crate::db::discussion_workspaces::is_registered_managed_checkout(conn, path)
    }) {
        Ok(Ok(())) => Ok(Ok(branch)),
        Ok(Err(refusal)) => Ok(Err(refusal.message())),
        Err(reason) => Ok(Err(reason)),
    }
}

/// KT-1688 — the integration-target check of the preflight. A named or declared
/// target that would be refused makes the task not launchable; with neither, the
/// preflight only warns, since `base_rev` is still given at launch.
pub fn integration_target_preparation(
    conn: &rusqlite::Connection,
    preparation: &mut crate::models::TaskExecutionPreparation,
    base_rev: Option<&str>,
) -> Result<()> {
    let Some(project_id) = preparation.project_id.clone() else {
        return Ok(());
    };
    let Some(project) = crate::db::projects::get_project(conn, &project_id)? else {
        return Ok(());
    };
    let repo = scanner::resolve_host_path(&project.path);
    let roots = project_managed_roots(&project);
    let Err(message) = resolve_launch_target(
        conn,
        &preparation.parent_discussion_id,
        &repo,
        &roots,
        base_rev,
    )?
    else {
        return Ok(());
    };
    let (code, detail) = message
        .split_once(": ")
        .filter(|(code, _)| code.starts_with("integration_target_"))
        .unwrap_or(("integration_target_invalid", message.as_str()));
    let undeclared = code == worktree::IntegrationTargetRefusal::NoneDeclared.code();
    let reason = preparation_reason(code, detail.to_string());
    if undeclared {
        preparation.warnings.push(reason);
    } else {
        preparation.launchable = false;
        preparation.reasons.push(reason);
    }
    Ok(())
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
        max_review_rounds,
        quota_fallback_agents,
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

    if !is_replay {
        if let Err(reason) = validate_new_validation_specs(validations) {
            return Ok(Err(ProvisionError::NotLaunchable(format!(
                "invalid validations: {reason}"
            ))));
        }
    }

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
    if let (true, Some(_), Some(execution)) = (is_replay, worker_scope, existing.as_ref()) {
        // Compare with the launch scope: a rework may have re-anchored the persisted one.
        let launched = crate::db::orchestration::launch_worker_scope(conn, &execution.id)?;
        if launched.as_ref() != worker_scope {
            return Ok(Err(ProvisionError::NotLaunchable(
                "idempotent replay cannot change the persisted worker_scope".into(),
            )));
        }
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
    if !is_replay {
        if let Some(reason) = worker_project_refusal_naming_finished(conn, worker, &project) {
            return Ok(Err(ProvisionError::NotLaunchable(format!(
                "{}: {}",
                reason.code, reason.detail
            ))));
        }
    }
    let repo_path = scanner::resolve_host_path(&project.path)
        .to_string_lossy()
        .to_string();

    // The child checkout is SHA-pinned later, but the integration target must
    // stay a named local branch. A raw SHA/tag is immutable and would make the
    // apply drift check observe that object forever instead of the real parent.
    let target_branch = if is_replay {
        // The replayed execution keeps the target its run already pinned.
        let pinned = match existing.as_ref() {
            Some(execution) => crate::db::orchestration::get_orchestration_run(
                conn,
                &execution.orchestration_run_id,
            )?
            .and_then(|run| run.target_branch),
            None => None,
        };
        match pinned.or_else(|| target_branch.map(str::to_string)) {
            Some(branch) => branch,
            None => {
                return Ok(Err(ProvisionError::NotLaunchable(
                    worktree::IntegrationTargetRefusal::NoneDeclared.message(),
                )))
            }
        }
    } else {
        match resolve_launch_target(
            conn,
            parent_discussion_id,
            std::path::Path::new(&repo_path),
            &project_managed_roots(&project),
            target_branch,
        )? {
            Ok(branch) => branch,
            Err(reason) => return Ok(Err(ProvisionError::NotLaunchable(reason))),
        }
    };

    if !is_replay {
        let active_blockers = task
            .blockers
            .iter()
            .filter(|b| !b.satisfies_dependents())
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
    let launch_target_branch = target_branch.clone();
    launch.target_branch = Some(target_branch);
    launch.validations = validations.to_vec();
    if let Some(rounds) = max_review_rounds {
        launch.max_review_rounds = rounds;
    }
    launch.quota_fallback_agents = quota_fallback_agents.to_vec();
    launch.worker_target_kind = Some(worker.kind);
    launch.worker_cli_session_id = worker.cli_session_id;
    launch.worker_connection_id = worker.connection_id.clone();
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
        target_branch: launch_target_branch.clone(),
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
pub fn advance_to_provisioning(
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

pub fn model_tier_from_db(s: &str) -> Option<ModelTier> {
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
pub fn worker_target_from_execution(execution: &TaskExecution) -> Result<MessageTarget> {
    let kind = execution
        .worker_target_kind
        .context("execution has no worker_target_kind — cannot reconstruct the worker")?;
    let agent_type = crate::db::orchestration::agent_type_from_db(
        execution
            .worker_agent_type
            .as_deref()
            .context("execution has no worker_agent_type")?,
    )?;
    let connection_id = execution
        .worker_connection_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if agent_type == AgentType::Custom && connection_id.is_none() {
        bail!("worker connection identifier is required for Custom providers");
    }
    let tier = execution
        .worker_model_tier
        .as_deref()
        .and_then(model_tier_from_db);
    Ok(MessageTarget {
        kind,
        agent_type,
        connection_id: connection_id.map(str::to_string),
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
        // KT-545 — the delegated worker's own resolved connection, so a
        // sub-discussion routed to a named HTTP connection keeps resolving
        // through it on any turn beyond the initial brief.
        connection_id: worker.connection_id.clone(),
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
pub fn build_brief(
    prepared: &Prepared,
    worktree_path: &str,
    branch: &str,
    base_sha: &str,
    toolchain: Option<&ToolchainCapabilities>,
    lang: RoomLang,
) -> DiscussionMessage {
    let can_run_shell = !matches!(
        prepared.execution.worker_agent_type.as_deref(),
        Some("Ollama" | "LiteLlm" | "Nvidia")
    );
    let mut content = worker_brief_markdown_in(
        lang,
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
        project_signs_off(worktree_path),
    );
    if let Some(toolchain) = toolchain {
        let profile =
            crate::core::project_toolchains::detect_toolchains(std::path::Path::new(worktree_path));
        content = insert_toolchain_section_in(content, toolchain, Some(&profile), lang);
    }
    let content = brief_for_agent(
        content,
        prepared.execution.worker_agent_type.as_deref(),
        lang,
    );
    orchestrator_message(
        format!(
            "orch-brief:{}:{}",
            prepared.execution.id, prepared.execution.attempt_no
        ),
        content,
    )
}

/// The brief's "what you can run" section (KT-1246), from the launch-time smoke
/// check: the worker reads it instead of discovering a refusal mid-task.
#[cfg(any(test, feature = "test-support"))]
pub fn toolchain_brief_section(toolchain: &ToolchainCapabilities, lang: RoomLang) -> String {
    toolchain_brief_section_in(toolchain, None, lang)
}

/// [`toolchain_brief_section`] limited to the toolchains of `profile` (KT-1753): a
/// project without Cargo is told nothing about `cargo nextest` or its target
/// directory, one without Node nothing about Vite. `None` describes everything.
/// The ports are named by the lane's variables, and no default port is free.
pub fn toolchain_brief_section_in(
    toolchain: &ToolchainCapabilities,
    profile: Option<&[crate::core::project_toolchains::PackageRoot]>,
    lang: RoomLang,
) -> String {
    use crate::core::project_toolchains::ToolchainKind;
    let has = |wanted: fn(ToolchainKind) -> bool| {
        profile.is_none_or(|roots| roots.iter().any(|root| wanted(root.kind)))
    };
    let cargo = has(|kind| kind == ToolchainKind::Cargo);
    let node = has(|kind| matches!(kind, ToolchainKind::Node(_)));
    let en = lang == RoomLang::En;
    let line = |label: &str, capability: &Capability| {
        format!(
            "- {label}{} {} — {}\n",
            lang.pick(" :", ":"),
            if capability.available {
                lang.pick("✅ disponible", "✅ available")
            } else {
                lang.pick("❌ indisponible", "❌ unavailable")
            },
            capability.reason
        )
    };
    let mut out = String::from(if en {
        "## Test capabilities (verified at launch)\n"
    } else {
        "## Capacités de test (vérifiées au lancement)\n"
    });
    if cargo {
        out.push_str(&line(
            if en {
                "Rust tests (`cargo`)"
            } else {
                "Tests Rust (`cargo`)"
            },
            &toolchain.rust_tests,
        ));
    }
    // A Node project that does not test with vitest is not told its tests are
    // "unavailable": it runs its own (see the project conventions).
    let vitest_line = node && !toolchain.frontend_tests.is_not_applicable();
    if vitest_line {
        out.push_str(&line(
            if en {
                "Frontend tests (`vitest`)"
            } else {
                "Tests frontend (`vitest`)"
            },
            &toolchain.frontend_tests,
        ));
    }
    out.push_str(&line(
        if en {
            "Loopback port binding"
        } else {
            "Binding de port loopback"
        },
        &toolchain.port_binding,
    ));
    let ports = match (en, node) {
        (true, true) => {
            "`$KRONN_PORT` for a backend and `$VITE_DEV_PORT` for a front-end dev server (Vite \
             proxies to `$KRONN_VITE_BACKEND_URL`)"
        }
        (false, true) => {
            "`$KRONN_PORT` pour un backend et `$VITE_DEV_PORT` pour un serveur de dev front \
             (Vite proxifie vers `$KRONN_VITE_BACKEND_URL`)"
        }
        (_, false) => "`$KRONN_PORT`",
    };
    out.push_str(&if en {
        format!(
            "Servers: anything you start (an API, a dev server, a preview) listens on a port of \
             your environment: {ports}. Never listen on a default port (3140, 5173, 3000, \
             8080…): those belong to the host's own services, and you would hijack them and the \
             other workers. What you start stops with your turn: Kronn ends every process \
             launched by your session when the dispatch finishes.\n"
        )
    } else {
        format!(
            "Serveurs : tout ce que tu démarres (une API, un serveur de dev, une prévisualisation) \
             écoute sur un port de ton environnement : {ports}. N'écoute jamais sur un port par \
             défaut (3140, 5173, 3000, 8080…) : ils sont ceux des services de l'hôte, et tu les \
             détournerais, avec les autres workers. Ce que tu démarres s'arrête avec ton tour : \
             Kronn termine tout processus lancé par ta session quand le dispatch finit.\n"
        )
    });
    if cargo {
        out.push_str(if en {
            "A binary your build produces runs from your lane's target (`$CARGO_TARGET_DIR/debug`).\n"
        } else {
            "Un binaire que ton build produit se lance depuis la cible de ta lane \
             (`$CARGO_TARGET_DIR/debug`).\n"
        });
    }
    if vitest_line {
        out.push_str(if en {
            "Vite and vitest run as they are (`vitest run`, `vite --port $VITE_DEV_PORT`): your \
             lane's `node_modules` is a writable folder, Vite writes its config bundle there. If \
             you see `EPERM` on `node_modules/.vite-temp`, run again with \
             `--configLoader native`.\n"
        } else {
            "Vite et vitest se lancent tels quels (`vitest run`, `vite --port $VITE_DEV_PORT`) : \
             le `node_modules` de ta lane est un dossier inscriptible, Vite y écrit son bundle de \
             configuration. Si tu vois `EPERM` sur `node_modules/.vite-temp`, relance avec \
             `--configLoader native`.\n"
        });
    }
    out.push_str(if en {
        "Run only what is ✅. A ❌ validation is never asserted: report it `skipped` with the \
         reason above, the principal will run it. `tests_executed` states explicitly whether at \
         least one test actually ran and which.\n"
    } else {
        "Lance uniquement ce qui est ✅. Une validation ❌ ne s'affirme jamais : reporte-la \
         `skipped` avec la raison ci-dessus, le principal l'exécutera. `tests_executed` dit \
         explicitement si au moins un test a réellement tourné et lesquels.\n"
    });
    // The checks above cover Rust and vitest only. A project's own test commands
    // are not "unavailable": they are simply not probed.
    if profile.is_some() {
        out.push_str(if en {
            "This check covers only the tests listed above. For any other toolchain, run the \
             project's own test commands (see Project conventions): they are not unavailable, \
             just not probed here.\n"
        } else {
            "Cette vérification ne couvre que les tests listés ci-dessus. Pour toute autre \
             chaîne d'outils, lance les commandes de test propres au projet (voir Conventions du \
             projet) : elles ne sont pas indisponibles, simplement non sondées ici.\n"
        });
    }
    out.push('\n');
    out
}

/// The brief of a task worker of `agent_type` (KT-1753). A Codex worker's sandbox
/// cannot write the worktree's git directory, so the section that tells a worker
/// to `git merge` the target is replaced by the line saying the principal does it.
pub fn brief_for_agent(content: String, agent_type: Option<&str>, lang: RoomLang) -> String {
    if agent_type != Some("Codex") {
        return content;
    }
    let (heading, replacement) = if lang == RoomLang::En {
        (
            "## Integrate the target branch\n",
            "## Integrate the target branch\n\
             Your sandbox cannot write this worktree's git directory: the principal integrates \
             the target branch into your branch when it is needed, you do not merge.\n\n",
        )
    } else {
        (
            "## Intégrer la branche cible\n",
            "## Intégrer la branche cible\n\
             Ta sandbox ne peut pas écrire le dossier git de ce worktree : le principal intègre \
             la branche cible dans ta branche quand c'est nécessaire, tu ne fusionnes pas.\n\n",
        )
    };
    let Some(start) = content.find(heading) else {
        return content;
    };
    let after = start + heading.len();
    let end = content[after..]
        .find("\n## ")
        .map_or(content.len(), |at| after + at + 1);
    format!("{}{replacement}{}", &content[..start], &content[end..])
}

/// Put the capability section right before the brief's `## Tests` section.
#[cfg(any(test, feature = "test-support"))]
pub fn insert_toolchain_section(
    content: String,
    toolchain: &ToolchainCapabilities,
    lang: RoomLang,
) -> String {
    insert_toolchain_section_in(content, toolchain, None, lang)
}

/// [`insert_toolchain_section`] limited to the toolchains of `profile` (KT-1753).
fn insert_toolchain_section_in(
    content: String,
    toolchain: &ToolchainCapabilities,
    profile: Option<&[crate::core::project_toolchains::PackageRoot]>,
    lang: RoomLang,
) -> String {
    let section = toolchain_brief_section_in(toolchain, profile, lang);
    match content.find("## Tests\n") {
        Some(at) => format!("{}{section}{}", &content[..at], &content[at..]),
        None => format!("{content}\n\n{section}"),
    }
}

/// Tell the principal room a launch was refused for lack of disk (KT-1318).
/// Never raised: the refusal itself is the error the caller gets.
async fn post_disk_pressure_alert(
    db: &Database,
    room_id: &str,
    task_reference: &str,
    reason: &str,
) {
    let room = room_id.to_string();
    let mut notice = orchestrator_message(
        format!("orch-disk-pressure:{}", uuid::Uuid::new_v4()),
        format!(
            "⚠️ {task_reference} was not launched: the disk is too full for a worker. {reason} \
             The execution is parked Blocked and resumes once space is free."
        ),
    );
    notice.role = MessageRole::System;
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::discussions::insert_message(conn, &room, &notice).map(|_| ())
        })
        .await
    {
        tracing::warn!("disk pressure alert not posted: {error}");
    }
}

/// Write the lane target's ownership marker (KT-1256). Never raised: a lane
/// that could not be marked is only ever reported by the sweep, never removed.
async fn mark_lane_target_for_execution(
    repo_path: &std::path::Path,
    worktree: &std::path::Path,
    exec_id: &str,
) {
    let (repo, worktree_path, id) = (
        repo_path.to_path_buf(),
        worktree.to_path_buf(),
        exec_id.to_string(),
    );
    match tokio::task::spawn_blocking(move || {
        worktree::mark_lane_target(&repo, &worktree_path, &id)
    })
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(reason)) => {
            tracing::warn!(execution_id = %exec_id, "lane target not marked: {reason}");
        }
        Err(error) => {
            tracing::warn!(execution_id = %exec_id, "lane target marking did not complete: {error}");
        }
    }
}

/// [`mark_lane_target_for_execution`] when the repository is derived from the
/// managed worktree layout: for the launches that only know the worktree.
async fn mark_lane_target_for_worker(exec_id: &str, worktree: &std::path::Path) {
    let (worktree_path, id) = (worktree.to_path_buf(), exec_id.to_string());
    match tokio::task::spawn_blocking(move || {
        worktree::mark_lane_target_for_worktree(&worktree_path, &id)
    })
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(reason)) => {
            tracing::debug!(execution_id = %exec_id, "lane target not marked: {reason}");
        }
        Err(error) => {
            tracing::warn!(execution_id = %exec_id, "lane target marking did not complete: {error}");
        }
    }
}

/// Seed a task worktree with the project's gitignored assets (KT-1438) and
/// journal what happened. Run at provisioning and again before the integration
/// validations, so a source that changed meanwhile is refreshed. Best-effort:
/// a path that cannot be seeded is recorded with its reason and the worktree
/// goes on without it; it never fails a launch or an integration.
///
/// Returns how long the copy took (KT-1637), `None` when nothing was seeded.
async fn seed_lane_assets(
    db: &Database,
    exec_id: &str,
    project_id: &str,
    repo: &std::path::Path,
    worktree: &std::path::Path,
) -> Option<u64> {
    let pid = project_id.to_string();
    let policy = crate::core::task_worker_lane_seeds::seed_policy();
    if !policy.copy_enabled {
        // The owner turned copying off (Settings > Worktrees): nothing is seeded.
        return None;
    }
    // Only what a human declared in Kronn applies, or, when the owner turned
    // adoption on, Kronn's suggestions for a project that declared nothing. The
    // `[lane_seeds]` table of `kronn/project.toml` reaches the repository through
    // a pull request, not through the human gate: it is proposed in the settings,
    // never seeded here. The secret-looking files a human confirmed travel with
    // them: a seed that gained one since is held, not copied.
    let (declared, confirmed) = match db
        .with_read_conn(move |conn| {
            Ok((
                crate::db::projects::lane_seeds(conn, &pid)?,
                crate::db::projects::lane_seeds_confirmed(conn, &pid)?,
            ))
        })
        .await
    {
        Ok(read) => read,
        Err(error) => {
            tracing::warn!(execution_id = %exec_id, "lane seeds not read: {error}");
            return None;
        }
    };
    let (repo, worktree) = (repo.to_path_buf(), worktree.to_path_buf());
    let (record, duration_ms) = match tokio::task::spawn_blocking(move || {
        let (seeds, _) =
            match crate::core::task_worker_lane_seeds::effective_seeds(policy, declared, || {
                crate::core::task_worker_lane_seeds::suggest_seeds(&repo)
            }) {
                Ok(effective) => effective,
                Err(reason) => {
                    tracing::warn!("lane seed suggestions not read: {reason}");
                    return None;
                }
            };
        if seeds.is_empty() {
            return None;
        }
        let copying = std::time::Instant::now();
        let record = crate::core::task_worker_lane_seeds::seed_worktree(
            &repo,
            &worktree,
            &seeds,
            &confirmed,
            &crate::core::task_worker_lane_warm::clone_tree,
        );
        Some((record, copying.elapsed().as_millis() as u64))
    })
    .await
    {
        Ok(Some(seeded)) => seeded,
        Ok(None) => return None,
        Err(error) => {
            tracing::warn!(execution_id = %exec_id, "lane seeding did not complete: {error}");
            return None;
        }
    };
    for (path, reason) in record.skipped() {
        tracing::warn!(execution_id = %exec_id, "lane seed `{path}` skipped: {reason}");
    }
    let id = exec_id.to_string();
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::orchestration::record_lane_seeds(conn, &id, &record, &backend_actor())
        })
        .await
    {
        tracing::warn!(execution_id = %exec_id, "lane seeds record not persisted: {error}");
    }
    Some(duration_ms)
}

/// Journal how long a sub-step of the warming phase took (KT-1637). Never
/// raised: a timing that is not recorded only leaves a hole in the breakdown.
async fn record_warming_step(
    db: &Database,
    exec_id: &str,
    step: crate::models::WarmingStepKind,
    duration_ms: u64,
) {
    let id = exec_id.to_string();
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::orchestration::record_warming_step(
                conn,
                &id,
                step,
                duration_ms,
                &backend_actor(),
            )
        })
        .await
    {
        tracing::warn!(execution_id = %exec_id, "warming step {step:?} not recorded: {error}");
    }
}

/// Fill the host's dependency caches with what the worker's lock files
/// reference and they lack (KT-1649), and journal it on the execution. Runs as
/// Kronn's own process, not under the worker's sandbox (it needs the network),
/// which is why `core::task_worker_dep_fetch` bounds it to the lock files'
/// public registries. Never raised: a fetch that fails only leaves the worker
/// with the cache it had.
pub async fn record_dependency_fetch(
    db: &Database,
    exec_id: &str,
    worktree: &std::path::Path,
    runner: std::sync::Arc<dyn crate::core::task_worker_env::ProbeRunner>,
    cancel: tokio_util::sync::CancellationToken,
) {
    let path = worktree.to_path_buf();
    // The staged projects live under Kronn's data directory, which the worker
    // cannot read; no data directory, no scratch, no fetch.
    let scratch_root = match crate::core::task_worker_dep_fetch::scratch_root() {
        Ok(root) => root,
        Err(error) => {
            tracing::warn!(execution_id = %exec_id, "dependency fetch skipped: {error}");
            return;
        }
    };
    let record = tokio::task::spawn_blocking(move || {
        crate::core::task_worker_dep_fetch::fetch_missing_dependencies(
            &path,
            &scratch_root,
            runner.as_ref(),
            &cancel,
        )
    })
    .await;
    let record = match record {
        Ok(record) => record,
        Err(error) => {
            tracing::warn!(execution_id = %exec_id, "dependency fetch did not complete: {error}");
            return;
        }
    };
    if record.steps.is_empty() {
        return;
    }
    for step in &record.steps {
        tracing::info!(
            execution_id = %exec_id,
            lockfile = %step.lockfile,
            outcome = ?step.outcome,
            "task worker dependency fetch"
        );
    }
    let id = exec_id.to_string();
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::orchestration::record_dependency_fetch(conn, &id, &record, &backend_actor())
        })
        .await
    {
        tracing::warn!(execution_id = %exec_id, "dependency fetch record not persisted: {error}");
    }
}

/// The commands of the execution's validations (its run's, principal-owned), or
/// none when they cannot be read — which warms the lane: when in doubt, warm.
async fn execution_validation_commands(db: &Database, exec_id: &str) -> Vec<String> {
    let id = exec_id.to_string();
    db.with_conn(move |conn| {
        let Some(execution) = crate::db::orchestration::get_task_execution(conn, &id)? else {
            return Ok(Vec::new());
        };
        Ok(
            crate::db::orchestration::get_orchestration_run(conn, &execution.orchestration_run_id)?
                .map(|run| run.validations.into_iter().map(|spec| spec.command).collect())
                .unwrap_or_default(),
        )
    })
    .await
    .unwrap_or_else(|error| {
        tracing::warn!(execution_id = %exec_id, "validations not read, the lane is warmed: {error}");
        Vec::new()
    })
}

/// How many times a skipped warm-up re-reads validations that changed under it.
const WARM_SKIP_RECHECKS: usize = 3;

/// KT-1980 — warm the lane unless the execution's validations start no build.
/// A skip is journalled, then the validations are read again: a principal who
/// replaced them before the skip was on record (the amendment is only refused
/// once it is) gets a decision on the new set, which warms when it needs to.
async fn warm_lane_for_validations(
    db: &Database,
    exec_id: &str,
    attempt_no: u32,
    room_id: &str,
    worktree: &std::path::Path,
    repo_path: &str,
    cancel: tokio_util::sync::CancellationToken,
) -> Option<crate::core::task_worker_lane_warm::WarmStartRecord> {
    let mut validations = execution_validation_commands(db, exec_id).await;
    let mut rechecks = 0;
    loop {
        let record = record_lane_warm_start(
            db,
            exec_id,
            attempt_no,
            room_id,
            worktree,
            &validations,
            worker_probe_runner(repo_path, worktree),
            cancel.clone(),
        )
        .await;
        if !record
            .as_ref()
            .is_some_and(|record| record.skipped_by_validations.is_some())
            || rechecks == WARM_SKIP_RECHECKS
        {
            return record;
        }
        let current = execution_validation_commands(db, exec_id).await;
        if current == validations {
            return record;
        }
        validations = current;
        rechecks += 1;
    }
}

/// Start a Claude task worker's lane warm (KT-1250): seed its Cargo target with
/// a copy-on-write clone of a warm one, then run its first build under the
/// host's concurrency cap, saying so in the worker's room when it has to queue.
/// Journalled on the execution; never raised — a lane that stays cold only
/// costs the worker its own compile time.
#[allow(clippy::too_many_arguments)]
pub async fn record_lane_warm_start(
    db: &Database,
    exec_id: &str,
    attempt_no: u32,
    room_id: &str,
    worktree: &std::path::Path,
    validations: &[String],
    runner: std::sync::Arc<dyn crate::core::task_worker_env::ProbeRunner>,
    cancel: tokio_util::sync::CancellationToken,
) -> Option<crate::core::task_worker_lane_warm::WarmStartRecord> {
    let (queued_tx, mut queued_rx) = tokio::sync::mpsc::unbounded_channel::<usize>();
    let path = worktree.to_path_buf();
    let validations = validations.to_vec();
    let warm = tokio::task::spawn_blocking(move || {
        crate::core::task_worker_lane_warm::warm_worktree(
            &path,
            &validations,
            runner.as_ref(),
            &cancel,
            |ahead| {
                let _ = queued_tx.send(ahead);
            },
        )
    });
    let announce = async {
        while let Some(ahead) = queued_rx.recv().await {
            let room = room_id.to_string();
            let mut notice = orchestrator_message(
                format!("orch-warmup-queued:{exec_id}:{attempt_no}"),
                format!(
                    "{ahead} task worker(s) are already compiling on this host. This worker's \
                     first build is queued and starts as soon as one finishes; the worker \
                     starts once it is done."
                ),
            );
            notice.role = MessageRole::System;
            if let Err(error) = db
                .with_conn(move |conn| {
                    crate::db::discussions::insert_message(conn, &room, &notice).map(|_| ())
                })
                .await
            {
                tracing::warn!("warm-start queue notice not posted: {error}");
            }
        }
    };
    let (record, ()) = tokio::join!(warm, announce);
    let record = match record {
        Ok(record) => record,
        Err(error) => {
            tracing::warn!("lane warm start did not complete: {error}");
            return None;
        }
    };
    let id = exec_id.to_string();
    let journal = record.clone();
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::orchestration::record_lane_warm_start(conn, &id, &journal, &backend_actor())
        })
        .await
    {
        tracing::warn!("lane warm-start record not persisted: {error}");
    }
    Some(record)
}

/// How much of a failed first build's reason the room line carries.
pub const WARM_FAILURE_CAUSE_CHARS: usize = 900;

/// KT-1736 — the room line for a lane whose first build failed or timed out, or
/// `None` when the lane is warm (or was never going to be built). The worker is
/// still dispatched, on a cold lane; this says why, with the build's own reason.
pub fn warm_failure_notice(
    lang: RoomLang,
    first_build: &crate::core::task_worker_lane_warm::FirstBuild,
) -> Option<String> {
    use crate::core::task_worker_lane_warm::FirstBuild;
    let (cause_fr, cause_en) = match first_build {
        FirstBuild::Failed { detail, .. } => {
            let cause = clip_warm_failure_cause(detail);
            (cause.clone(), cause)
        }
        FirstBuild::TimedOut { duration_ms } => {
            let minutes = duration_ms / 60_000;
            (
                format!("délai dépassé après {minutes} min"),
                format!("timed out after {minutes} min"),
            )
        }
        FirstBuild::Succeeded { .. }
        | FirstBuild::Cancelled { .. }
        | FirstBuild::Skipped { .. } => return None,
    };
    Some(match lang {
        RoomLang::Fr => format!(
            "⚠️ préchauffage échoué : {cause_fr}\n\nLe worker démarre sur une lane froide : sa \
             première compilation (plusieurs dizaines de minutes) aura lieu pendant son tour."
        ),
        RoomLang::En => format!(
            "⚠️ warm-up failed: {cause_en}\n\nThe worker starts on a cold lane: its first build \
             (tens of minutes) runs during its turn."
        ),
    })
}

/// `detail` clipped to [`WARM_FAILURE_CAUSE_CHARS`] characters.
pub fn clip_warm_failure_cause(detail: &str) -> String {
    let detail = detail.trim();
    if detail.chars().count() <= WARM_FAILURE_CAUSE_CHARS {
        return detail.to_string();
    }
    let kept: String = detail.chars().take(WARM_FAILURE_CAUSE_CHARS).collect();
    format!("{kept}…")
}

/// Post the cold-lane notice in the worker's room. Never raised: a notice that is
/// not posted only leaves the cause in the journal.
async fn post_warm_failure_notice(
    db: &Database,
    exec_id: &str,
    attempt_no: u32,
    room_id: &str,
    content: String,
) {
    let mut notice = orchestrator_message(
        format!("orch-warmup-failed:{exec_id}:{attempt_no}"),
        content,
    );
    notice.role = MessageRole::System;
    let room = room_id.to_string();
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::discussions::insert_message(conn, &room, &notice).map(|_| ())
        })
        .await
    {
        tracing::warn!(execution_id = %exec_id, "warm-up failure notice not posted: {error}");
    }
}

/// Provision a Claude task worker's lane and check, with the worker's own
/// environment, what it can build and test (KT-1246). Journalled on the
/// execution; a check that cannot run is recorded as unverified, never raised.
pub async fn record_toolchain_smoke_check(
    db: &Database,
    exec_id: &str,
    worktree: &std::path::Path,
    runner: std::sync::Arc<dyn crate::core::task_worker_env::ProbeRunner>,
) -> ToolchainCapabilities {
    record_toolchain_smoke_check_on(db, exec_id, worktree, runner, None).await
}

/// [`record_toolchain_smoke_check`] for a lane the warm-skip rule left cold
/// (KT-1980): the Rust capability says the target is cold and why, in the record
/// and so in the brief, so the worker does not start a backend build by accident.
pub async fn record_toolchain_smoke_check_on(
    db: &Database,
    exec_id: &str,
    worktree: &std::path::Path,
    runner: std::sync::Arc<dyn crate::core::task_worker_env::ProbeRunner>,
    cold: Option<&crate::core::task_worker_warm_skip::WarmSkip>,
) -> ToolchainCapabilities {
    let path = worktree.to_path_buf();
    let mut capabilities = tokio::task::spawn_blocking(move || {
        let loopback = crate::agents::runner::claude_task_worker_allows_loopback_binding(&path);
        crate::core::task_worker_env::check_worktree(&path, loopback, runner.as_ref())
    })
    .await
    .unwrap_or_else(|error| {
        ToolchainCapabilities::unverified(&format!("the smoke check did not complete: {error}"))
    });
    if let Some(skip) = cold {
        let note = skip.cold_target_note();
        let reason = &mut capabilities.rust_tests.reason;
        *reason = if reason.trim().is_empty() {
            note
        } else {
            format!("{reason}. {note}")
        };
    }
    let exec_id = exec_id.to_string();
    let record = capabilities.clone();
    if let Err(error) = db
        .with_conn(move |conn| {
            crate::db::orchestration::record_toolchain_capabilities(
                conn,
                &exec_id,
                &record,
                &backend_actor(),
            )
        })
        .await
    {
        tracing::warn!("toolchain capability record not persisted: {error}");
    }
    capabilities
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
    lang: RoomLang,
) -> DiscussionMessage {
    let content = worker_brief_markdown_in(
        lang,
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
        project_signs_off(worktree_path),
    );
    orchestrator_message(format!("orch-brief:{exec_id}:{attempt_no}"), content)
}

/// The durable worker-activation notice posted in the origin room at acceptance.
/// The session remains a member of that room; its additional worker authority is
/// scoped to the execution and child task room named by the notice.
pub fn build_attach_notice(
    exec_id: &str,
    attempt_no: u32,
    task_reference: &str,
    task_title: &str,
    child_discussion_id: &str,
    worker_alias: Option<&str>,
    lang: RoomLang,
) -> DiscussionMessage {
    let content = match lang {
        RoomLang::Fr => {
            let who = worker_alias
                .map(|a| format!("La session worker `{a}`"))
                .unwrap_or_else(|| "La session worker".to_string());
            format!(
                "{who} a accepté **{task_reference} : {task_title}** comme worker de la \
                 sous-discussion `{child_discussion_id}`. Elle reste présente dans cette room \
                 principale ; son rôle worker est limité à cette exécution."
            )
        }
        RoomLang::En => {
            let who = worker_alias
                .map(|a| format!("The worker session `{a}`"))
                .unwrap_or_else(|| "The worker session".to_string());
            format!(
                "{who} accepted **{task_reference}: {task_title}** as worker of the \
                 sub-discussion `{child_discussion_id}`. It stays present in this main room; \
                 its worker role is limited to this execution."
            )
        }
    };
    orchestrator_message(format!("orch-attach:{exec_id}:{attempt_no}"), content)
}

/// The principal-targeted review request posted in the PARENT room when a worker delivers
/// (KT-319 DoD-3). NOT the work brief: it tells the principal a delivery awaits its review
/// and names the two possible decisions. Deterministic id per `(exec, attempt)` so a resume
/// never double-posts (the message PK rejects it).
#[allow(clippy::too_many_arguments)]
pub fn build_review_request_message(
    exec_id: &str,
    attempt_no: u32,
    task_reference: &str,
    task_title: &str,
    child_discussion_id: &str,
    head_sha: &str,
    summary: &str,
    provider_changes: &[crate::db::orchestration::ProviderChange],
    lang: RoomLang,
) -> DiscussionMessage {
    let mut content = match lang {
        RoomLang::Fr => format!(
            "**Revue demandée — {task_reference} : {task_title}**\n\n\
             Le worker a livré son travail (HEAD `{head_sha}`) dans la sous-discussion \
             `{child_discussion_id}` et soumis un DeliveryManifest. Décide via \
             `task_exec_review` :\n\
             - `approve` — le travail est intégré (refusé si le HEAD a dérivé depuis la \
             livraison — le manifeste épingle l'état exact revu) ;\n\
             - `request_changes` — renvoyé au worker avec un `comment` actionnable.\n\n\
             Résumé du worker : {summary}"
        ),
        RoomLang::En => format!(
            "**Review requested — {task_reference}: {task_title}**\n\n\
             The worker delivered its work (HEAD `{head_sha}`) in the sub-discussion \
             `{child_discussion_id}` and submitted a DeliveryManifest. Decide through \
             `task_exec_review`:\n\
             - `approve` — the work is integrated (refused if HEAD drifted since the \
             delivery — the manifest pins the exact state reviewed);\n\
             - `request_changes` — sent back to the worker with an actionable `comment`.\n\n\
             Worker summary: {summary}"
        ),
    };
    // KT-1564 — a delivery made after a provider switch says so where the
    // reviewer decides, not only in the report written once it is accepted.
    for change in provider_changes {
        let reason = change.reason.trim();
        content.push_str(&match lang {
            RoomLang::Fr => format!(
                "\n\n⚠️ Repli de provider : l'exécution est passée de `{}` à `{}` ({}).",
                change.from,
                change.to,
                if reason.is_empty() {
                    "aucune raison enregistrée"
                } else {
                    reason
                }
            ),
            RoomLang::En => format!(
                "\n\n⚠️ Provider fallback: the execution moved from `{}` to `{}` ({}).",
                change.from,
                change.to,
                if reason.is_empty() {
                    "no reason recorded"
                } else {
                    reason
                }
            ),
        });
    }
    orchestrator_message(
        format!("orch-review-request:{exec_id}:{attempt_no}"),
        content,
    )
}

/// KT-1654 — a Rust change is delivered without a passing `clippy --all-targets`
/// entry: the CI gate is `cargo clippy --all-targets -- -D warnings`, and a
/// `--lib` / `--lib --tests` run never lints `tests/` (KT-1608 broke clippy that
/// way). Non-blocking: the delivery is accepted and the principal reads this on
/// the review request.
pub fn delivery_lint_gate_warning(
    manifest: &crate::models::DeliveryManifestV1,
    lang: RoomLang,
) -> Option<String> {
    let rust_change = manifest.files_touched.iter().any(|file| {
        file.path.ends_with(".rs")
            || file.path.ends_with("Cargo.toml")
            || file.path.ends_with("Cargo.lock")
    });
    if !rust_change {
        return None;
    }
    let linted = manifest.tests.iter().any(|test| {
        let haystack = format!("{} {}", test.name, test.evidence.as_deref().unwrap_or(""));
        test.status == crate::models::TestStatus::Pass
            && haystack.contains("clippy")
            && haystack.contains("--all-targets")
    });
    if linted {
        return None;
    }
    Some(match lang {
        RoomLang::Fr => "⚠️ Gate de lint : cette livraison touche du Rust mais aucune validation \
             `clippy --all-targets` au statut `pass` n'y figure. Le gate de la CI est `cargo \
             clippy --all-targets -- -D warnings` (`--lib` ou `--lib --tests` ne lintent pas \
             `tests/`) : lance-le avant d'approuver."
            .to_string(),
        RoomLang::En => "⚠️ Lint gate: this delivery touches Rust but lists no passing \
             `clippy --all-targets` validation. The CI gate is `cargo clippy --all-targets -- -D \
             warnings` (`--lib` or `--lib --tests` do not lint `tests/`): run it before approving."
            .to_string(),
    })
}

/// The escalation solicitation posted to the PRINCIPAL in the parent room when the review budget
/// is exhausted (KT-319 DoD-6). It tells the principal the run has paused in `Escalated` and names
/// the human choices — it does NOT re-offer. Deterministic id per `(exec, attempt)` so a resume
/// never double-posts.
#[allow(clippy::too_many_arguments)]
pub fn build_escalation_message(
    exec_id: &str,
    attempt_no: u32,
    task_reference: &str,
    task_title: &str,
    review_rounds: u32,
    max_review_rounds: u32,
    child_discussion_id: &str,
    lang: RoomLang,
) -> DiscussionMessage {
    let content = match lang {
        RoomLang::Fr => format!(
            "**Escalade — budget de revue épuisé ({review_rounds}/{max_review_rounds}) — \
             {task_reference} : {task_title}**\n\n\
             Le worker vient de recevoir `request_changes` et le nombre de tours de revue \
             autorisés est atteint. L'exécution est mise en pause en `Escalated` : elle ne \
             sera **pas** ré-offerte automatiquement — une **décision humaine** est requise. \
             Options :\n\
             - reprendre la main sur la sous-discussion `{child_discussion_id}` (piloter le \
             worker directement) ;\n\
             - rallonger la limite de tours de cette exécution : `task_exec_reassign` avec \
             `extend_review_rounds` (nombre de tours à ajouter, ou `unlimited`) la relève et \
             renvoie les points au worker ;\n\
             - basculer sur un worker natif, ou abandonner l'exécution."
        ),
        RoomLang::En => format!(
            "**Escalation — review budget exhausted ({review_rounds}/{max_review_rounds}) — \
             {task_reference}: {task_title}**\n\n\
             The worker has just received `request_changes` and the number of allowed review \
             rounds is reached. The execution is paused in `Escalated`: it will **not** be \
             re-offered automatically — a **human decision** is required. Options:\n\
             - take over the sub-discussion `{child_discussion_id}` (drive the worker \
             directly);\n\
             - extend this execution's round limit: `task_exec_reassign` with \
             `extend_review_rounds` (rounds to add, or `unlimited`) raises it and hands the \
             findings back to the worker;\n\
             - switch to a native worker, or abandon the execution."
        ),
    };
    orchestrator_message(format!("orch-escalation:{exec_id}:{attempt_no}"), content)
}

/// The solicitation posted to the PRINCIPAL when a `request_changes` finds the campaign's
/// token budget spent (KT-1345). Same pause as [`build_escalation_message`], but the cause is
/// the token budget, so raising the review-round limit would not help and is not offered.
fn build_token_budget_escalation_message(
    exec_id: &str,
    attempt_no: u32,
    task_reference: &str,
    task_title: &str,
    child_discussion_id: &str,
    lang: RoomLang,
) -> DiscussionMessage {
    let content = match lang {
        RoomLang::Fr => format!(
            "**Escalade — budget de tokens de la campagne épuisé — \
             {task_reference} : {task_title}**\n\n\
             Le worker vient de recevoir `request_changes`, mais le budget de tokens de la \
             campagne est consommé : aucune nouvelle tentative n'est planifiée, quelle que soit \
             la limite de tours de revue. L'exécution est en pause en `Escalated` \
             (`token_budget_exhausted`) — une **décision humaine** est requise. Options :\n\
             - reprendre la main sur la sous-discussion `{child_discussion_id}` (corriger \
             directement) ;\n\
             - abandonner l'exécution."
        ),
        RoomLang::En => format!(
            "**Escalation — campaign token budget exhausted — {task_reference}: {task_title}**\n\n\
             The worker has just received `request_changes`, but the campaign's token budget is \
             spent: no further attempt is scheduled, whatever the review-round limit. The \
             execution is paused in `Escalated` (`token_budget_exhausted`) — a **human \
             decision** is required. Options:\n\
             - take over the sub-discussion `{child_discussion_id}` (fix it directly);\n\
             - abandon the execution."
        ),
    };
    orchestrator_message(
        format!("orch-escalation-tokens:{exec_id}:{attempt_no}"),
        content,
    )
}

/// The validation gate of a project that builds with Cargo (Kronn's own CI gate).
const RUST_GATE_EN: &str = "The default validations are fmt + clippy + targeted lib tests (`cargo test --lib <filter>`): the full suite is owned by the PR's CI, never by a local run. For a Rust change the lint gate is EXACTLY `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` (the CI gate: `--lib`, `--lib --tests` or a filtered output do not lint `tests/`); report each exit code in the evidence of its `tests` entry.";
const RUST_GATE_FR: &str = "Les validations par défaut sont fmt + clippy + tests lib ciblés (`cargo test --lib <filtre>`) : la suite complète appartient à la CI de la PR, jamais à un run local. Pour un changement Rust, le gate de lint est EXACTEMENT `cargo fmt --check` et `cargo clippy --all-targets -- -D warnings` (le gate de la CI : `--lib`, `--lib --tests` ou une sortie filtrée ne lintent pas `tests/`) ; reporte le code de sortie de chacun dans la preuve de son entrée `tests`.";
/// The gate of any other project: its own formatter, linter and tests.
const GENERIC_GATE_EN: &str = "The default validations are the project's own formatter, linter and targeted tests (see the project conventions): the full suite is owned by the PR's CI, never by a local run. Report each command you ran and its exit code in the evidence of its `tests` entry.";
const GENERIC_GATE_FR: &str = "Les validations par défaut sont le formateur, le linter et les tests ciblés propres au projet (voir les conventions du projet) : la suite complète appartient à la CI de la PR, jamais à un run local. Reporte chaque commande lancée et son code de sortie dans la preuve de son entrée `tests`.";

/// Whether the project of the worktree signs its commits off (KT-1750): what the
/// worker brief names, so it never asks for a trailer the project does not use.
fn project_signs_off(worktree_path: &str) -> bool {
    crate::core::commit_policy::CommitPolicy::for_repo(std::path::Path::new(worktree_path)).signoff
}

/// The French brief, which the existing brief tests pin.
#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::too_many_arguments)]
pub fn worker_brief_markdown(
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
    worker_brief_markdown_in(
        RoomLang::Fr,
        task_reference,
        task_title,
        task_description,
        dod,
        worktree_path,
        branch,
        base_sha,
        can_run_shell,
        native_delivery_projection,
        worker_scope,
        true,
    )
}

/// Shared worker-brief markdown (KT-318 DoD-6: objective, DoD, decisions/scope,
/// constraints, tests, workspace and the concrete DeliveryManifest v1 delivery
/// format) — the focused context, not the whole plan. Native and CLI briefs both
/// render through this so their lifecycle contract stays aligned while the
/// concrete method names the tools each transport actually exposes.
/// Written in the room's language (KT-1316).
#[allow(clippy::too_many_arguments)]
pub fn worker_brief_markdown_in(
    lang: RoomLang,
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
    signoff: bool,
) -> String {
    let fr = lang == RoomLang::Fr;
    let mediated_host_commit = can_run_shell && native_delivery_projection;
    // KT-1750 — the sign-off is named only where the project signs off.
    let commit_cmd = if signoff {
        "git commit -s"
    } else {
        "git commit"
    };
    let merge_cmd = if signoff {
        "git merge --signoff"
    } else {
        "git merge"
    };
    let finish_merge_cmd = if signoff {
        "git commit -s --no-edit"
    } else {
        "git commit --no-edit"
    };
    let en_and_signoff = if signoff { " and the sign-off" } else { "" };
    let fr_and_signoff = if signoff { " et le sign-off" } else { "" };
    let fr_clean_commit = format!("Crée un commit propre avec `{commit_cmd}` avant la livraison.");
    let en_clean_commit = format!("Create a clean commit with `{commit_cmd}` before delivery.");
    let en_identity = if signoff {
        "it signs with the repository's git identity"
    } else {
        "it commits as the repository's git identity, without a sign-off (this project does not use one)"
    };
    let fr_identity = if signoff {
        "il signe avec l'identité git du dépôt"
    } else {
        "il commite avec l'identité git du dépôt, sans sign-off (ce projet n'en utilise pas)"
    };
    // A joined CLI retains the room tools exposed by the regular bridge. Spawned
    // native workers and HTTP workers receive their narrower task/workspace
    // surfaces, so their briefs must request a relay instead of naming a tool
    // they cannot call.
    let has_room_publication = can_run_shell && !native_delivery_projection;
    let human_arbitration = if has_room_publication && !fr {
        "## Human arbitration and parent milestones\n\
         - A normal implementation choice, compatible with the objective and the DoD, is the \
           worker's to make. A product decision that changes the need, the scope or a user \
           trade-off is reserved to the human.\n\
         - Before asking for a product decision, call `disc_question_list` and reuse any \
           applicable durable human answer; do not ask again for an arbitration already made.\n\
         - Without an applicable answer, publish through `disc_append` a closed `kronn-question` \
           card with `\"version\":1` (a number), a stable key and the blocking question. Then read \
           that key back with `disc_question_list` to verify it was recorded.\n\
         - Meanwhile, pause only the affected scope and continue with independent work only. \
           Neither the worker nor the principal ever decides in the human's place.\n\n"
            .to_string()
    } else if !has_room_publication && !fr {
        "## Human arbitration and parent milestones\n\
         - A normal implementation choice, compatible with the objective and the DoD, is the \
           worker's to make. A product decision that changes the need, the scope or a user \
           trade-off is reserved to the human.\n\
         - This surface declares neither `disc_question_list` nor `disc_append`. Reuse any \
           durable human answer provided in the context; without an applicable answer, state \
           the exact blocker and ask the principal to relay a valid `kronn-question` card.\n\
         - While waiting for the durable answer, pause only the affected scope. Neither the \
           worker nor the principal ever decides in the human's place.\n\n"
            .to_string()
    } else if has_room_publication {
        "## Arbitrages humains et jalons parent\n\
         - Un choix normal d'implémentation, compatible avec l'objectif et la DoD, relève du worker. \
           Une décision produit qui change le besoin, le périmètre ou un compromis utilisateur est \
           réservée à l'humain.\n\
         - Avant de solliciter une décision produit, appelle `disc_question_list` et réutilise une \
           réponse humaine durable applicable ; ne redemande pas un arbitrage déjà tranché.\n\
         - Sans réponse applicable, publie via `disc_append` une carte fermée `kronn-question` \
           avec `\"version\":1` (nombre), une clé stable et la question bloquante. Relis ensuite \
           cette clé avec `disc_question_list` pour vérifier l'enregistrement.\n\
         - En attendant, pause ce seul périmètre et poursuis uniquement le travail indépendant. \
           Ni le worker ni le principal ne décide jamais à la place de l'humain.\n\n"
            .to_string()
    } else {
        "## Arbitrages humains et jalons parent\n\
         - Un choix normal d'implémentation, compatible avec l'objectif et la DoD, relève du worker. \
           Une décision produit qui change le besoin, le périmètre ou un compromis utilisateur est \
           réservée à l'humain.\n\
         - Cette surface ne déclare pas `disc_question_list` ni `disc_append`. Réutilise toute \
           réponse humaine durable fournie dans le contexte ; sans réponse applicable, indique le \
           blocker exact et demande au principal de relayer une carte `kronn-question` valide.\n\
         - En attendant la réponse durable, pause ce seul périmètre. Ni le worker ni le principal \
           ne décide jamais à la place de l'humain.\n\n"
            .to_string()
    };
    // `disc_append` accepts an explicit `disc_id` (backend/scripts/disc-introspection-mcp.py:5122),
    // so a joined CLI can target the real parent via `task_exec_status`'s `parent_discussion_id`.
    // `disc_question_list` truly is bound-only (backend/scripts/disc-introspection-mcp.py:4298-4309).
    // Native/HTTP workers declare neither tool.
    let parent_milestones = if has_room_publication && !fr {
        "## Factual parent milestones\n\
         `disc_append` targets by default the sub-discussion this session was re-attached to \
         on acceptance, but accepts an explicit `disc_id`: call `task_exec_status` to get \
         `parent_discussion_id`, the real and authorized id of the parent room, and publish a \
         factual milestone there directly (notable progress, blocker, result) when there is one \
         — no automatic or systematic mirroring. `disc_question_list` stays bound to this \
         sub-discussion whatever argument is passed: it never gives access to the questions \
         pending in the parent room. The milestones already visible without any action from you \
         remain the attachment recorded by the orchestrator on acceptance and, after a valid \
         DeliveryManifest, the review request it creates; they are not the only possible \
         milestones. If a human decision must reach the parent room and you cannot or must not \
         publish it yourself, explicitly ask the principal to relay it.\n\n"
            .to_string()
    } else if !fr {
        "## Factual parent milestones\n\
         This surface declares no message publication tool (neither `disc_append` nor \
         `disc_question_list`). The only automatic parent milestones are the attachment recorded \
         by the orchestrator and, after a valid DeliveryManifest, the review request. For any \
         other notable verified fact (progress, blocker, result), report it in your available \
         output and ask the principal to relay it to the parent room, not only for a human \
         decision; no automatic visibility or guaranteed delivery, no automatic mirroring, no \
         invented tool.\n\n"
            .to_string()
    } else if has_room_publication {
        "## Jalons parent factuels\n\
         `disc_append` cible par défaut la sous-discussion où cette session a été rebranchée \
         à l'acceptation, mais accepte un `disc_id` explicite : appelle `task_exec_status` pour \
         obtenir `parent_discussion_id`, l'identifiant réel et autorisé de la room parente, et \
         publie-y directement un jalon factuel (avancement notable, blocker, résultat) quand \
         c'en est un — pas de miroir automatique ni systématique. `disc_question_list` reste \
         borné à cette seule sous-discussion quel que soit l'argument passé : il ne donne jamais \
         accès aux questions en attente dans la room parente. Les jalons déjà visibles sans \
         action de ta part restent l'attachement enregistré par l'orchestrateur à l'acceptation \
         et, après un DeliveryManifest validé, la demande de revue qu'il crée ; ce ne sont pas \
         les seuls jalons possibles. Si une décision humaine doit atteindre la room parente et \
         que tu ne peux ou ne dois pas la publier toi-même, demande explicitement au principal \
         de relayer.\n\n"
            .to_string()
    } else {
        "## Jalons parent factuels\n\
         Cette surface ne déclare aucun outil de publication de message (ni `disc_append` \
         ni `disc_question_list`). Les seuls jalons parent automatiques sont l'attachement \
         enregistré par l'orchestrateur et, après un DeliveryManifest validé, la demande de \
         revue. Pour tout autre fait vérifié notable (avancement, blocker, résultat), \
         signale-le dans ta sortie disponible et demande au principal de le relayer vers la \
         room parente, pas seulement pour une décision humaine ; ni visibilité automatique \
         ni livraison garantie, pas de miroir automatique, ni d'outil inventé.\n\n"
            .to_string()
    };
    let dod = if dod.is_empty() {
        lang.pick("_(aucune)_", "_(none)_").to_string()
    } else {
        // KT-1254 — a live check is made by the principal AFTER integration:
        // the worker declares it unmet (`met: false`) with what it could not prove.
        let live = |item: &PlanningDodItem| {
            if item.verification == PlanningDodVerification::PostIntegration {
                lang.pick(
                    " _(vérification en direct par le principal APRÈS intégration : ne la \
                     prouve pas, déclare `met: false` et dis ce qui reste à vérifier)_",
                    " _(live check made by the principal AFTER integration: do not prove \
                     it, declare `met: false` and say what remains to be verified)_",
                )
            } else {
                ""
            }
        };
        if native_delivery_projection {
            dod.iter()
                .enumerate()
                .map(|(index, item)| format!("{}. [ ] {}{}", index + 1, item.sentence, live(item)))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            dod.iter()
                .map(|item| format!("- [ ] `{}` — {}{}", item.id, item.sentence, live(item)))
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
                    if fr {
                        format!(
                            "- Fichier : `{path}`\n- Plage inclusive : `{start_line}..={end_line}`"
                        )
                    } else {
                        format!("- File: `{path}`\n- Inclusive range: `{start_line}..={end_line}`")
                    },
                    "edit_lines",
                    lang.pick(
                        "remplacer uniquement la plage gelée",
                        "replace only the frozen range",
                    ),
                ),
                TaskWorkerScope::PrelocalizedInsertAfter { path, anchor_line } => (
                    if fr {
                        format!(
                            "- Fichier : `{path}`\n- Insertion après la ligne d'ancre : `{anchor_line}`"
                        )
                    } else {
                        format!(
                            "- File: `{path}`\n- Insertion after the anchor line: `{anchor_line}`"
                        )
                    },
                    "insert_after_line",
                    lang.pick(
                        "fournir uniquement le nouveau texte ; l'ancre est préservée mécaniquement",
                        "provide only the new text; the anchor is preserved mechanically",
                    ),
                ),
            };
            let delivery = match (native_delivery_projection, fr) {
                (true, true) => {
                    "Dans `task_exec_deliver`, fournis uniquement `tests`, `dod_status` dans l'ordre, \
                     `docs`, `migrations`, `risks`, `limitations` et `summary`. Kronn injecte les \
                     identifiants, le SHA et l'inventaire depuis le worktree réel."
                }
                (true, false) => {
                    "In `task_exec_deliver`, provide only `tests`, `dod_status` in order, `docs`, \
                     `migrations`, `risks`, `limitations` and `summary`. Kronn injects the ids, the \
                     SHA and the inventory from the real worktree."
                }
                (false, true) => {
                    "Dans `task_exec_deliver`, remplis le DeliveryManifest v1 complet selon le schéma \
                     déclaré par l'outil."
                }
                (false, false) => {
                    "In `task_exec_deliver`, fill in the complete DeliveryManifest v1 according to \
                     the schema the tool declares."
                }
            };
            if !fr {
                return format!(
                    "# {task_reference} — {task_title}\n\n\
                     ## Objective\n{objective}\n\n\
                     ## Definition of Done\n{dod}\n\n\
                     ## Prelocalized mechanical target\n{target}\n\n\
                     ## Bounded protocol\n\
                     1. Call the single constrained `read_file`.\n\
                     2. Kronn permanently removes the read tools; call the single \
                        `{mutation_tool}` with the `content_sha256` received to {mutation_instruction}.\n\
                     3. Call `git_commit` with that single file.\n\
                     4. Call `task_exec_deliver`.\n\n\
                     Do not search elsewhere, do not re-read and never widen the target. Kronn exposes \
                     a single constrained `read_file`. If the window is not enough, state the exact \
                     blocker: not a single byte may be guessed. You have no shell: do not claim any \
                     validation was run; report it `skipped` with the command the principal will have \
                     to run. No push, merge or force-push.\n\n\
                     {delivery}\n\n\
                     Start now with `read_file`; the fresh worktree and the target are already \
                     established, there is nothing to inventory."
                );
            }
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
    // KT-839: a worker was pushed to "run the validations", ran a full suite in the
    // background because its shell tool cuts a command at 600 s, handed the turn back
    // to wait for it, and its process ended WITHOUT delivering
    // (`worker_completed_without_delivery`, twice on KT-847). The long gates the
    // principal persisted are Kronn's to run at integration; the worker's job is the
    // targeted tests, then commit and deliver in the same turn.
    // KT-1751 — the cargo lint gate is for a project that builds with Cargo. Any
    // other project, one whose toolchain Kronn does not know included (Python,
    // Go, Gradle), is told to use its own formatter, linter and tests.
    let worktree_dir = std::path::Path::new(worktree_path);
    let toolchains = crate::core::project_toolchains::detect_toolchains(worktree_dir);
    let cargo_gate = toolchains
        .iter()
        .any(|root| root.kind == crate::core::project_toolchains::ToolchainKind::Cargo);
    let tests = if can_run_shell && !fr {
        format!(
            "Run the **targeted** tests that cover your change and report each result \
         (`pass`/`fail`/`skipped` + non-empty evidence) in `tests`. A `pass` without a \
         verifiable command or output is refused.\n\
         {gate} The long \
         validations persisted by the principal are run by Kronn at integration, after the \
         review: do not replay them in full. The shell tool \
         cuts a command at the per-command cap of Settings > Workers (60 min by default). A cold \
         test-profile build can take many minutes: run it in the foreground and pass the Bash \
         tool an explicit `timeout` up to that cap, never cut it short and never restart it in \
         a loop. Never end your turn while a command runs in the background, and never start \
         one to wait for it later: your process ends as soon as you hand the turn back, takes \
         the build with it, and the execution ends without a delivery; if you must background \
         a command, wait for it in the same turn. Chain it all in the same turn: targeted \
         tests, commit, then `task_exec_deliver`.",
            gate = if cargo_gate {
                RUST_GATE_EN
            } else {
                GENERIC_GATE_EN
            }
        )
    } else if !fr {
        "You have no shell: never claim to have run `cargo`, `pnpm`, `make` or any other \
         command. You may write the tests that are needed, then report the commands the \
         principal must run with `status: skipped` and a non-empty reason. The principal will \
         run these validations before approval and attach its evidence to the \
         `ReviewDecision`."
            .to_string()
    } else if can_run_shell {
        format!(
            "Lance les tests **ciblés** qui couvrent ton changement et reporte chaque résultat \
         (`pass`/`fail`/`skipped` + preuve non vide) dans `tests`. Un `pass` sans \
         commande ou sortie vérifiable est refusé.\n\
         {gate} Les validations longues persistées par le principal sont \
         jouées par Kronn à l'intégration, après la revue : ne les rejoue pas en entier. \
         L'outil shell coupe une commande au plafond par commande de Settings > Workers \
         (60 min par défaut). Un build à froid du profil de test peut durer de nombreuses \
         minutes : lance-le au premier plan en passant à l'outil Bash un `timeout` explicite \
         jusqu'à ce plafond, ne l'interromps jamais et ne le relance pas en boucle. Ne \
         termine jamais ton tour pendant qu'une commande tourne en arrière-plan et n'en lance \
         jamais une pour l'attendre plus tard : ton processus s'arrête dès que tu rends la \
         main, emporte le build, et l'exécution finit sans livraison ; si tu dois mettre une \
         commande en arrière-plan, attends sa fin dans le même tour. Enchaîne dans le même \
         tour : tests ciblés, commit, puis `task_exec_deliver`.",
            gate = if cargo_gate {
                RUST_GATE_FR
            } else {
                GENERIC_GATE_FR
            }
        )
    } else {
        "Tu n'as pas de shell : n'affirme jamais avoir exécuté `cargo`, `pnpm`, `make` \
         ou une autre commande. Tu peux écrire les tests nécessaires, puis reporte les \
         commandes que le principal doit exécuter avec `status: skipped` et une raison \
         non vide. Le principal exécutera ces validations avant l'approbation et joindra \
         sa preuve au `ReviewDecision`."
            .to_string()
    };
    let french_scope = match worker_scope {
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
                 puis édite dès que tu sais quoi changer. Exécute les tests ciblés (section Tests). {}",
                if mediated_host_commit {
                    "Ne lance pas `git commit` dans le shell : appelle `task_exec_commit` avec \
                     uniquement les fichiers explicites et le message. Kronn possède seul \
                     l'accès Git administratif."
                } else {
                    &fr_clean_commit
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
    let (method, first_action, mechanical_scope) = if fr {
        french_scope
    } else {
        match worker_scope {
            Some(TaskWorkerScope::PrelocalizedEdit {
                path,
                start_line,
                end_line,
            }) => (
                format!(
                    "This execution is mechanically prelocalized. Kronn first exposes a single \
                     `read_file` constrained to the frozen window around `{path}:{start_line}-{end_line}`, \
                     then permanently removes the read tools and exposes only `edit_lines` \
                     on that same range. Do not search elsewhere, do not re-read, and change \
                     neither the file nor the range. If the window is not enough to produce \
                     a safe edit, state the exact blocker: Kronn will hand control back to the \
                     principal. After an accepted edit, commit then deliver."
                ),
                "First call: use the single declared `read_file` with its already constrained \
                 arguments. On the next turn, immediately call the single declared `edit_lines` \
                 with the fresh `content_sha256` received."
                    .to_string(),
                format!(
                    "## Prelocalized mechanical target\n- File: `{path}`\n- Inclusive range: \
                     `{start_line}..={end_line}`\n- The target is persisted and cannot be \
                     widened by the worker; the hash comes from the real read of the worktree.\n\n"
                ),
            ),
            Some(TaskWorkerScope::PrelocalizedInsertAfter { path, anchor_line }) => (
                format!(
                    "This execution is mechanically prelocalized for an insertion. Kronn first \
                     exposes a single `read_file` constrained to the frozen window around \
                     `{path}:{anchor_line}`, then permanently removes the read tools and \
                     exposes only `insert_after_line` on that anchor. Do not search elsewhere, \
                     do not re-read and change neither the file nor the anchor. The tool \
                     mechanically preserves the anchor line: provide only the text to add. If the \
                     window is not enough to produce a safe insertion, state the exact blocker. \
                     After an accepted insertion, commit then deliver."
                ),
                "First call: use the single declared `read_file` with its already constrained \
                 arguments. On the next turn, immediately call the single declared \
                 `insert_after_line` with the fresh `content_sha256` received and only the text \
                 to add."
                    .to_string(),
                format!(
                    "## Prelocalized mechanical target\n- File: `{path}`\n- Insertion after \
                     the anchor line: `{anchor_line}`\n- The target is persisted, the anchor line \
                     is preserved mechanically and the hash comes from the real read of the \
                     worktree.\n\n"
                ),
            ),
            None if can_run_shell => (
                format!(
                    "Use your CLI's native tools (search, targeted reading, editing and shell). \
                     First search for the symbol cited in the objective, read only the useful \
                     region, then edit as soon as you know what to change. Run the targeted tests \
                     (Tests section). {}",
                    if mediated_host_commit {
                        "Do not run `git commit` in the shell: call `task_exec_commit` with only \
                         the explicit files and the message. Kronn alone owns administrative Git \
                         access."
                    } else {
                        &en_clean_commit
                    }
                ),
                "Search for the symbol cited in the objective with your CLI's native tools, \
                 read only the region found, then implement."
                    .to_string(),
                String::new(),
            ),
            None => (
                "The shortest path, whatever the size of the file: \
                 `search_text` to find the exact spot (file + line), \
                 `read_file` with `offset` and `limit` to read that region only, \
                 `edit_lines` with `start_line`, `end_line` and the `content_sha256` received \
                 to replace that range without copying an anchor, then `git_commit`, \
                 then deliver. If the hash drifted, re-read: Kronn refuses the mutation. \
                 As soon as you know what to change, make the edit: re-reading a region \
                 already read produces nothing new."
                    .to_string(),
                "First call: `search_text` on the symbol cited in the objective. \
                 Then read ONLY the region found, and make the edit with \
                 `edit_lines` as soon as you know what to change — reading the file \
                 continuously, slice after slice, burns your turn without producing anything."
                    .to_string(),
                String::new(),
            ),
        }
    };
    let delivery_format = if !native_delivery_projection && !fr {
        format!(
            "Required fields:\n\
             - `version`: `\"1\"`\n\
             - `task_ref`: `{task_reference}`\n\
             - `head_sha`: the exact HEAD delivered (SHA of the branch's last commit)\n\
             - `files_touched`: list of `{{ path, kind: added|modified|deleted }}`\n\
             - `tests`: list of `{{ name, status: pass|fail|skipped, evidence }}`\n\
             - `dod_status`: one `{{ dod_id, met, evidence }}` per DoD item\n\
             - `docs`, `migrations`, `risks`, `limitations`: lists (put `[]` explicitly if not applicable)\n\
             - `summary`: short summary of the changes"
        )
    } else if !fr {
        "You provide only the following semantic assertions:\n\
         - `tests`: list of `{ name, status: pass|fail|skipped, evidence }`\n\
         - `dod_status`: exactly one `{ met, evidence }` per DoD item, in the order above\n\
         - `docs`, `migrations`, `risks`, `limitations`: lists (put `[]` explicitly if not applicable)\n\
         - `summary`: short summary of the changes\n\
         - `tests_executed`: `{ executed: bool, which: [commands] }` — mandatory; \
           `{ executed: false, which: [] }` if no test actually ran\n\
         Do not invent or copy any UUID, SHA or inventory: after authorization, Kronn itself injects `version`, `task_ref`, `head_sha`, `files_touched` and each `dod_id` from the task and the exact worktree."
            .to_string()
    } else if !native_delivery_projection {
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
         - `tests_executed` : `{ executed: bool, which: [commandes] }` — obligatoire ; \
           `{ executed: false, which: [] }` si aucun test n'a réellement tourné\n\
         N'invente et ne recopie aucun UUID, SHA ou inventaire : après autorisation, Kronn injecte lui-même `version`, `task_ref`, `head_sha`, `files_touched` et chaque `dod_id` depuis la tâche et le worktree exact."
            .to_string()
    };
    let commit_boundary = if !fr && mediated_host_commit {
        "Before delivery, call `task_exec_commit` with only the relative paths actually \
         modified and a concise message. Do not use `git commit` in the shell: the shared \
         objects and refs are deliberately outside your sandbox."
            .to_string()
    } else if !fr && can_run_shell {
        format!(
            "Create a clean commit in this worktree before delivery, with `{commit_cmd}`: \
             {en_identity}. Never write an identity trailer \
             (`Signed-off-by`, `Co-Authored-By`…) by hand: Kronn refuses the delivery if a \
             trailer does not match that identity."
        )
    } else if !fr {
        "Before delivery, call `git_commit` with only the relative paths actually \
         modified and a concise message."
            .to_string()
    } else if mediated_host_commit {
        "Avant la livraison, appelle `task_exec_commit` avec les seuls chemins relatifs réellement \
         modifiés et un message concis. N'utilise pas `git commit` dans le shell : les objets et \
         refs partagés restent volontairement hors de ta sandbox."
            .to_string()
    } else if can_run_shell {
        format!(
            "Crée un commit propre dans ce worktree avant la livraison, avec `{commit_cmd}` : \
             {fr_identity}. N'écris jamais de trailer d'identité \
             (`Signed-off-by`, `Co-Authored-By`…) à la main : Kronn refuse la livraison si un \
             trailer ne correspond pas à cette identité."
        )
    } else {
        "Avant la livraison, appelle `git_commit` avec les seuls chemins relatifs réellement \
         modifiés et un message concis."
            .to_string()
    };
    // The one merge a worker is allowed: bringing the target branch INTO its own
    // branch when the principal asks. A worker that meets git's refusal of a
    // path-limited commit mid-merge otherwise erases MERGE_HEAD, and the
    // single-parent commit it then makes turns into add/add conflicts at
    // integration (KT-854).
    // KT-1408 — only a spawned CLI worker reads `time_limit` in the results of
    // the mediated tools; a worker that commits itself has neither of them.
    let time_limit_rule = match (mediated_host_commit, fr) {
        (false, _) => "",
        (true, false) => {
            "- The execution has a time limit. Commit as you go; when it nears, the results of \
             `task_exec_status` and `task_exec_commit` carry a `time_limit` notice — then commit \
             and deliver at once. Uncommitted work at the limit is only checkpointed (WIP), \
             never delivered.\n"
        }
        (true, true) => {
            "- L'exécution a une limite de temps. Commite au fil de l'eau ; quand elle approche, \
             les résultats de `task_exec_status` et `task_exec_commit` portent un avis \
             `time_limit` — commite et livre alors sans attendre. Le travail non commité à la \
             limite est seulement sauvegardé (WIP), jamais livré.\n"
        }
    };
    let target_integration = if !can_run_shell {
        String::new()
    } else if !fr && mediated_host_commit {
        format!(
            "## Integrate the target branch\n\
             When the principal asks you to integrate the target branch into your branch, it is the \
             only merge allowed (it goes in that direction: target → your branch, never the reverse).\n\
             1. Run `git merge --no-commit --no-ff <target>` in your worktree.\n\
             2. In case of a conflict, resolve it in the files.\n\
             3. Call `task_exec_commit` with the resolved files and a message: Kronn completes \
                the merge with both parents{en_and_signoff}. Without a conflict, name at least one \
                file the merge brings in or that you modify.\n\
             Never delete `.git/MERGE_HEAD` and run neither `git merge --abort` nor `git reset` \
             to re-commit by hand: a single-parent commit makes the target's files appear as \
             \"added by both sides\" at integration. If `task_exec_commit` refuses (unresolved \
             conflicts, foreign indexed path), it changed nothing: fix it then call it again.\n\n"
        )
    } else if !fr {
        format!(
            "## Integrate the target branch\n\
             When the principal asks you to integrate the target branch into your branch, it is the \
             only merge allowed (it goes in that direction: target → your branch, never the reverse).\n\
             1. Run `{merge_cmd} <target>` in your worktree.\n\
             2. In case of a conflict, resolve it, `git add` the resolved files, then finish with \
                `{finish_merge_cmd}` — with no path and no `-m`: git refuses a path-limited \
                commit during a merge.\n\
             Never delete `.git/MERGE_HEAD` to re-commit by hand: a single-parent commit makes the \
             target's files appear as \"added by both sides\" at integration.\n\n"
        )
    } else if mediated_host_commit {
        format!(
            "## Intégrer la branche cible\n\
             Quand le principal te demande d'intégrer la branche cible dans ta branche, c'est le \
             seul merge permis (il va dans ce sens : cible → ta branche, jamais l'inverse).\n\
             1. Lance `git merge --no-commit --no-ff <cible>` dans ton worktree.\n\
             2. En cas de conflit, résous-le dans les fichiers.\n\
             3. Appelle `task_exec_commit` avec les fichiers résolus et un message : Kronn termine \
                la fusion avec ses deux parents{fr_and_signoff}. Sans conflit, nomme au moins un \
                fichier que la fusion apporte ou que tu modifies.\n\
             N'efface jamais `.git/MERGE_HEAD` et ne lance ni `git merge --abort` ni `git reset` \
             pour recommiter à la main : un commit à un seul parent fait apparaître les fichiers de \
             la cible comme « ajoutés des deux côtés » à l'intégration. Si `task_exec_commit` \
             refuse (conflits non résolus, chemin indexé étranger), il n'a rien changé : corrige \
             puis rappelle-le.\n\n"
        )
    } else {
        format!(
            "## Intégrer la branche cible\n\
             Quand le principal te demande d'intégrer la branche cible dans ta branche, c'est le \
             seul merge permis (il va dans ce sens : cible → ta branche, jamais l'inverse).\n\
             1. Lance `{merge_cmd} <cible>` dans ton worktree.\n\
             2. En cas de conflit, résous-le, `git add` les fichiers résolus, puis termine avec \
                `{finish_merge_cmd}` — sans chemin ni `-m` : git refuse un commit limité à des \
                chemins pendant une fusion.\n\
             N'efface jamais `.git/MERGE_HEAD` pour recommiter à la main : un commit à un seul \
             parent fait apparaître les fichiers de la cible comme « ajoutés des deux côtés » à \
             l'intégration.\n\n"
        )
    };
    // KT-1669 — a lane re-anchors only the citations its own gate reads; the
    // integration settles a citation-only conflict, never the lane.
    // KT-1747 — only for a project whose docs carry `[src:]` citations.
    let citations = if !crate::core::brief_conventions::has_doc_citations(worktree_dir) {
        ""
    } else if !fr {
        "## Doc citations\n\
         Keep the `[src:]` citations of the docs you edit resolving: that is your own \
         gate. Do not re-anchor the citations of docs you did not edit, nor the anchors a merge \
         of the target moved. The integration re-anchors every citation your code shift only \
         moved, in any doc, and takes the target's side when the only conflict is citation line \
         numbers, without sending your work back. A citation your change breaks for good (its \
         symbol removed or renamed) is yours to fix: the integration sends it back with the \
         exact list.\n\n"
    } else {
        "## Citations de documentation\n\
         Garde résolubles les citations `[src:]` des docs que tu modifies : c'est ta \
         propre porte. Ne ré-ancre pas les citations des docs que tu n'as pas modifiés, ni les \
         ancres qu'une fusion de la cible a déplacées. L'intégration ré-ancre toute citation que \
         ton décalage de code a seulement déplacée, dans n'importe quel doc, et prend le côté de \
         la cible quand le seul conflit porte sur des numéros de ligne de citations, sans te \
         renvoyer ton travail. Une citation que ton changement casse pour de bon (symbole \
         supprimé ou renommé) est à toi : l'intégration te la renvoie avec la liste exacte.\n\n"
    };
    // Screen captures are asked of a project that has a web UI, not of every one.
    let ui_captures = if crate::core::brief_conventions::has_web_ui(worktree_dir, &toolchains) {
        crate::core::ui_captures::brief_section(lang)
    } else {
        ""
    };
    let conventions = crate::core::brief_conventions::section(worktree_dir, &toolchains, fr);
    if !fr {
        return format!(
            "# {task_reference} — {task_title}\n\n\
             ## Objective\n{objective}\n\n\
             ## Definition of Done\n{dod}\n\n\
             {human_arbitration}\
             {parent_milestones}\
             ## Decisions & scope\n\
             The exact scope of this task IS the Definition of Done above: touch only what \
             satisfies it, do not spill over onto other tasks or unrelated files. The applicable \
             design decisions are those already recorded in the objective and the linked design \
             documents — respect them, do not replay them.\n\n\
             ## Method\n\
             {method}\n\n\
             {mechanical_scope}\
             ## Constraints\n\
             - Work ONLY in this worktree; never touch another checkout.\n\
             - No `git push`, no force-push, no merge into the target: the protected integration \
             is done by Kronn AFTER review.\n\
             - Stay within the DoD's scope.\n\
             {time_limit_rule}\n\
             {target_integration}\
             {citations}{ui_captures}{conventions}\
             ## Tests\n\
             {tests}\n\n\
             ## Workspace\n\
             - Branch: `{branch}`\n\
             - Worktree: `{worktree_path}`\n\
             - Pinned base: `{base_sha}`\n\n\
             ## Delivery format\n\
             {commit_boundary}\n\
             When the DoD is satisfied, submit a **DeliveryManifest v1** through the \
             `task_exec_deliver` tool (the body is validated; a non-conforming manifest is \
             refused, not silently accepted).\n\
             {delivery_format}\n\
             Only report \"ready for review\" AFTER that submission: it is what wakes the \
             principal agent for the review.\n\n\
             ## Start now\n\
             This worktree is fresh: there is NO state to inventory, that is established. \
             {first_action}"
        );
    }
    format!(
        "# {reference} — {title}\n\n\
         ## Objectif\n{objective}\n\n\
         ## Definition of Done\n{dod}\n\n\
         {human_arbitration}\
         {parent_milestones}\
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
         - Pas de `git push`, pas de force-push, pas de merge vers la cible : l'intégration \
         protégée est faite par Kronn APRÈS revue.\n\
         - Reste dans le périmètre de la DoD.\n\
         {time_limit_rule}\n\
         {target_integration}\
         {citations}{ui_captures}{conventions}\
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
        human_arbitration = human_arbitration,
        parent_milestones = parent_milestones,
        target_integration = target_integration,
        citations = citations,
        delivery_format = delivery_format,
        commit_boundary = commit_boundary,
        first_action = first_action,
        branch = branch,
        worktree = worktree_path,
        base = base_sha,
    )
}

/// The language the orchestrator speaks in a campaign's rooms: the parent
/// discussion's configured language (KT-1316).
async fn room_lang(db: &Database, parent_discussion_id: &str) -> RoomLang {
    let id = parent_discussion_id.to_string();
    db.with_conn(move |conn| Ok(crate::db::discussions::room_lang(conn, &id)))
        .await
        .unwrap_or_default()
}

/// The notice a project without a commit identity gets (KT-1750): the worker's
/// commits carry the global git identity, a personal one on a repository that may
/// not be personal.
pub fn global_commit_identity_notice(lang: RoomLang, email: &str) -> String {
    match lang {
        RoomLang::Fr => format!(
            "**Identité de commit à régler** — Ce projet n'a pas d'identité de commit : Kronn \
             utilise {email}. Règle-la dans la carte du projet."
        ),
        RoomLang::En => format!(
            "**Commit identity to set** — This project has no commit identity: Kronn uses \
             {email}. Set it in the project card."
        ),
    }
}

/// Tell the worker room and the principal room, once per execution, that its
/// commits are made with the global git identity (KT-1750). The ids are
/// deterministic: a second commit, a resume or a restart cannot post it twice.
/// Commits are never blocked: the point is that it is visible.
pub async fn announce_global_commit_identity(
    db: &Database,
    execution_id: &str,
    worker_room: &str,
    principal_room: &str,
    email: &str,
) {
    let lang = room_lang(db, principal_room).await;
    for (kind, room) in [("worker", worker_room), ("principal", principal_room)] {
        let message = orchestrator_message(
            format!("orch-commit-identity:{execution_id}:{kind}"),
            global_commit_identity_notice(lang, email),
        );
        let room = room.to_string();
        let posted = db
            .with_conn(move |conn| {
                let already: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1)",
                    [&message.id],
                    |row| row.get(0),
                )?;
                if !already {
                    crate::db::discussions::insert_message(conn, &room, &message)?;
                }
                Ok(())
            })
            .await;
        if let Err(error) = posted {
            tracing::warn!(execution_id, "commit identity notice not posted: {error}");
        }
    }
}

/// An orchestrator-authored discussion message (brief, control offer, attach notice):
/// a backend-authored `User`/`Main` message with the `Orchestrateur` pseudonym. The
/// caller sets the deterministic id so a resume can never double-post.
pub fn orchestrator_message(id: String, content: String) -> DiscussionMessage {
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
        session_continuity: None,
        kind: crate::models::MessageKind::Orchestration,
    }
}

/// The CLI control-offer message posted in the ORIGIN room (NOT the work brief). Its
/// id is deterministic per `(execution, attempt)` so a resume can never double-post
/// (the PK rejects it), and it carries the opaque `offer_id` the exact session uses
/// to accept (KT-328 tranche 2).
pub fn build_control_offer_message(
    exec_id: &str,
    attempt_no: u32,
    offer_id: &str,
    task_reference: &str,
    task_title: &str,
    child_discussion_id: &str,
    lang: RoomLang,
) -> DiscussionMessage {
    let content = match lang {
        RoomLang::Fr => format!(
            "**Offre de prise en charge — {task_reference} : {task_title}**\n\n\
             Une tâche t'est proposée comme worker. Pour l'accepter sans quitter cette \
             room principale, appelle :\n\n\
             `task_exec_accept_worker_offer({{ offer_id: \"{offer_id}\" }})`\n\n\
             - Sous-discussion : `{child_discussion_id}`\n\
             - Ceci est une offre de contrôle, pas encore le brief de travail : le brief \
             est renvoyé directement par l'acceptation et conservé dans la sous-discussion."
        ),
        RoomLang::En => format!(
            "**Takeover offer — {task_reference}: {task_title}**\n\n\
             A task is offered to you as worker. To accept it without leaving this \
             main room, call:\n\n\
             `task_exec_accept_worker_offer({{ offer_id: \"{offer_id}\" }})`\n\n\
             - Sub-discussion: `{child_discussion_id}`\n\
             - This is a control offer, not yet the work brief: the brief is returned \
             directly by the acceptance and kept in the sub-discussion."
        ),
    };
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
        session_continuity: None,
        kind: crate::models::MessageKind::Orchestration,
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
    /// KT-1345 — integer or `"unlimited"`; absent = Settings > Workers.
    #[serde(default, deserialize_with = "explicit_review_rounds")]
    pub max_review_rounds: Option<serde_json::Value>,
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
    /// Explicit opt-in to a provider fallback on quota exhaustion (KT-1564).
    #[serde(default)]
    pub quota_fallback: bool,
    /// KT-1643 — who reviews each delivery. Absent: a configured remote model,
    /// else the human; never an agent nobody serves.
    #[serde(default)]
    pub reviewer: Option<crate::models::CampaignReviewer>,
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
pub fn worker_static_refusal(worker: &MessageTarget) -> Option<crate::models::CampaignTaskReason> {
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
    if matches!(
        worker.kind,
        MessageTargetKind::DiscussionAgent | MessageTargetKind::Agent
    ) {
        if let Err(reason) =
            crate::agents::runner::task_worker_route_policy(&worker.agent_type, false, true)
        {
            return Some(preparation_reason("worker_capability", reason));
        }
    }
    None
}

fn worker_project_refusal(
    worker: &MessageTarget,
    project: &crate::models::Project,
) -> Option<crate::models::CampaignTaskReason> {
    if worker.kind == MessageTargetKind::Agent && worker.agent_type == AgentType::ClaudeCode {
        return crate::agents::runner::claude_task_worker_catalogue_preflight(project)
            .err()
            .map(|detail| preparation_reason("claude_sandbox_catalogue_unsafe", detail));
    }
    None
}

/// [`worker_project_refusal`] for the launch and preparation boundaries, which
/// hold a connection: a worktree catalogue over its cap refuses naming the
/// finished worktrees a human can remove (KT-1646). Their async callers run
/// [`reclaim_finished_worktrees_for_launch`] first, so no Git runs here.
fn worker_project_refusal_naming_finished(
    conn: &rusqlite::Connection,
    worker: &MessageTarget,
    project: &crate::models::Project,
) -> Option<crate::models::CampaignTaskReason> {
    if worker.kind == MessageTargetKind::Agent && worker.agent_type == AgentType::ClaudeCode {
        return claude_catalogue_verdict(
            conn,
            project,
            crate::agents::runner::claude_task_worker_catalogue_measure,
        )
        .err()
        .map(|detail| preparation_reason("claude_sandbox_catalogue_unsafe", detail));
    }
    None
}

/// KT-515 — refuse a native/HTTP worker whose provider has an open quota
/// escalation (see `provider_has_open_quota_exhaustion`). A `cli` worker is
/// untouched: its own joined-session presence already carries that signal.
/// `exclude_exec_id` is forwarded verbatim so an execution is never refused by
/// its own open row. A retry of the provider that stopped it skips this
/// refusal altogether and re-arms instead (KT-838, `reassign_native_execution`).
pub fn worker_quota_refusal(
    conn: &rusqlite::Connection,
    worker: &MessageTarget,
    exclude_exec_id: Option<&str>,
) -> Result<Option<crate::models::CampaignTaskReason>> {
    if !matches!(
        worker.kind,
        MessageTargetKind::DiscussionAgent | MessageTargetKind::Agent
    ) {
        return Ok(None);
    }
    let provider = crate::db::orchestration::agent_type_to_db(&worker.agent_type);
    if crate::db::orchestration::provider_has_open_quota_exhaustion(
        conn,
        &provider,
        exclude_exec_id,
    )? {
        return Ok(Some(fixed_worker_reason("quota_exhausted")));
    }
    Ok(None)
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
    if let Some(reason) = worker_quota_refusal(conn, worker, None)? {
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

pub fn worker_scope_contract_refusal(
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

/// Preflight verdict on the gates the principal is about to launch with.
///
/// The same predicate the launch boundary enforces, so a `launchable: true`
/// answer cannot be followed by a launch refused for its validations.
pub fn validations_refusal(
    validations: &[crate::models::ValidationSpec],
) -> Option<crate::models::CampaignTaskReason> {
    validate_new_validation_specs(validations)
        .err()
        .map(|reason| preparation_reason("invalid_validations", reason))
}

/// KT-1636 — a validation that replays the whole Rust suite: bare
/// `cargo test` / `cargo nextest run` (no `--lib`/`--bin`/`--test`/`--doc`/…
/// target, no name filter after the subcommand or after `--`), or the
/// `make test-backend` family. Averages ~20 min locally against 3.3 min for
/// `cargo test --lib` (KT-1630 audit): in the CI gate model the PR owns it.
/// Split on whitespace exactly as the runner does; not a shell parser.
pub fn is_full_suite_validation(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    if let ["make", target, ..] = words.as_slice() {
        return matches!(*target, "test-backend" | "test-backend-cov");
    }
    let Ok(Some(cargo)) = validation_cargo_argv(command) else {
        return false;
    };
    // Global options may precede the subcommand (`cargo --offline test`).
    let Some(subcommand) = cargo.iter().position(|word| !word.starts_with('-')) else {
        return false;
    };
    let args = &cargo[subcommand + 1..];
    let args = match cargo[subcommand] {
        "test" => args,
        "nextest" => match args {
            ["run", rest @ ..] => rest,
            _ => return false,
        },
        _ => return false,
    };
    // Flags whose next word is their value, not a test-name filter.
    const VALUE_FLAGS: &[&str] = &[
        "--manifest-path",
        "--target-dir",
        "--config",
        "--features",
        "--package",
        "--profile",
        "--jobs",
        "--color",
        "--message-format",
        "--cargo-profile",
        "-p",
        "-j",
        "-F",
        "-Z",
    ];
    const NARROWING_FLAGS: &[&str] = &[
        "--lib",
        "--bin",
        "--bins",
        "--test",
        "--tests",
        "--doc",
        "--example",
        "--examples",
        "--bench",
        "--benches",
        "--no-run",
        "--list",
    ];
    let mut index = 0;
    while index < args.len() {
        let word = args[index];
        if word == "--" {
            // Anything after the separator is a filter or a harness flag; a
            // positional one narrows the run.
            return !args[index + 1..].iter().any(|rest| !rest.starts_with('-'));
        }
        if NARROWING_FLAGS.contains(&word)
            || NARROWING_FLAGS.iter().any(|flag| {
                word.strip_prefix(flag)
                    .is_some_and(|value| value.starts_with('='))
            })
        {
            return false;
        }
        if VALUE_FLAGS.contains(&word) {
            index += 2;
            continue;
        }
        if !word.starts_with('-') {
            return false; // a test-name filter
        }
        index += 1;
    }
    true
}

/// KT-1636 — non-blocking preflight note for each full-suite gate. The PR's CI
/// owns the full suite; a local integration validation is fmt + clippy +
/// targeted `--lib` tests. The gate still runs if the principal keeps it.
pub fn full_suite_validation_warnings(
    validations: &[crate::models::ValidationSpec],
) -> Vec<crate::models::CampaignTaskReason> {
    validations
        .iter()
        .filter(|spec| is_full_suite_validation(&spec.command))
        .map(|spec| {
            preparation_reason(
                "full_suite_ci_owned",
                format!(
                    "`{}` replays the whole test suite (about 20 min locally, against 3.3 min for \
                     `cargo test --lib`): the full suite belongs to the PR's CI. Prefer `cargo fmt \
                     --check`, `cargo clippy` and targeted lib tests (`cargo test --lib <filter>`) \
                     as integration validations.",
                    spec.command.trim()
                ),
            )
        })
        .collect()
}

pub fn worker_scope_refusal(
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

pub fn prepare_task_execution(
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
        .filter(|blocker| !blocker.satisfies_dependents())
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
            match crate::db::projects::get_project(conn, project_id)? {
                None => reasons.push(preparation_reason(
                    "project_missing",
                    "task project no longer exists",
                )),
                Some(project) => {
                    if let Some(reason) =
                        worker_project_refusal_naming_finished(conn, worker, &project)
                    {
                        reasons.push(reason);
                    }
                }
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
        warnings: Vec::new(),
        active_execution,
        default_max_review_rounds: None,
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
    if let Err(reason) = validate_new_validation_specs(&request.validations) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            format!("invalid validations: {reason}"),
        ));
    }
    let max_review_rounds = match review_rounds_argument(request.max_review_rounds.as_ref()) {
        Ok(Some(rounds)) => rounds,
        Ok(None) => state
            .config
            .read()
            .await
            .workers
            .effective_max_review_rounds(),
        Err(detail) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, detail)),
    };
    // A reviewer the caller names is checked now, so nothing is created for one
    // that cannot review. Without one, the room keeps its own setup and the
    // campaign records who that makes the reviewer (read inside the transaction).
    let named_reviewer = match request
        .reviewer
        .clone()
        .map(crate::core::contribution::resolve_reviewer)
    {
        Some(Ok(reviewer)) => Some(reviewer),
        Some(Err(detail)) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, detail)),
        None => None,
    };
    let rounds_text = if max_review_rounds == crate::models::UNLIMITED_REVIEW_ROUNDS {
        "unlimited".to_string()
    } else {
        max_review_rounds.to_string()
    };
    let resilience = crate::models::OrchestrationResiliencePolicy {
        activity_timeout_secs: request.activity_timeout_secs,
        review_timeout_secs: request.review_timeout_secs,
        human_wait_timeout_secs: request.human_wait_timeout_secs,
        cancellation_cleanup_policy: request
            .cancellation_cleanup_policy
            // KT-514 — reclaim the clean checkout by default (the branch and its
            // commits survive); `Preserve` stays an explicit opt-in.
            .unwrap_or(crate::models::CancellationCleanupPolicy::RemoveIfClean),
    };
    let result = state
        .db
        .with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let Some(room) = crate::db::discussions::get_discussion(&tx, &request.discussion_id)?
            else {
                bail!("principal discussion not found");
            };
            let discussion_id = request.discussion_id.clone();
            let explicit = named_reviewer.is_some();
            let reviewer = match named_reviewer {
                Some(reviewer) => reviewer,
                None => crate::core::contribution::room_reviewer(
                    &room.agent,
                    room.model.as_deref(),
                    room.connection_id.as_deref(),
                    crate::db::discussions::disc_is_no_agent(&tx, &discussion_id)?,
                ),
            };
            let announcement = crate::core::contribution::room_announcement(
                &target_branch,
                &rounds_text,
                &reviewer,
            );
            let input = crate::models::OrchestrationRunInput {
                kind: crate::models::OrchestrationRunKind::Campaign,
                discussion_id: request.discussion_id,
                project_id: request.project_id,
                target_workspace_id: request.target_workspace_id,
                target_branch: Some(target_branch),
                max_review_rounds,
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
                quota_fallback: request.quota_fallback,
                quota_fallback_agents: Vec::new(),
                reviewer: Some(reviewer.clone()),
            };
            let run = crate::db::orchestration::create_orchestration_run(&tx, &input)?;
            crate::db::orchestration::set_resilience_policy(&tx, &run.id, &resilience)?;
            if explicit {
                crate::db::orchestration::reconfigure_room_for_reviewer(
                    &tx,
                    &discussion_id,
                    &reviewer,
                )?;
            }
            crate::db::orchestration::announce_campaign_rules(&tx, &discussion_id, &announcement)?;
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

pub fn summarize_http_turn_usage(
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
    let mut cached_prompt_tokens = 0u64;
    let mut cache_reported_turns = 0u32;
    let mut cache_write_prompt_tokens = 0u64;
    let mut cache_write_reported_turns = 0u32;
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
            if let Some(cached) = turn.cached_prompt_tokens {
                cached_prompt_tokens = cached_prompt_tokens.saturating_add(cached);
                cache_reported_turns = cache_reported_turns.saturating_add(1);
            }
            if let Some(written) = turn.cache_write_prompt_tokens {
                cache_write_prompt_tokens = cache_write_prompt_tokens.saturating_add(written);
                cache_write_reported_turns = cache_write_reported_turns.saturating_add(1);
            }
            eval_tokens = eval_tokens.saturating_add(turn.eval_tokens);
            duration_ms = duration_ms.saturating_add(turn.duration_ms);
            peak_context_tokens = peak_context_tokens.max(turn.prompt_tokens);
            let phase = phases
                .entry(turn.phase)
                .or_insert(TaskExecutionHttpPhaseUsage {
                    phase: turn.phase,
                    turns: 0,
                    prompt_tokens: 0,
                    cached_prompt_tokens: 0,
                    cache_reported_turns: 0,
                    cache_write_prompt_tokens: 0,
                    cache_write_reported_turns: 0,
                    eval_tokens: 0,
                    duration_ms: 0,
                });
            phase.turns = phase.turns.saturating_add(1);
            phase.prompt_tokens = phase.prompt_tokens.saturating_add(turn.prompt_tokens);
            if let Some(cached) = turn.cached_prompt_tokens {
                phase.cached_prompt_tokens = phase.cached_prompt_tokens.saturating_add(cached);
                phase.cache_reported_turns = phase.cache_reported_turns.saturating_add(1);
            }
            if let Some(written) = turn.cache_write_prompt_tokens {
                phase.cache_write_prompt_tokens =
                    phase.cache_write_prompt_tokens.saturating_add(written);
                phase.cache_write_reported_turns =
                    phase.cache_write_reported_turns.saturating_add(1);
            }
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
        cached_prompt_tokens,
        cache_reported_turns,
        cache_write_prompt_tokens,
        cache_write_reported_turns,
        eval_tokens,
        traffic_tokens: prompt_tokens.saturating_add(eval_tokens),
        peak_context_tokens,
        duration_ms,
        phases: phases.into_values().collect(),
        recent_turns,
    })
}

pub fn execution_detail(
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
                    in_app_breakdown: None,
                    in_app_cost_unknown_reasons: Vec::new(),
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
    let progress = execution_progress(conn, &lineage.execution)?;
    let last_delivery_refusal = crate::db::orchestration::latest_delivery_refusal(conn, exec_id)?;
    let integration_target = run.target_branch.as_deref().and_then(|branch| {
        let [project_id] = task.summary.project_ids.as_slice() else {
            return None;
        };
        let project = crate::db::projects::get_project(conn, project_id)
            .ok()
            .flatten()?;
        let repo = scanner::resolve_host_path(&project.path);
        Some(worktree::describe_integration_target(
            &repo,
            branch,
            &project_managed_roots(&project),
            &|path| crate::db::discussion_workspaces::is_registered_managed_checkout(conn, path),
        ))
    });
    Ok(crate::models::TaskExecutionDetail {
        lineage,
        target_branch: run.target_branch,
        integration_target,
        definition_of_done: task.definition_of_done,
        attempts: attempts.into_values().collect(),
        worker_sessions: crate::db::orchestration::list_worker_cli_sessions(conn, exec_id)?,
        validation_runs: crate::db::orchestration::list_validation_runs(conn, exec_id)?,
        recovery,
        last_delivery_refusal,
        usage: crate::models::TaskExecutionUsage {
            duration_ms,
            in_app_tokens: tokens.in_app_tokens,
            in_app_messages: tokens.in_app_messages,
            in_app_breakdown: tokens.in_app_breakdown,
            in_app_cost_unknown_reasons: tokens.in_app_cost_unknown_reasons,
            in_app_cost_usd,
            in_app_cost_is_partial,
            cli_traffic_tokens: tokens.cli_traffic_tokens,
            cli_billable_tokens: tokens.cli_billable_tokens,
            cli_sessions: tokens.cli_sessions,
            cli_sessions_measured: tokens.cli_sessions_measured,
            cli_sessions_unmeasured: tokens.cli_sessions_unmeasured,
            http,
        },
        progress,
    })
}

/// The execution's progress, with the timing of its warming sub-steps (KT-1637)
/// on top of the phase it is in.
fn execution_progress(
    conn: &rusqlite::Connection,
    execution: &crate::models::TaskExecution,
) -> Result<crate::models::TaskExecutionProgress> {
    let mut progress = execution_phase_progress(conn, execution)?;
    let (steps, total_ms) = warming_timings(conn, execution)?;
    progress.warming_steps = steps;
    progress.warming_total_ms = total_ms;
    Ok(progress)
}

/// How long each warming sub-step took, in the order they run, and the wall time
/// from the execution's creation to the worker's dispatch (to now while it is
/// still provisioning). What the steps do not add up to is time nobody measured.
fn warming_timings(
    conn: &rusqlite::Connection,
    execution: &crate::models::TaskExecution,
) -> Result<(Vec<crate::models::TaskExecutionWarmingStep>, Option<u64>)> {
    use crate::models::{TaskExecutionWarmingStep, WarmingStepKind};
    let mut timed = crate::db::orchestration::latest_warming_steps(conn, &execution.id)?;
    if let Some(record) = crate::db::orchestration::latest_lane_warm_start(conn, &execution.id)? {
        for (step, duration_ms) in record.timed_steps() {
            timed.retain(|known| known.step != step);
            timed.push(TaskExecutionWarmingStep { step, duration_ms });
        }
    }
    const ORDER: [WarmingStepKind; 7] = [
        WarmingStepKind::WorktreeCreation,
        WarmingStepKind::LaneSeeds,
        WarmingStepKind::LaneProvision,
        WarmingStepKind::SeedClone,
        WarmingStepKind::GateWait,
        WarmingStepKind::FirstBuild,
        WarmingStepKind::SmokeCheck,
    ];
    timed.sort_by_key(|known| ORDER.iter().position(|step| *step == known.step));

    // The clock starts with the execution, not with the `lane_warming` event: the
    // worktree and the lane seeds are timed before that event is written. Only a
    // deferred warm-up has a total (the others dispatch at once).
    let deferred =
        crate::db::orchestration::lane_warming_started_at(conn, &execution.id)?.is_some();
    let end = match execution.dispatch_job_id.as_deref() {
        Some(dispatch_id) => {
            crate::db::agent_dispatch::get(conn, dispatch_id)?.map(|job| job.created_at)
        }
        None if execution.status == crate::models::TaskExecutionStatus::Provisioning => {
            Some(chrono::Utc::now())
        }
        None => None,
    };
    let total_ms = end
        .filter(|_| deferred)
        .map(|end| (end - execution.created_at).num_milliseconds().max(0) as u64);
    Ok((timed, total_ms))
}

fn execution_phase_progress(
    conn: &rusqlite::Connection,
    execution: &crate::models::TaskExecution,
) -> Result<crate::models::TaskExecutionProgress> {
    use crate::db::agent_dispatch::DispatchStatus;
    use crate::models::{
        TaskExecutionProgress, TaskExecutionProgressPhase, TaskExecutionTelemetryMode,
    };
    // KT-1250 — a lane whose first build is queued or running holds its worker
    // back in `Provisioning`: say so, and since when, so a stuck warm-up is
    // visible to the principal (the silent-worker watchdog only covers
    // `Working`).
    if execution.status == crate::models::TaskExecutionStatus::Provisioning
        && execution.dispatch_job_id.is_none()
    {
        if let Some(since) = crate::db::orchestration::lane_warming_since(conn, &execution.id)? {
            return Ok(TaskExecutionProgress {
                phase: TaskExecutionProgressPhase::Warming,
                reason: Some("lane_first_build".into()),
                queue_position: None,
                queued_since: Some(since),
                process_alive: None,
                last_reliable_signal_at: Some(since),
                telemetry_mode: TaskExecutionTelemetryMode::Unavailable,
                warming_steps: Vec::new(),
                warming_total_ms: None,
            });
        }
    }
    let Some(dispatch_id) = execution.dispatch_job_id.as_deref() else {
        return Ok(TaskExecutionProgress {
            phase: TaskExecutionProgressPhase::Unknown,
            reason: Some("no_native_dispatch".into()),
            queue_position: None,
            queued_since: None,
            process_alive: None,
            last_reliable_signal_at: None,
            telemetry_mode: TaskExecutionTelemetryMode::Unavailable,
            warming_steps: Vec::new(),
            warming_total_ms: None,
        });
    };
    let Some(job) = crate::db::agent_dispatch::get(conn, dispatch_id)? else {
        return Ok(TaskExecutionProgress {
            phase: TaskExecutionProgressPhase::Unknown,
            reason: Some("dispatch_unavailable".into()),
            queue_position: None,
            queued_since: None,
            process_alive: None,
            last_reliable_signal_at: None,
            telemetry_mode: TaskExecutionTelemetryMode::Unavailable,
            warming_steps: Vec::new(),
            warming_total_ms: None,
        });
    };
    let (phase, reason, queue_position) = match job.status {
        // KT-1632 — the task-worker cap holds this launch: the claim left the
        // reason on the job (`task_worker_limit:<running>/<cap>`), so it is
        // still told after a restart. It is told only while the job is really in
        // line: one a backoff keeps out has no place, and its stored reason is
        // stale.
        DispatchStatus::Pending => {
            let cap_reason = job
                .progress_detail
                .clone()
                .filter(|detail| {
                    job.progress_phase.as_deref() == Some("queued")
                        && detail.starts_with(
                            crate::db::agent_dispatch::TASK_WORKER_QUEUED_DETAIL_PREFIX,
                        )
                })
                .map(|detail| {
                    crate::db::agent_dispatch::task_worker_queue_position(conn, &job.id)
                        .map(|position| position.map(|position| (detail, position)))
                })
                .transpose()?
                .flatten();
            match cap_reason {
                Some((detail, position)) => (
                    TaskExecutionProgressPhase::Queued,
                    Some(detail),
                    Some(position),
                ),
                None => (
                    TaskExecutionProgressPhase::Queued,
                    Some("concurrency_limit_or_scheduler".into()),
                    None,
                ),
            }
        }
        DispatchStatus::Running if job.agent_started_at.is_none() => (
            TaskExecutionProgressPhase::Queued,
            Some("local_concurrency_limit".into()),
            None,
        ),
        DispatchStatus::Running => {
            let phase = match job.progress_phase.as_deref() {
                Some("tool_activity") => TaskExecutionProgressPhase::ToolActivity,
                Some("delivering") => TaskExecutionProgressPhase::Delivering,
                Some("upstream_wait") => TaskExecutionProgressPhase::UpstreamWait,
                _ => TaskExecutionProgressPhase::Launching,
            };
            // A tool name never leaves as such: it can name an MCP server.
            let reason = match (&phase, job.progress_detail.as_deref()) {
                (TaskExecutionProgressPhase::ToolActivity, Some(detail)) => {
                    Some(crate::agents::activity::stored_tool_detail_label(detail).to_string())
                }
                _ => job.progress_detail.clone(),
            };
            (phase, reason, None)
        }
        DispatchStatus::Completed => (TaskExecutionProgressPhase::Completed, None, None),
        DispatchStatus::Failed | DispatchStatus::Cancelled => (
            TaskExecutionProgressPhase::Failed,
            job.failure_kind.clone().or(job.last_error.clone()),
            None,
        ),
    };
    Ok(TaskExecutionProgress {
        phase,
        reason,
        queue_position,
        queued_since: (job.status == DispatchStatus::Pending || job.agent_started_at.is_none())
            .then_some(job.created_at),
        process_alive: None,
        last_reliable_signal_at: job
            .last_progress_at
            .or(job.agent_started_at)
            .or(job.claimed_at),
        telemetry_mode: if job.last_progress_at.is_some() {
            TaskExecutionTelemetryMode::BoundaryOnly
        } else {
            TaskExecutionTelemetryMode::Unavailable
        },
        warming_steps: Vec::new(),
        warming_total_ms: None,
    })
}

pub fn attach_runtime_liveness(state: &AppState, detail: &mut crate::models::TaskExecutionDetail) {
    let Some(dispatch_id) = detail.lineage.execution.dispatch_job_id.as_deref() else {
        return;
    };
    detail.progress.process_alive = state
        .agent_runtime_registry
        .lock()
        .ok()
        .and_then(|registry| registry.contains(dispatch_id).then_some(true));
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
        Ok(mut view) => {
            attach_runtime_liveness(&state, &mut view);
            Json(ApiResponse::ok(view))
        }
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

pub fn execution_observability(
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

/// Body of `POST /api/orchestration/executions/{id}/extend-review-rounds`.
#[derive(Deserialize)]
pub struct ExtendReviewRoundsRequest {
    /// Rounds to add, or `"unlimited"`.
    pub extend_review_rounds: serde_json::Value,
    pub reason: String,
}

/// The human's « extend by N » (KT-1345): the same amendment the principal
/// makes through `task_exec_reassign`, journaled as a human decision.
pub async fn extend_review_rounds_http(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
    Json(request): Json<ExtendReviewRoundsRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let extension =
        match crate::db::orchestration::ReviewRoundsExtension::parse(&request.extend_review_rounds)
        {
            Ok(extension) => extension,
            Err(detail) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, detail)),
        };
    let mut actor = backend_actor();
    actor.kind = PlanningActorKind::Human;
    actor.id = Some("human".to_string());
    match extend_execution_review_rounds(&state, &exec_id, extension, &request.reason, actor).await
    {
        Ok(value) => Json(ApiResponse::ok(value)),
        Err((code, message)) => Json(ApiResponse::err_coded(code, message)),
    }
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

/// Retry an Applying-origin hold once its cause (dirty, moved or missing target
/// checkout) was dealt with.
///
/// This is deliberately narrower than boot recovery: only an Applying-origin
/// block may enter, the real target branch and cleanliness are re-checked, and
/// the state-machine checkpoint is cleared through `Blocked -> Applying` before
/// any Git operation. No caller can use this path to skip provisioning/review.
pub async fn resume_blocked_apply(
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
    let target_shape =
        worktree::integration_target(&repo, &target).map_err(ProvisionError::CheckpointRefused)?;
    let dirty = worktree::integration_target_dirty_files(&target_shape)
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
                    serde_json::json!({ "recovery": "apply_hold_cleared" }),
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

/// Is `exec_id` an integration send-back the principal can take back (KT-862)?
async fn restorable_rework(
    db: &Database,
    exec_id: &str,
) -> Result<Option<crate::db::orchestration::RestorableApproval>, ProvisionError> {
    let id = exec_id.to_string();
    db.with_conn(move |conn| {
        let Some(execution) = crate::db::orchestration::get_task_execution(conn, &id)? else {
            return Ok(None);
        };
        if !matches!(
            execution.status,
            TaskExecutionStatus::Working
                | TaskExecutionStatus::Provisioning
                | TaskExecutionStatus::Blocked
                | TaskExecutionStatus::ChangesRequested
        ) {
            return Ok(None);
        }
        crate::db::orchestration::restorable_approval(conn, &execution)
    })
    .await
    .map_err(|error| ProvisionError::Internal(error.to_string()))
}

/// Run the protected integration again on the SAME approved delivery, after the
/// principal established that the failure did not come from it (KT-862): a flaky
/// test, a full disk, a busy machine. No new delivery is asked of the worker.
///
/// Sending an approved delivery back re-activates its worker, so this stands it
/// down first, then returns the execution to `Approved`. The worktree is checked
/// against the HEAD the worker was sent back with: a worker that already committed
/// or left changes has made a delivery nobody reviewed, which the approval does not
/// cover, so the re-validation is refused instead of integrating it.
async fn revalidate_sent_back_execution(
    db: &Database,
    exec_id: &str,
) -> Result<IntegrationOutcome, ProvisionError> {
    let internal = |error: String| ProvisionError::Internal(error);
    let (approval, workspace) = {
        let id = exec_id.to_string();
        db.with_conn(move |conn| {
            let Some(execution) = crate::db::orchestration::get_task_execution(conn, &id)? else {
                return Ok((None, None));
            };
            let approval = crate::db::orchestration::restorable_approval(conn, &execution)?;
            let workspace = crate::db::discussion_workspaces::get_managed_for_execution(conn, &id)?;
            Ok((approval, workspace))
        })
        .await
        .map_err(|error| internal(error.to_string()))?
    };
    let Some(approval) = approval else {
        return Err(ProvisionError::CheckpointRefused(
            "execution is not an integration send-back with an intact approved delivery".into(),
        ));
    };
    let child_path = workspace
        .and_then(|workspace| workspace.canonical_path)
        .ok_or_else(|| ProvisionError::CheckpointRefused("no managed worktree".into()))?;
    let child = std::path::Path::new(&child_path);
    let dirty = worktree::worktree_dirty_files(child).map_err(ProvisionError::CheckpointRefused)?;
    if !dirty.is_empty() {
        return Err(ProvisionError::CheckpointRefused(format!(
            "the worker left {} uncommitted file(s) in the worktree since the send-back: they are \
             not part of the approved delivery",
            dirty.len()
        )));
    }
    if let Some(sent_back_at) = approval.head_sha.as_deref() {
        let head =
            worktree::resolve_commit(child, "HEAD").map_err(ProvisionError::CheckpointRefused)?;
        if head != sent_back_at {
            return Err(ProvisionError::CheckpointRefused(format!(
                "the worker committed since the send-back (worktree at {head}, sent back at \
                 {sent_back_at}): that work is not the approved delivery — wait for its \
                 re-delivery and review it"
            )));
        }
    }

    let id = exec_id.to_string();
    let reopened = db
        .with_conn(move |conn| {
            crate::db::orchestration::reopen_approval_after_rework(conn, &id, &backend_actor())
        })
        .await
        .map_err(|error| internal(error.to_string()))?;
    match reopened {
        crate::db::orchestration::ReopenOutcome::Reopened { .. } => {}
        crate::db::orchestration::ReopenOutcome::NotRestorable => {
            return Err(ProvisionError::CheckpointRefused(
                "the worker already answered the send-back: review its delivery instead".into(),
            ))
        }
        crate::db::orchestration::ReopenOutcome::ExecutionRaced => {
            return Err(ProvisionError::CheckpointRefused(
                "the execution moved while its approval was being restored".into(),
            ))
        }
    }
    run_integration(db, exec_id).await
}

pub async fn resume_execution(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
) -> Json<ApiResponse<ExecutionRecoveryView>> {
    let Some(_slot) = ResumeSlot::claim(&state.execution_resumes, &exec_id) else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            "a resume of this execution is already running",
        ));
    };
    resume_claimed_execution(state, exec_id).await
}

/// `/resume` for a caller that holds the execution's [`ResumeSlot`].
pub async fn resume_claimed_execution(
    state: AppState,
    exec_id: String,
) -> Json<ApiResponse<ExecutionRecoveryView>> {
    // KT-1646 — a checkout a release is removing is not put back to use.
    if let Err(error) = crate::core::checkout_release::ensure_not_being_released(&exec_id) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            error.to_string(),
        ));
    }
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
    if snapshot.execution.status == TaskExecutionStatus::Done
        && snapshot.execution.integrated_sha.is_some()
    {
        let mut snapshot = snapshot;
        snapshot.outcome = Some("already resumed: Done".to_string());
        return Json(ApiResponse::ok(snapshot));
    }
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
    // An approved delivery the integration sent back, whose failure the principal
    // judges did not come from the delivery: run the same validations again on the
    // same candidate, without a new delivery (KT-862). Any other worker-owned state
    // falls through to the ordinary refusals below.
    if matches!(restorable_rework(&state.db, &exec_id).await, Ok(Some(_))) {
        let outcome = match revalidate_sent_back_execution(&state.db, &exec_id).await {
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
    // KT-1537 — an `Interrupted` execution whose decision was consumed was
    // interrupted again after that resume: arm a fresh decision instead of
    // refusing it. A replayed resume finds the execution moved on, not here.
    let snapshot = if snapshot.execution.status == TaskExecutionStatus::Interrupted
        && snapshot
            .recovery
            .as_ref()
            .is_none_or(|recovery| !recovery.pending)
    {
        if let Err(error) = rearm_stranded_interruption(&state, &exec_id).await {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                format!("interrupted execution has no recoverable decision: {error}"),
            ));
        }
        let id = exec_id.clone();
        match state
            .db
            .with_conn(move |conn| recovery_view(conn, &id))
            .await
        {
            Ok(rearmed) => rearmed,
            Err(error) => {
                return Json(ApiResponse::err_coded(
                    ApiErrorCode::Internal,
                    error.to_string(),
                ))
            }
        }
    } else {
        snapshot
    };
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

    let origin = snapshot
        .execution
        .interrupted_from_status
        .unwrap_or(TaskExecutionStatus::Interrupted);
    let recovery = if recovery.recovery_action.resumable_from(origin) {
        recovery
    } else {
        match refresh_integration_recovery(&state.db, &exec_id).await {
            Ok(refreshed) => refreshed,
            Err(error) => {
                return Json(ApiResponse::err_coded(
                    ApiErrorCode::Conflict,
                    format!(
                        "recovery decision {} cannot resume from {}: {error}",
                        recovery.recovery_action.as_str(),
                        origin.as_str()
                    ),
                ));
            }
        }
    };
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
        ExecutionRecoveryAction::ResumeWorker => {
            // KT-1713 — a fallback taken for a quota window is temporary: once
            // the window is over, the resume goes back to the declared worker.
            match return_lapsed_fallback_on_resume(&state, &exec_id).await {
                Some(outcome) => Ok(outcome),
                None => {
                    wake_recovered_worker(&state.db, &exec_id)
                        .await
                        .map_err(|error| match error.downcast::<StaleWorkerScope>() {
                            Ok(stale) => ProvisionError::CheckpointRefused(stale.0),
                            Err(error) => ProvisionError::Internal(error.to_string()),
                        })
                }
            }
        }
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
            // Only BlockDirtyTarget parks by design; any other refusal leaves the
            // decision pending so the next resume can apply it.
            match resume_recovered_integration(&state.db, &exec_id, action).await {
                Ok(IntegrationOutcome::Refused { reason })
                    if action != ExecutionRecoveryAction::BlockDirtyTarget =>
                {
                    Err(ProvisionError::CheckpointRefused(reason))
                }
                other => other.map(|outcome| format!("{outcome:?}")),
            }
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
    let applied = recovery.clone();
    let refreshed = state
        .db
        .with_conn(move |conn| {
            // KT-1537 — an execution interrupted again meanwhile keeps its
            // fresh decision, and so does a decision re-armed meanwhile; only
            // the exact one this resume applied is consumed.
            crate::db::orchestration::consume_applied_recovery(conn, &id, &applied)?;
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
pub async fn settle_cancelled_workspace(
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
        // Only a clean checkout goes at cancel time: the cancel merely signalled
        // the worker, which may still be writing. A dirty tree (and every
        // Preserve cancel) is left to the finished-execution sweep (KT-1397),
        // which proves the execution terminal and idle before it saves the diff.
        match worktree::remove_cancelled_task_worktree(&repo, &checkout, &workspace.branch) {
            Ok(()) => {
                survives = false;
                // KT-1256 — the checkout is gone, so is the lane it built in.
                // The DB's own path string, because the audit looks the row up
                // by that exact value.
                reclaim_lane_target(db, &repo, path).await;
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

pub async fn wait_for_cancelled_dispatches_to_settle(
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

/// What a cancel tells the principal about the task it left alone (KT-1267).
/// Cancelling never moves the task: the work may already be integrated by
/// hand, so the status stays the principal's to set.
fn cancelled_task_status_note(status: PlanningTaskStatus) -> String {
    match status {
        PlanningTaskStatus::Todo => {
            "task status kept: todo; it can be prepared again directly".to_string()
        }
        other => format!(
            "task status kept: {}; to run it again, set it back to todo (task_update \
             status=todo) before task_exec_prepare, or leave it if the work is integrated",
            other.as_str()
        ),
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
    // KT-1250 — an execution whose lane is still warming has no dispatch yet,
    // but it has a first build compiling (or queued): stop it and free its
    // slot. Signalled once the cancel is durable, so a refused cancel never
    // leaves a `Provisioning` execution with its build killed.
    let warm_up_signalled = signal_warm_up_cancel(&exec_id);
    let cancellation_signal_sent = cancellation_signal_sent || warm_up_signalled;
    let dispatches_settled = wait_for_cancelled_dispatches_to_settle(
        &state.cancel_registry,
        &signalled_keys,
        CANCELLATION_TERMINATION_TIMEOUT,
    )
    .await;
    let warm_up_settled = !warm_up_signalled
        || wait_for_warm_up_to_settle(&exec_id, CANCELLATION_TERMINATION_TIMEOUT).await;
    let termination_confirmed =
        (dispatches_settled || (signalled_keys.is_empty() && warm_up_signalled)) && warm_up_settled;
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
            // KT-1267 — the cancel leaves the task status alone; say what it
            // is and how to relaunch, since the launch preflight wants `todo`.
            let task_status = crate::db::planning::get_task(conn, &view.execution.task_id)?
                .map(|task| task.summary.status);
            view.outcome = Some(match task_status {
                Some(status) => format!("{outcome}; {}", cancelled_task_status_note(status)),
                None => outcome,
            });
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
/// What a replacement worker must know when its reassignment rejected the
/// delivery that was awaiting review (KT-791).
fn rejected_delivery_note(before: &TaskExecution, lang: RoomLang) -> Option<String> {
    (before.status == TaskExecutionStatus::AwaitingReview).then(|| {
        let attempt = before.attempt_no;
        match lang {
            RoomLang::Fr => format!(
                "\n\n## Livraison rejetée\n\nLa livraison de la tentative {attempt} attendait une \
                 revue ; cette réaffectation l'a rejetée. Elle reste dans l'historique : produis \
                 et livre une nouvelle tentative."
            ),
            RoomLang::En => format!(
                "\n\n## Delivery rejected\n\nThe delivery of attempt {attempt} was waiting for a \
                 review; this reassignment rejected it. It stays in the history: produce and \
                 deliver a new attempt."
            ),
        }
    })
}

/// KT-1402 — what a reassignment tells the replacement worker when it adopted a
/// Definition of Done that differs from the one the execution was launched with.
fn dod_readopted_note(
    conn: &rusqlite::Connection,
    before: &TaskExecution,
    after: &TaskExecution,
    lang: RoomLang,
) -> Result<Option<String>> {
    if before.worker_dod_ids == after.worker_dod_ids {
        return Ok(None);
    }
    Ok(
        crate::db::planning::get_task(conn, &after.task_id)?.map(|task| {
            format!(
                "\n\n{}",
                crate::db::orchestration::dod_readopted_notice(lang, &task.definition_of_done)
            )
        }),
    )
}

pub async fn reassign_native_execution(
    state: &AppState,
    exec_id: &str,
    selection: crate::models::CampaignWorkerSelection,
    reason: &str,
) -> Result<ExecutionRecoveryView> {
    reassign_native_execution_as(state, exec_id, selection, reason, ReassignCause::Human).await
}

/// Who is moving an execution to another worker (KT-1564). A person's gesture
/// is final. The two Kronn-made moves are explicit and bounded: the quota
/// fallback a campaign opted into, and the return to the worker that fallback
/// (or the refused provider) left once the announced reset has passed.
#[derive(Debug, Clone, Copy)]
pub enum ReassignCause {
    Human,
    /// Opted-in provider fallback; the worker being left is remembered, with
    /// the reset the refusal announced, so the sweep can bring it back.
    QuotaFallback {
        reset_at: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// The sweep handing an execution back after the reset. Re-validated
    /// inside the reassignment transaction.
    QuotaReturn,
    /// The provider re-check (KT-1664) found the provider answering again for
    /// an escalation that announced no reset. Same worker, same worktree;
    /// re-validated inside the reassignment transaction.
    QuotaProbed,
    /// KT-1713 — a resume of an `Interrupted` execution whose quota fallback
    /// window has ended: it goes back to the worker it declared instead of
    /// being woken on the alternative. Re-validated inside the transaction.
    FallbackLapsed,
}

impl ReassignCause {
    /// The actor a same-provider quota re-arm is attributed to: a person's
    /// gesture is `Human`, Kronn's own decision `Backend`, so the audit never
    /// credits one to the other.
    fn rearm_actor(self) -> PlanningActorKind {
        match self {
            Self::Human => PlanningActorKind::Human,
            Self::QuotaFallback { .. }
            | Self::QuotaReturn
            | Self::QuotaProbed
            | Self::FallbackLapsed => PlanningActorKind::Backend,
        }
    }

    /// Kronn handing an execution back to the worker it was parked on.
    fn is_quota_return(self) -> bool {
        matches!(
            self,
            Self::QuotaReturn | Self::QuotaProbed | Self::FallbackLapsed
        )
    }

    /// The still-valid check a quota return runs before and inside its
    /// reassignment transaction.
    fn ensure_return_still_due(
        self,
        conn: &rusqlite::Connection,
        exec_id: &str,
        provider: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now();
        match self {
            Self::QuotaProbed => crate::db::orchestration::ensure_probed_quota_return_still_parked(
                conn, exec_id, provider, now,
            ),
            Self::FallbackLapsed => crate::db::orchestration::ensure_fallback_lapsed_on_resume(
                conn, exec_id, provider, now,
            ),
            _ => crate::db::orchestration::ensure_quota_return_still_due(
                conn, exec_id, provider, now,
            ),
        }
    }
}

/// KT-1758 — a fallback's reassignment found the execution no longer on the
/// dispatch that failed: something else (the quota return, a person) moved it
/// since. The failed dispatch's hand-over has nothing left to do, and says
/// nothing about an execution that is no longer its own.
#[derive(Debug)]
pub struct ExecutionMovedOn {
    pub execution_id: String,
}

impl std::fmt::Display for ExecutionMovedOn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "execution {} no longer runs the dispatch that failed: it was moved meanwhile",
            self.execution_id
        )
    }
}

impl std::error::Error for ExecutionMovedOn {}

/// Whether `error` is [`ExecutionMovedOn`].
pub fn is_execution_moved_on(error: &anyhow::Error) -> bool {
    error.downcast_ref::<ExecutionMovedOn>().is_some()
}

/// [`reassign_native_execution`] with the [`ReassignCause`] behind the move.
pub async fn reassign_native_execution_as(
    state: &AppState,
    exec_id: &str,
    selection: crate::models::CampaignWorkerSelection,
    reason: &str,
    cause: ReassignCause,
) -> Result<ExecutionRecoveryView> {
    reassign_native_execution_for_dispatch(state, exec_id, selection, reason, cause, None).await
}

/// [`reassign_native_execution_as`] for the hand-over of a failed dispatch:
/// `expected_dispatch` is the dispatch whose failure is being answered. The
/// execution must still point at it, checked before anything is written and
/// again inside the transaction; otherwise nothing is done and the error is
/// [`ExecutionMovedOn`].
pub async fn reassign_native_execution_for_dispatch(
    state: &AppState,
    exec_id: &str,
    selection: crate::models::CampaignWorkerSelection,
    reason: &str,
    cause: ReassignCause,
    expected_dispatch: Option<&str>,
) -> Result<ExecutionRecoveryView> {
    let expected_dispatch = expected_dispatch.map(str::to_string);
    if selection.target.kind == MessageTargetKind::Cli {
        bail!("native reassignment cannot target a CLI session");
    }
    crate::db::orchestration::ensure_task_worker_transport_compatible(&selection.target)
        .map_err(|error| anyhow::anyhow!("worker_transport: {error}"))?;
    // KT-1749 — the launch's own static verdict (native ACP route, typed
    // delivery capability), so a worker is never moved onto an agent the
    // spawn would then refuse. `Custom` keeps its own connection-pin
    // validation, which names the missing connection.
    if selection.target.agent_type != AgentType::Custom {
        if let Some(reason) = worker_static_refusal(&selection.target) {
            bail!("{}: {}", reason.code, reason.detail);
        }
    }
    let id = exec_id.to_string();
    let execution = state
        .db
        .with_conn(move |conn| {
            crate::db::orchestration::get_task_execution(conn, &id)?
                .context("execution vanished before worker reassignment")
        })
        .await?;
    if let Some(expected) = expected_dispatch.as_deref() {
        // Before the git re-anchoring below, which is not undone.
        if execution.dispatch_job_id.as_deref() != Some(expected) {
            return Err(ExecutionMovedOn {
                execution_id: execution.id,
            }
            .into());
        }
    }
    // KT-2000 — the boundary every Kronn-made quota fallback ends at: a
    // generic worker builds, tests and commits, so Kronn never moves it on its
    // own to an agent without a shell, whichever selector chose that agent. An
    // explicit choice, by a person or by the principal (`Human`), stays
    // possible; a quota return hands back the worker the execution declared.
    if matches!(cause, ReassignCause::QuotaFallback { .. })
        && execution.worker_scope.is_none()
        && !crate::core::agent_fallback::capabilities(&selection.target.agent_type).shell
    {
        bail!(
            "quota fallback refused: {} cannot take a generic task worker: {}",
            crate::core::agent_fallback::display_name(&selection.target.agent_type),
            crate::core::agent_fallback::NO_SHELL
        );
    }
    if cause.is_quota_return() {
        // Same check as the one inside the reassignment transaction (which
        // stays authoritative), run first so a return that is already stale
        // does not persist a re-anchored scope for a worker round that will
        // never happen.
        let id = exec_id.to_string();
        let provider = crate::db::orchestration::agent_type_to_db(&selection.target.agent_type);
        state
            .db
            .with_conn(move |conn| cause.ensure_return_still_due(conn, &id, &provider))
            .await?;
    }
    let lang = room_lang(&state.db, &execution.parent_discussion_id).await;
    let scope_note = reanchor_execution_scope(&state.db, &execution)
        .await?
        .map(|reanchored| reanchored_scope_note(&reanchored, lang));
    let rejected_note = rejected_delivery_note(&execution, lang);
    let id = exec_id.to_string();
    let persisted_reason = reason.to_string();
    let (view, replaced_dispatch_id) = state
        .db
        .with_conn(move |conn| {
            // Retrying the very provider whose quota stopped this execution is
            // the human's "the quota is back" signal (KT-838). It re-arms the
            // whole quota generation below, so a second execution escalated by
            // the same outage no longer keeps the provider refused, whichever
            // of the two is reassigned first.
            let provider = crate::db::orchestration::agent_type_to_db(&selection.target.agent_type);
            if let Some(expected) = expected_dispatch.as_deref() {
                // Authoritative: the check above ran before the git
                // re-anchoring, and the quota return may have landed since.
                let current = crate::db::orchestration::get_task_execution(conn, &id)?
                    .context("execution vanished before worker reassignment")?;
                if current.dispatch_job_id.as_deref() != Some(expected) {
                    return Err(ExecutionMovedOn {
                        execution_id: id.clone(),
                    }
                    .into());
                }
            }
            let quota_retry =
                crate::db::orchestration::is_quota_retry_on_same_provider(conn, &id, &provider)?;
            // Refuse tôt (KT-515): before starting the transaction, and before
            // creating any new dispatch/worktree, if the target provider still
            // has an open quota escalation elsewhere.
            if !quota_retry {
                if let Some(reason) = worker_quota_refusal(conn, &selection.target, Some(&id))? {
                    bail!("{}: {}", reason.code, reason.detail);
                }
            }
            let transaction = conn.unchecked_transaction()?;
            if cause.is_quota_return() {
                // The sweep read this execution before the git re-anchoring
                // above. A person may have moved, paused or finished it since:
                // refuse here, before anything is written or cancelled.
                cause.ensure_return_still_due(&transaction, &id, &provider)?;
            }
            let before = crate::db::orchestration::get_task_execution(&transaction, &id)?
                .context("execution vanished before worker reassignment")?;
            let replaced_dispatch_id = before.dispatch_job_id.clone();
            let earlier_origin = crate::db::orchestration::quota_origin(&transaction, &id)?;
            // Read before the reassignment, which forgets them with the origin:
            // a fallback carries the chain's refusals over to its next link.
            let earlier_refusals =
                crate::db::orchestration::execution_quota_refusals(&transaction, &id)?;
            // The origin outlives the sweep's own moves until the execution is
            // back on it. When only the fallback provider's reset has passed,
            // the execution retries that provider and still remembers where it
            // must return; a person's choice always ends the memory.
            // "Back" means the original provider AND tier, not a clock reading.
            let back_on_origin = earlier_origin
                .as_ref()
                .is_some_and(|origin| origin.is_worker(&provider, selection.target.tier));
            let keep_quota_origin = cause.is_quota_return()
                && earlier_origin.is_some()
                && !back_on_origin;
            let execution = crate::db::orchestration::reassign_execution_worker_keeping_origin(
                &transaction,
                &id,
                &selection,
                &persisted_reason,
                &backend_actor(),
                keep_quota_origin,
            )?;
            // KT-1758 — a person's choice ends the chain's memory of where the
            // worker started, as it ends the quota origin: the worker they pick
            // is not a fallback, so a refusal at its launch is theirs to read.
            // (A return keeps no such need: the origin is then the current
            // worker, which `worker_launch_context` already reads as not moved.)
            if matches!(cause, ReassignCause::Human) {
                crate::core::agent_fallback::clear_execution_origin(&transaction, &id)?;
            }
            let dod_note = dod_readopted_note(&transaction, &before, &execution, lang)?;
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
            // `reassign_execution_worker` forgot any earlier origin: a person's
            // choice, or the return itself, leaves nothing to go back to. Only
            // a quota fallback remembers the worker it is leaving, tier, model
            // and profile included, so the sweep can hand it back. A second
            // fallback keeps the first origin: it is the worker that was chosen.
            if let ReassignCause::QuotaFallback { reset_at } = cause {
                if before.worker_target_kind != Some(MessageTargetKind::Cli) && !back_on_origin {
                    let origin = match earlier_origin {
                        Some(origin) => origin,
                        None => crate::db::orchestration::QuotaOrigin::new(
                            crate::models::CampaignWorkerSelection {
                                target: worker_target_from_execution(&before)?,
                                model: before.worker_model.clone(),
                                profile_id: before.worker_profile_id.clone(),
                            },
                            reset_at,
                        ),
                    };
                    crate::db::orchestration::record_quota_origin(&transaction, &id, &origin)?;
                    // The chain so far, plus the provider this fallback leaves:
                    // none of them is a candidate again until the execution is
                    // back on its origin (or, if it announced a reset, until
                    // that reset). Without a reset nothing else would stop the
                    // chain bouncing between two exhausted providers.
                    for refusal in earlier_refusals {
                        crate::db::orchestration::record_execution_quota_refusal(
                            &transaction,
                            &id,
                            &refusal,
                        )?;
                    }
                    if let Some(left) = before.worker_agent_type.clone() {
                        crate::db::orchestration::record_execution_quota_refusal(
                            &transaction,
                            &id,
                            &crate::db::orchestration::QuotaRefusal {
                                provider: left,
                                reset_at: reset_at.map(crate::db::orchestration::quota_reset_stamp),
                            },
                        )?;
                    }
                }
            }
            if quota_retry {
                // Same transaction as the reassignment: a refused or failed
                // handoff leaves the provider exactly as blocked as before.
                crate::db::orchestration::rearm_provider_quota(
                    &transaction,
                    &provider,
                    &format!("reassign:{id}:{generation}"),
                    cause.rearm_actor().as_str(),
                    Some(&format!("reassign:{id}")),
                )?;
            }
            let mut handoff = handoff_notice_in(
                lang,
                has_recorded_delivery(&transaction, &id)?,
                Some(generation),
                Some(&transaction),
                Some(&id),
            );
            if !persisted_reason.trim().is_empty() {
                handoff.push_str(lang.pick(
                    "\n\n## Consigne du principal pour cette réaffectation\n\n",
                    "\n\n## Principal's instruction for this reassignment\n\n",
                ));
                handoff.push_str(persisted_reason.trim());
            }
            handoff.push_str(scope_note.as_deref().unwrap_or_default());
            handoff.push_str(rejected_note.as_deref().unwrap_or_default());
            handoff.push_str(dod_note.as_deref().unwrap_or_default());
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
            // KT-1758 — the replacement is due now, whatever hold the worker it
            // replaces is under: that hold is on the provider it left, not on
            // this dispatch's. A replay of the same generation returns the job
            // already queued; it is brought forward too.
            crate::db::agent_dispatch::make_available_now(&transaction, &job.id)?;
            crate::db::orchestration::attach_execution_dispatch(&transaction, &id, &job.id)?;
            let current = crate::db::orchestration::get_task_execution(&transaction, &id)?
                .context("execution vanished during handoff")?;
            if matches!(
                current.status,
                TaskExecutionStatus::Interrupted
                    | TaskExecutionStatus::ChangesRequested
                    | TaskExecutionStatus::Escalated
                    // KT-640: a redirected awaiting-CLI-acceptance hold cleared
                    // back to Provisioning (db::reassign_execution_worker); a
                    // native worker dispatches immediately, exactly like the
                    // initial KT-328 handshake's Provisioning -> Working.
                    | TaskExecutionStatus::Provisioning
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
                "handoff generation {} queued in the existing child discussion{}",
                recovery.assignment_generation,
                if dod_note.is_some() {
                    "; the task's Definition of Done changed since launch and the execution re-adopted it"
                } else {
                    ""
                }
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
            let before = crate::db::orchestration::get_task_execution(conn, &id)?
                .context("unknown task execution")?;
            let transaction = conn.unchecked_transaction()?;
            let execution = crate::db::orchestration::reassign_execution_worker(
                &transaction,
                &id,
                &selection,
                &reason,
                &backend_actor(),
            )?;
            // KT-1758 — a person's choice of worker, a CLI session included,
            // ends the chain's memory of where the worker started (as the
            // native reassignment does): the session is not a fallback.
            crate::core::agent_fallback::clear_execution_origin(&transaction, &id)?;
            transaction.commit()?;
            let lang = crate::db::discussions::room_lang(conn, &before.parent_discussion_id);
            let dod_note = dod_readopted_note(conn, &before, &execution, lang)?;
            let note = [rejected_delivery_note(&before, lang), dod_note.clone()]
                .into_iter()
                .flatten()
                .collect::<String>();
            Ok((
                execution,
                (!note.is_empty()).then_some(note),
                dod_note.is_some(),
            ))
        })
        .await;
    let Ok((execution, rejected_note, dod_readopted)) = reassigned else {
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
                // A same-assignment recovery keeps the worker in its child.
                // A newly selected CLI still starts in the principal room.
                let origin: String = tx.query_row(
                    "SELECT disc_id FROM discussion_sessions \
                     WHERE id = ?1 AND agent_type = ?2 AND status <> 'left' \
                       AND (disc_id = ?3 OR disc_id = ?4)",
                    rusqlite::params![
                        session_id,
                        crate::db::orchestration::agent_type_to_db(&provider),
                        origin,
                        child_for_offer,
                    ],
                    |row| row.get(0),
                )?;
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
                        {
                            let generation = recovery.assignment_generation;
                            let offer_id = &offer.id;
                            let rejected = rejected_note.as_deref().unwrap_or_default();
                            match crate::db::discussions::room_lang(
                                &tx,
                                &current.parent_discussion_id,
                            ) {
                                RoomLang::Fr => format!(
                                    "**Offre de réassignation — génération {generation}**\n\n\
                                     Accepte avec `task_exec_accept_worker_offer({{ offer_id: \"{offer_id}\" }})`. \
                                     Tu reprendras la même sous-discussion, le même worktree, les manifests, \
                                     constats et SHA déjà persistés ; aucun travail validé ne doit être rejoué.{rejected}"
                                ),
                                RoomLang::En => format!(
                                    "**Reassignment offer — generation {generation}**\n\n\
                                     Accept with `task_exec_accept_worker_offer({{ offer_id: \"{offer_id}\" }})`. \
                                     You will take over the same sub-discussion, the same worktree, the manifests, \
                                     findings and SHAs already persisted; no validated work must be replayed.{rejected}"
                                ),
                            }
                        },
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
                    "CLI handoff generation {} awaits exact session acceptance{}",
                    recovery.assignment_generation,
                    if dod_readopted {
                        "; the task's Definition of Done changed since launch and the execution re-adopted it"
                    } else {
                        ""
                    }
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
    /// KT-1345 — integer or `"unlimited"`; absent = Settings > Workers.
    #[serde(default, deserialize_with = "explicit_review_rounds")]
    pub max_review_rounds: Option<serde_json::Value>,
    /// KT-1564 — providers this launch explicitly lets the execution fall back
    /// to when its provider refuses for quota. Absent or empty: never.
    #[serde(default)]
    pub quota_fallback_agents: Vec<crate::models::AgentType>,
}

#[derive(Deserialize)]
pub struct TaskExecPrepareRequest {
    pub task_reference: String,
    pub parent_discussion_id: String,
    pub worker: MessageTarget,
    #[serde(default)]
    pub source_agent: String,
    #[serde(default)]
    pub source_session_id: String,
    /// The room's own native agent, injected by the bridge from the runner's
    /// environment. Exclusive with `source_agent`/`source_session_id`.
    #[serde(default)]
    pub room_agent: Option<SpawnedAgentCaller>,
    #[serde(default)]
    pub worker_scope_intent: Option<TaskWorkerScopeIntent>,
    #[serde(default)]
    pub worker_scope: Option<TaskWorkerScope>,
    /// The gates the principal intends to launch with. Optional: when given they
    /// are held to the launch boundary's rules, so a command that could never run
    /// makes the preflight refuse instead of the integration.
    #[serde(default)]
    pub validations: Vec<crate::models::ValidationSpec>,
    /// KT-1688 — the integration branch the launch will name. Checked like at
    /// launch, so an owner's checkout refuses the preflight instead of the
    /// integration. Absent: the room's declared integration branch is checked.
    #[serde(default)]
    pub base_rev: Option<String>,
    /// KT-1345 — checked like at launch, so a bad value refuses the preflight.
    #[serde(default, deserialize_with = "explicit_review_rounds")]
    pub max_review_rounds: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskWorkerScopeIntent {
    Generic,
    Scoped,
}

#[derive(Deserialize)]
pub struct TaskWorkerCatalogueRequest {
    /// The discussion whose catalogue is read. No caller identity: reading it
    /// is not a delegation, and an agent Kronn launched in a plain discussion
    /// has no room session to prove anyway.
    pub parent_discussion_id: String,
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
    #[serde(default)]
    pub source_agent: String,
    #[serde(default)]
    pub source_session_id: String,
    /// The room's own native agent, injected by the bridge from the runner's
    /// environment. Exclusive with `source_agent`/`source_session_id`.
    #[serde(default)]
    pub room_agent: Option<SpawnedAgentCaller>,
    #[serde(default)]
    pub worker_scope_intent: Option<TaskWorkerScopeIntent>,
    #[serde(default)]
    pub worker_scope: Option<TaskWorkerScope>,
    /// KT-1345 — integer or `"unlimited"`; absent = Settings > Workers.
    #[serde(default, deserialize_with = "explicit_review_rounds")]
    pub max_review_rounds: Option<serde_json::Value>,
    /// KT-1564 — providers this launch explicitly lets the execution fall back
    /// to when its provider refuses for quota. Absent or empty: never.
    #[serde(default)]
    pub quota_fallback_agents: Vec<crate::models::AgentType>,
}

#[derive(Deserialize, Default)]
pub struct TaskExecCallerRequest {
    #[serde(default)]
    pub source_agent: String,
    #[serde(default)]
    pub source_session_id: String,
    #[serde(default)]
    pub spawned_agent: Option<SpawnedAgentCaller>,
    /// The room's own native agent, injected by the bridge from the runner's
    /// environment. Exclusive with `source_agent`/`source_session_id`; it is the
    /// same principal identity `task_exec_launch` accepts, so the agent that
    /// launched an execution can also follow, decide and stop it.
    #[serde(default)]
    pub room_agent: Option<SpawnedAgentCaller>,
}

#[derive(Deserialize)]
pub struct TaskExecCancelRequest {
    #[serde(default)]
    pub source_agent: String,
    #[serde(default)]
    pub source_session_id: String,
    /// The room's own native agent, injected by the bridge from the runner's
    /// environment. Exclusive with `source_agent`/`source_session_id`; it is the
    /// same principal identity `task_exec_launch` accepts, so the agent that
    /// launched an execution can also follow, decide and stop it.
    #[serde(default)]
    pub room_agent: Option<SpawnedAgentCaller>,
    #[serde(default = "default_cancel_reason")]
    pub reason: String,
    #[serde(default)]
    pub cleanup_policy: Option<crate::models::CancellationCleanupPolicy>,
}

#[derive(Debug, Deserialize)]
pub struct TaskExecReassignRequest {
    #[serde(default)]
    pub source_agent: String,
    #[serde(default)]
    pub source_session_id: String,
    /// The room's own native agent, injected by the bridge from the runner's
    /// environment. Exclusive with `source_agent`/`source_session_id`; it is the
    /// same principal identity `task_exec_launch` accepts, so the agent that
    /// launched an execution can also follow, decide and stop it.
    #[serde(default)]
    pub room_agent: Option<SpawnedAgentCaller>,
    /// The same typed `MessageTarget` shape `agent_list` hands back verbatim
    /// and `task_exec_prepare`/`task_exec_launch` accept as `worker` — not
    /// the internal `CampaignWorkerSelection` envelope. `model`/`profile_id`
    /// are not publicly overridable here; a reassignment always resolves
    /// them from the target's tier, exactly like a fresh launch would.
    ///
    /// Exactly one of `worker` and `validations` per call — see
    /// `execution_amendment`.
    #[serde(default)]
    pub worker: Option<MessageTarget>,
    /// KT-839 — the complete new set of gates: it REPLACES the execution's
    /// validations, it is not merged into them. An empty list removes every gate.
    #[serde(default)]
    pub validations: Option<Vec<crate::models::ValidationSpec>>,
    /// KT-1345 — raise the execution's review-round limit instead: a number
    /// of rounds to add, or `"unlimited"`.
    #[serde(default)]
    pub extend_review_rounds: Option<serde_json::Value>,
    pub reason: String,
}

/// The one change a `task_exec_reassign` call makes to an existing execution.
///
/// Reassign is the principal's amendment tool for a live execution — principal-only,
/// reason-journaled, room and evidence kept — so replacing its validations lives
/// here rather than in a tool of its own (KT-839: every declaration is paid for on
/// every session, see `mcp_surface_budget.py`). The two changes are never combined:
/// each is journaled and refused on its own terms, and a half-applied pair would
/// be neither.
pub enum ExecutionAmendment {
    Reassign(MessageTarget),
    ReplaceValidations(Vec<crate::models::ValidationSpec>),
    /// KT-1345 — more review rounds, without an escalation + reassignment.
    ExtendReviewRounds(crate::db::orchestration::ReviewRoundsExtension),
}

pub fn execution_amendment(
    worker: Option<MessageTarget>,
    validations: Option<Vec<crate::models::ValidationSpec>>,
    extend_review_rounds: Option<&serde_json::Value>,
) -> Result<ExecutionAmendment, String> {
    let extension = extend_review_rounds
        .map(crate::db::orchestration::ReviewRoundsExtension::parse)
        .transpose()?;
    match (worker, validations, extension) {
        (Some(worker), None, None) => Ok(ExecutionAmendment::Reassign(worker)),
        (None, Some(validations), None) => Ok(ExecutionAmendment::ReplaceValidations(validations)),
        (None, None, Some(extension)) => Ok(ExecutionAmendment::ExtendReviewRounds(extension)),
        (None, None, None) => Err(
            "task_exec_reassign needs a change: a typed `worker` to reassign the execution, the \
             complete `validations` set to replace its gates ([] removes every gate), or \
             `extend_review_rounds` to raise its review-round limit"
                .into(),
        ),
        _ => Err(
            "task_exec_reassign makes one change per call: pass a `worker`, the complete \
             `validations` set, or `extend_review_rounds` — not several"
                .into(),
        ),
    }
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

/// Make the calling parent-room CLI the one this execution's notices address
/// (KT-790). Routing metadata only, so a failure is logged, never surfaced.
pub async fn pin_cli_principal(
    db: &Database,
    exec_id: &str,
    source_agent: &str,
    source_session_id: &str,
) {
    let (id, agent, session) = (
        exec_id.to_string(),
        source_agent.to_string(),
        source_session_id.to_string(),
    );
    let pinned = db
        .with_conn(move |conn| {
            let Some(session) =
                crate::db::discussion_sessions::find_active_session(conn, &agent, &session)?
            else {
                return Ok(false);
            };
            crate::db::orchestration::pin_principal_cli_session(conn, &id, session.id)
        })
        .await;
    if let Err(error) = pinned {
        tracing::warn!(execution = %exec_id, %error, "could not pin the principal CLI session");
    }
}

/// One answer for an unknown room, a stranger and a grant that is not the
/// room's: none of them learns which it was.
const PRINCIPAL_ROOM_REFUSAL: &str =
    "principal discussion not found or caller is not an active member";

/// Who is asking, as the room's principal, to prepare, launch, follow, review,
/// cancel, resume or reassign a task execution.
#[derive(Clone)]
enum PrincipalCaller {
    Cli { agent: String, session_id: String },
    RoomAgent(SpawnedAgentCaller),
}

fn principal_caller(
    source_agent: &str,
    source_session_id: &str,
    room_agent: Option<SpawnedAgentCaller>,
) -> Result<PrincipalCaller, &'static str> {
    let cli_given = !source_agent.trim().is_empty() || !source_session_id.trim().is_empty();
    match (room_agent, cli_given) {
        (Some(_), true) => Err("choose exactly one principal identity mode"),
        (Some(room), false) => Ok(PrincipalCaller::RoomAgent(room)),
        (None, _) => caller_fields(source_agent, source_session_id)
            .map(|(agent, session_id)| PrincipalCaller::Cli { agent, session_id })
            .ok_or("durable source_agent and source_session_id are required"),
    }
}

fn principal_is_authorized(
    conn: &rusqlite::Connection,
    parent_discussion_id: &str,
    caller: &PrincipalCaller,
) -> Result<bool> {
    match caller {
        PrincipalCaller::Cli { agent, session_id } => {
            principal_cli_is_authorized(conn, parent_discussion_id, agent, session_id)
        }
        PrincipalCaller::RoomAgent(room) => {
            room_agent_is_authorized(conn, parent_discussion_id, room)
        }
    }
}

/// Authorize a principal-only call on an EXISTING execution (status, review,
/// cancel, resume, reassign) with the same identities `task_exec_launch`
/// accepts. Before this, those calls took only a joined CLI session, so a
/// room's native agent could launch an execution and then never follow, decide
/// or stop it. An unknown execution and a stranger fuse into `None`.
async fn authorize_execution_principal(
    db: &Database,
    exec_id: &str,
    caller: &PrincipalCaller,
    bridge: Option<&crate::core::bridge_token::BridgeCaller>,
) -> Option<TaskExecution> {
    let (id, declared, grant) = (exec_id.to_string(), caller.clone(), bridge.cloned());
    db.with_conn(move |conn| {
        let Some(execution) = crate::db::orchestration::get_task_execution(conn, &id)? else {
            return Ok(None);
        };
        Ok(
            (principal_is_authorized(conn, &execution.parent_discussion_id, &declared)?
                && principal_session_is_the_grants(conn, grant.as_ref(), &declared, &execution)?)
            .then_some(execution),
        )
    })
    .await
    .ok()
    .flatten()
    .filter(|execution| bridge_holds_execution_principal(bridge, caller, execution))
}

/// KT-1954 — whether the caller is the principal of the latest execution of
/// `task_reference`, by the identities the execution tools accept. `Some` is
/// the agent and session the journal names; a worker, a stranger, an unknown
/// task and a task that never ran all answer `None`.
///
/// A room agent's identity is made of facts any reader of the room can see (its
/// running dispatch and the message that started it), so with a bridge token
/// the grant itself must be the principal's: it owns the parent room and not
/// the worker's.
pub fn live_check_principal(
    conn: &rusqlite::Connection,
    task_reference: &str,
    source_agent: &str,
    source_session_id: &str,
    room_agent: Option<SpawnedAgentCaller>,
    bridge: Option<&crate::core::bridge_token::BridgeCaller>,
) -> Result<Option<(String, String)>> {
    if room_agent.is_none() && caller_fields(source_agent, source_session_id).is_none() {
        return Ok(None);
    }
    let Ok(caller) = principal_caller(source_agent, source_session_id, room_agent) else {
        return Ok(None);
    };
    let Some(task) = crate::db::planning::get_task(conn, task_reference)? else {
        return Ok(None);
    };
    let Some(execution) =
        crate::db::orchestration::get_latest_execution_for_task(conn, &task.summary.id)?
    else {
        return Ok(None);
    };
    if !principal_is_authorized(conn, &execution.parent_discussion_id, &caller)? {
        return Ok(None);
    }
    match &caller {
        PrincipalCaller::Cli { agent, session_id } => {
            // The worker's own session is never its execution's principal.
            let session =
                crate::db::discussion_sessions::find_active_session(conn, agent, session_id)?;
            if session.is_none_or(|session| execution.worker_cli_session_id == Some(session.id)) {
                return Ok(None);
            }
            // A session id is a declared fact, readable in the journal: a
            // grant proves it only when it runs in the parent room and not in
            // the worker's. Whatever runs in that room is a principal by its
            // own session already (a workflow step that joined it included).
            if bridge.is_some_and(|bridge| !bridge_is_execution_principal(bridge, &execution))
                || !principal_session_is_the_grants(conn, bridge, &caller, &execution)?
            {
                return Ok(None);
            }
            Ok(Some((agent.clone(), session_id.clone())))
        }
        PrincipalCaller::RoomAgent(room) => {
            // Owning the room is not being its agent: a workflow step or
            // another launch hosted there reads the same running turn.
            if bridge.is_some_and(|bridge| {
                !bridge_is_execution_principal(bridge, &execution)
                    || !bridge_is_room_agent(bridge, room, &execution.parent_discussion_id)
            }) {
                return Ok(None);
            }
            Ok(Some((
                room.agent_type.trim().to_string(),
                room_agent_actor_session(room),
            )))
        }
    }
}

/// Make a CLI principal the one this execution's notices address. A room's
/// native agent has no durable session to pin: its room already receives them.
async fn pin_principal_caller(db: &Database, exec_id: &str, caller: &PrincipalCaller) {
    if let PrincipalCaller::Cli { agent, session_id } = caller {
        pin_cli_principal(db, exec_id, agent, session_id).await;
    }
}

/// The `actor_session_id` a room's native agent leaves in the planning journal:
/// `native:{room}:{message}`, the same shape the native HTTP tools write, so a
/// room agent's turn is never mistaken for a bare CLI session id.
fn room_agent_actor_session(room: &SpawnedAgentCaller) -> String {
    format!(
        "native:{}:{}",
        room.discussion_id.trim(),
        room.source_message_id.trim()
    )
}

/// The journal actor of a principal call: the CLI alias + session, or the room
/// agent's provider alias + `native:{room}:{message}` for the turn that made it.
fn principal_actor(caller: &PrincipalCaller) -> OrchestrationActor {
    match caller {
        PrincipalCaller::Cli { agent, session_id } => agent_actor(agent, Some(session_id)),
        PrincipalCaller::RoomAgent(room) => agent_actor(
            room.agent_type.trim(),
            Some(&room_agent_actor_session(room)),
        ),
    }
}

/// A room's native agent is the principal only while its own turn runs there:
/// the dispatch job must be running in that room, answer the same trigger,
/// belong to the same provider and not be a delegated worker's dispatch.
fn room_agent_is_authorized(
    conn: &rusqlite::Connection,
    parent_discussion_id: &str,
    caller: &SpawnedAgentCaller,
) -> Result<bool> {
    let Ok((agent_type, discussion_id, dispatch_job_id, source_message_id)) =
        spawned_native_caller(caller)
    else {
        return Ok(false);
    };
    if discussion_id != parent_discussion_id {
        return Ok(false);
    }
    let Some(job) = crate::db::agent_dispatch::get(conn, dispatch_job_id)? else {
        return Ok(false);
    };
    if job.discussion_id != parent_discussion_id
        || job.trigger_message_id != source_message_id
        || job.status != crate::db::agent_dispatch::DispatchStatus::Running
    {
        return Ok(false);
    }
    if crate::db::orchestration::get_execution_for_dispatch(conn, dispatch_job_id)?.is_some() {
        return Ok(false);
    }
    let room_agent: Option<String> = conn
        .query_row(
            "SELECT agent FROM discussions WHERE id = ?1",
            [parent_discussion_id],
            |row| row.get(0),
        )
        .optional()?;
    let expected = match (job.agent_override, room_agent) {
        (Some(agent), _) => agent,
        (None, Some(agent)) => crate::db::orchestration::agent_type_from_db(&agent)?,
        (None, None) => return Ok(false),
    };
    Ok(expected == agent_type)
}

pub fn execution_party_is_authorized(
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

fn execution_lookup(
    conn: &rusqlite::Connection,
    reference: &str,
    of: crate::core::uplink::ExecutionOf,
) -> Result<Option<TaskExecution>> {
    use crate::core::uplink::ExecutionOf;
    match of {
        ExecutionOf::ActiveForTask => {
            crate::db::orchestration::get_active_execution_for_task(conn, reference)
        }
        ExecutionOf::LatestForTask => {
            crate::db::orchestration::get_latest_execution_for_task(conn, reference)
        }
        ExecutionOf::Reference => resolve_task_execution_reference(conn, reference),
    }
}
inventory::submit! { crate::core::uplink::TaskExecutionLookup(execution_lookup) }
inventory::submit! {
    crate::core::uplink::TaskDodAdoption(crate::db::orchestration::adopt_task_dod_into_live_executions)
}

pub fn resolve_task_execution_reference(
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
        AgentType::OpenCode => "OpenCode",
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
    verdicts: &[(AgentType, crate::models::CatalogTierVerdict)],
) -> Vec<crate::models::TaskWorkerTier> {
    [ModelTier::Economy, ModelTier::Default, ModelTier::Reasoning]
        .into_iter()
        .map(|tier| {
            let configured =
                crate::agents::runner::resolve_model_flag(agent, tier, Some(model_tiers));
            let verdict = verdicts
                .iter()
                .find(|(kind, verdict)| kind == agent && verdict.tier == tier)
                .map(|(_, verdict)| verdict);
            match verdict {
                Some(verdict) if !verdict.launchable => crate::models::TaskWorkerTier {
                    tier,
                    resolved_model: verdict.requested_model.clone().or(configured),
                    refusal: Some(catalog_refusal_reason(tier, verdict)),
                    requested_model: None,
                    warning: None,
                },
                Some(verdict) => crate::models::TaskWorkerTier {
                    tier,
                    resolved_model: verdict.effective_model.clone().or(configured),
                    refusal: None,
                    requested_model: verdict
                        .requested_model
                        .clone()
                        .filter(|requested| verdict.effective_model.as_ref() != Some(requested)),
                    warning: catalog_notice_reason(verdict),
                },
                None => crate::models::TaskWorkerTier {
                    tier,
                    resolved_model: configured,
                    refusal: None,
                    requested_model: None,
                    warning: None,
                },
            }
        })
        .collect()
}

fn catalog_refusal_reason(
    tier: ModelTier,
    verdict: &crate::models::CatalogTierVerdict,
) -> crate::models::CampaignTaskReason {
    preparation_reason(
        "model_unavailable",
        format!(
            "the {tier:?} tier would be refused at launch for model `{}`: {}",
            verdict
                .requested_model
                .as_deref()
                .unwrap_or("runtime default"),
            verdict
                .detail
                .as_deref()
                .unwrap_or("the model catalogue refuses it"),
        ),
    )
}

fn catalog_notice_reason(
    verdict: &crate::models::CatalogTierVerdict,
) -> Option<crate::models::CampaignTaskReason> {
    verdict
        .notice
        .as_ref()
        .map(|notice| preparation_reason("model_not_listed", notice.clone()))
}

/// The verdict the launch preflight would reach for a native worker tier,
/// read from the durable catalogue without running discovery.
async fn native_tier_verdict(
    db: &crate::db::Database,
    agent: &AgentType,
    tier: ModelTier,
    model_tiers: &crate::models::ModelTiersConfig,
) -> crate::models::CatalogTierVerdict {
    let target = crate::core::model_catalog::legacy_runtime_target_id(db, agent).await;
    let decision = crate::core::model_catalog::launch_verdict(
        db,
        target.as_deref(),
        agent.clone(),
        tier,
        None,
        Some(model_tiers),
    )
    .await;
    let requested = match &decision {
        Ok(resolution) => resolution.requested_model.clone(),
        Err(failure) => failure.model_id.clone(),
    };
    crate::models::CatalogTierVerdict::from_decision(tier, requested, decision.map(Some))
}

async fn bounded_tier_verdicts(
    db: &crate::db::Database,
    model_tiers: &crate::models::ModelTiersConfig,
) -> Vec<(AgentType, crate::models::CatalogTierVerdict)> {
    let mut verdicts = Vec::new();
    for agent in CATALOGUED_PROVIDERS {
        for tier in [ModelTier::Economy, ModelTier::Default, ModelTier::Reasoning] {
            let verdict = native_tier_verdict(db, &agent, tier, model_tiers).await;
            verdicts.push((agent.clone(), verdict));
        }
    }
    verdicts
}

/// The catalogue verdict for a native worker's tier: a refusal the preflight
/// would make, else its pre-launch warning, so `task_exec_prepare` never
/// calls launchable what the run then rejects.
async fn worker_catalog_verdict(
    state: &AppState,
    worker: &MessageTarget,
) -> (
    Option<crate::models::CampaignTaskReason>,
    Option<crate::models::CampaignTaskReason>,
) {
    if worker.kind == MessageTargetKind::Cli || worker.connection_id.is_some() {
        return (None, None);
    }
    let tiers = state.config.read().await.agents.model_tiers.clone();
    let tier = worker.tier.unwrap_or_default();
    let verdict = native_tier_verdict(&state.db, &worker.agent_type, tier, &tiers).await;
    if verdict.launchable {
        (None, catalog_notice_reason(&verdict))
    } else {
        (Some(catalog_refusal_reason(tier, &verdict)), None)
    }
}

fn fixed_worker_reason(code: &str) -> crate::models::CampaignTaskReason {
    let detail = match code {
        "disabled" => "This provider is disabled in Kronn configuration.",
        "not_configured" => "No runnable host runtime or configured HTTP transport is available.",
        "auth_required" => "The authentication required by this provider is not configured.",
        "endpoint_unreachable" => {
            "The HTTP provider did not answer within the bounded discovery probe."
        }
        "connectivity_unverified" => {
            "No recent bounded probe result exists for this HTTP connection; Kronn does not \
             treat an unprobed address as reachable."
        }
        "model_not_configured" => {
            "No concrete model resolves for this HTTP provider; configure at least one tier."
        }
        "runtime_degraded" => {
            "The detected runtime reports degraded capabilities; inspect Agent settings."
        }
        "copilot_auth_invalid" => {
            "Copilot rejected the effective authentication during a bounded account probe; run `copilot login`."
        }
        "copilot_preflight_malformed" => {
            "Copilot completed its bounded account probe without a usable account response; verify `copilot login`."
        }
        "copilot_preflight_spawn_failed" => {
            "Kronn could not invoke Copilot's bounded account probe; verify the local Copilot CLI installation."
        }
        "copilot_preflight_timed_out" => {
            "Copilot did not finish its bounded account probe; retry later or reassign the execution."
        }
        "quota_exhausted" => {
            "This provider's plan/credit quota was recently reported exhausted by a real dispatch \
             attempt and the escalation is still open; reassign the affected execution (or wait for \
             it to be resolved) before relying on this worker again."
        }
        _ => "The worker is unavailable for a stable, backend-classified reason.",
    };
    preparation_reason(code, detail)
}

/// Everything the pure build needs that had to be observed first.
///
/// Grouped rather than passed one by one: each field is a bounded async
/// preflight the caller runs before building, and the list only grows. Empty
/// slices are the honest default — "nobody looked", never "the answer is no".
#[derive(Default)]
struct WorkerPreflight<'a> {
    http_reachability: &'a [(AgentType, bool)],
    cli: &'a [(AgentType, crate::agents::runner::CopilotTaskWorkerPreflight)],
    quota_exhausted: &'a [(AgentType, bool)],
    media_capabilities: &'a [MediaCapabilityEntry],
    tier_verdicts: &'a [(AgentType, crate::models::CatalogTierVerdict)],
    connection_reachability: &'a [(
        String,
        crate::core::endpoint_reachability::EndpointObservation,
    )],
}

/// What the catalogue says about a named connection's network. A missing or
/// expired observation is unverified: the absence of a probe is never success.
pub fn connection_connectivity(
    observation: Option<&crate::core::endpoint_reachability::EndpointObservation>,
    now: chrono::DateTime<chrono::Utc>,
) -> crate::models::WorkerConnectivity {
    use crate::core::endpoint_reachability::{ProbeOutcome, MAX_OBSERVATION_AGE};
    use crate::models::WorkerConnectivityState as State;
    let Some(observation) = observation else {
        return crate::models::WorkerConnectivity {
            state: State::Unverified,
            unreachable_reason: None,
            http_status: None,
            checked_at: None,
        };
    };
    let checked_at = Some(observation.checked_at.to_rfc3339());
    let expired = (now - observation.checked_at)
        .to_std()
        .is_ok_and(|age| age > MAX_OBSERVATION_AGE);
    if expired {
        return crate::models::WorkerConnectivity {
            state: State::Unverified,
            unreachable_reason: None,
            http_status: None,
            checked_at,
        };
    }
    match observation.outcome {
        ProbeOutcome::Reachable { http_status } => crate::models::WorkerConnectivity {
            state: State::Verified,
            unreachable_reason: None,
            http_status: Some(http_status),
            checked_at,
        },
        ProbeOutcome::Unreachable {
            reason,
            http_status,
        } => crate::models::WorkerConnectivity {
            state: State::Unreachable,
            unreachable_reason: Some(reason.as_str().to_string()),
            http_status,
            checked_at,
        },
    }
}

fn build_task_worker_catalogue(
    config: &crate::models::AppConfig,
    detections: &[crate::models::AgentDetection],
    joined: &[(
        crate::db::discussion_sessions::DiscussionSession,
        Option<String>,
    )],
    connections: &[crate::models::ExternalApiConnection],
    preflight: &WorkerPreflight<'_>,
) -> crate::models::TaskWorkerCatalogue {
    let mut workers = Vec::new();
    let native_agents = CATALOGUED_PROVIDERS;

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
        let observed = preflight
            .http_reachability
            .iter()
            .find(|(kind, _)| kind == &agent)
            .map(|(_, value)| *value);
        let reachable = if http {
            observed.unwrap_or(false)
        } else {
            runtime_present
        };
        let connectivity = http.then_some(crate::models::WorkerConnectivity {
            state: match observed {
                Some(true) => crate::models::WorkerConnectivityState::Verified,
                Some(false) => crate::models::WorkerConnectivityState::Unreachable,
                None => crate::models::WorkerConnectivityState::Unverified,
            },
            unreachable_reason: None,
            http_status: None,
            checked_at: None,
        });
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
        let tiers = worker_tiers(&agent, &config.agents.model_tiers, preflight.tier_verdicts);
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
        if agent == AgentType::CopilotCli {
            if let Some(status) = preflight
                .cli
                .iter()
                .find(|(kind, _)| kind == &agent)
                .map(|(_, status)| *status)
            {
                if let Some(code) = status.reason_code() {
                    reasons.push(fixed_worker_reason(code));
                }
            }
        }
        if preflight
            .quota_exhausted
            .iter()
            .find(|(kind, _)| kind == &agent)
            .is_some_and(|(_, exhausted)| *exhausted)
        {
            reasons.push(fixed_worker_reason("quota_exhausted"));
        }
        let worker = if http {
            MessageTarget::discussion_agent(agent.clone()).with_tier(ModelTier::Default)
        } else {
            MessageTarget::agent(agent.clone()).with_tier(ModelTier::Default)
        };
        if let Some(reason) = worker_static_refusal(&worker) {
            reasons.push(reason);
        }
        if let Some(refusal) = tiers
            .iter()
            .find(|tier| Some(tier.tier) == worker.tier)
            .and_then(|tier| tier.refusal.clone())
        {
            reasons.push(refusal);
        }
        let mut warnings = detection
            .and_then(|item| item.runtime_warning.as_ref())
            .map(|_| vec![fixed_worker_reason("runtime_degraded")])
            .unwrap_or_default();
        warnings.extend(
            tiers
                .iter()
                .find(|tier| Some(tier.tier) == worker.tier)
                .and_then(|tier| tier.warning.clone()),
        );
        workers.push(crate::models::TaskWorkerCatalogueEntry {
            worker,
            label: worker_label(&agent).to_string(),
            declared_model: None,
            configured,
            reachable,
            available: enabled && configured && reachable && reasons.is_empty(),
            tiers,
            media: Vec::new(),
            reasons,
            warnings,
            connectivity,
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
            media: Vec::new(),
            reasons,
            warnings: Vec::new(),
            connectivity: None,
        });
    }

    // External connections are workers too, and they were in none of the two
    // loops above: `Custom` has no local binary, so detection never produces it
    // and every surface derived from detection ignored it. A principal could
    // therefore not delegate to a configured OpenRouter at all, and its media
    // models were invisible even though `/api/media/generate` serves them.
    let now = chrono::Utc::now();
    for connection in connections {
        if connection
            .endpoint
            .as_ref()
            .is_none_or(|e| e.trim().is_empty())
        {
            continue;
        }
        let mut worker = crate::models::MessageTarget::agent(AgentType::Custom);
        worker.connection_id = Some(connection.id.clone());
        worker.tier = Some(crate::models::ModelTier::Default);

        let tiers = [
            (crate::models::ModelTier::Economy, &connection.economy_model),
            (crate::models::ModelTier::Default, &connection.default_model),
            (
                crate::models::ModelTier::Reasoning,
                &connection.reasoning_model,
            ),
        ]
        .into_iter()
        .map(|(tier, model)| crate::models::TaskWorkerTier {
            tier,
            resolved_model: model.clone().filter(|m| !m.trim().is_empty()),
            refusal: None,
            requested_model: None,
            warning: None,
        })
        .collect::<Vec<_>>();

        // Listed only when configured: an agent must never read that it can
        // produce a video the generation request would then refuse.
        let media = [
            (crate::models::MediaModality::Image, &connection.image_model),
            (crate::models::MediaModality::Video, &connection.video_model),
        ]
        .into_iter()
        .filter_map(|(modality, model)| {
            model
                .as_ref()
                .map(|m| m.trim())
                .filter(|m| !m.is_empty())
                .map(|m| crate::models::TaskWorkerModality {
                    modality,
                    model: m.to_string(),
                    // Carried, never re-derived: the agent reads exactly what
                    // the launcher shows. An envelope missing here means the
                    // catalogue could not be read, not that anything goes.
                    capabilities: preflight
                        .media_capabilities
                        .iter()
                        .find(|entry| {
                            entry.connection_id == connection.id
                                && entry.modality == modality
                                && entry.model == m
                        })
                        .map(|entry| entry.capabilities.clone()),
                })
        })
        .collect::<Vec<_>>();

        let has_text_model = tiers.iter().any(|tier| tier.resolved_model.is_some());
        let mut reasons = Vec::new();
        if !has_text_model && media.is_empty() {
            reasons.push(fixed_worker_reason("model_unconfigured"));
        }
        let connectivity = connection_connectivity(
            preflight
                .connection_reachability
                .iter()
                .find(|(id, _)| id == &connection.id)
                .map(|(_, observation)| observation),
            now,
        );
        let reachable = connectivity.state == crate::models::WorkerConnectivityState::Verified;
        match connectivity.state {
            crate::models::WorkerConnectivityState::Verified => {}
            crate::models::WorkerConnectivityState::Unreachable => {
                reasons.push(fixed_worker_reason("endpoint_unreachable"));
            }
            crate::models::WorkerConnectivityState::Unverified => {
                reasons.push(fixed_worker_reason("connectivity_unverified"));
            }
        }
        // Listed whatever the network says, so its id and media slots stay
        // discoverable; only `available` depends on the probe.
        workers.push(crate::models::TaskWorkerCatalogueEntry {
            worker,
            label: connection.display_name.clone(),
            declared_model: None,
            configured: true,
            reachable,
            available: reachable && reasons.is_empty(),
            tiers,
            media,
            reasons,
            warnings: Vec::new(),
            connectivity: Some(connectivity),
        });
    }

    crate::models::TaskWorkerCatalogue { workers }
}

fn apply_project_worker_refusals(
    catalogue: &mut crate::models::TaskWorkerCatalogue,
    project: &crate::models::Project,
) {
    for entry in &mut catalogue.workers {
        if let Some(reason) = worker_project_refusal(&entry.worker, project) {
            entry.reasons.push(reason);
            entry.available = false;
        }
    }
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
        tokio::time::timeout(
            timeout,
            crate::api::ollama_health::health(State(state.clone()))
        ),
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

/// One transport probe per named connection, all at once, each bounded and
/// served from the shared short-lived cache when warm.
async fn bounded_connection_reachability(
    connections: &[crate::models::ExternalApiConnection],
) -> Vec<(
    String,
    crate::core::endpoint_reachability::EndpointObservation,
)> {
    use crate::core::endpoint_reachability::{DEFAULT_BUDGET, SHARED};
    futures::future::join_all(connections.iter().filter_map(|connection| {
        let endpoint = connection
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())?;
        Some(async move {
            (
                connection.id.clone(),
                SHARED.observe(endpoint, DEFAULT_BUDGET).await,
            )
        })
    }))
    .await
}

async fn bounded_cli_worker_preflight(
    detections: &[crate::models::AgentDetection],
) -> Vec<(AgentType, crate::agents::runner::CopilotTaskWorkerPreflight)> {
    let Some(copilot) = detections
        .iter()
        .find(|item| item.agent_type == AgentType::CopilotCli)
    else {
        return Vec::new();
    };
    if !(copilot.installed || copilot.runtime_available) {
        return Vec::new();
    }
    // A native Copilot worker is refused whatever its account state
    // (`task_worker_route_policy`): no CLI is started to probe it.
    if crate::agents::runner::requires_explicit_full_access(&AgentType::CopilotCli) {
        return Vec::new();
    }
    let work_dir = std::env::temp_dir();
    // The shared runner helper owns the deadline and cancellation policy, so
    // catalogue discovery cannot abandon a live Copilot child at an outer
    // timeout boundary.
    let status = crate::agents::runner::probe_copilot_task_worker_preflight(
        "copilot",
        Some("@github/copilot"),
        &work_dir,
    )
    .await;
    vec![(AgentType::CopilotCli, status)]
}

/// KT-515 — native/HTTP providers eligible for the `agent_list` catalogue.
/// Kept in one place so the quota-exhaustion query below and
/// `build_task_worker_catalogue`'s `native_agents` list can never drift apart.
const CATALOGUED_PROVIDERS: [AgentType; 10] = [
    AgentType::Ollama,
    AgentType::LiteLlm,
    AgentType::Nvidia,
    AgentType::ClaudeCode,
    AgentType::Codex,
    AgentType::OpenCode,
    AgentType::GeminiCli,
    AgentType::Kiro,
    AgentType::CopilotCli,
    AgentType::Vibe,
];

/// One cheap read per catalogued provider: does it have an open, unresolved
/// quota escalation (KT-515)? No live process is spawned — this is a bounded
/// SQL preflight over already-recorded dispatch history.
async fn bounded_provider_quota_state(state: &AppState) -> Vec<(AgentType, bool)> {
    bounded_provider_quota_details(state)
        .await
        .into_iter()
        .map(|quota| (quota.provider, quota.blocked))
        .collect()
}

/// Same read as [`bounded_provider_quota_state`], plus the reset instant the
/// provider announced (KT-838). It is only reported for a provider that is
/// still blocked: once re-armed there is nothing left to wait for.
async fn bounded_provider_quota_details(
    state: &AppState,
) -> Vec<crate::models::ProviderQuotaState> {
    state
        .db
        .with_read_conn(move |conn| {
            CATALOGUED_PROVIDERS
                .iter()
                .map(|agent| {
                    let provider = crate::db::orchestration::agent_type_to_db(agent);
                    let blocked = crate::db::orchestration::provider_has_open_quota_exhaustion(
                        conn, &provider, None,
                    )?;
                    let reset_at = if blocked {
                        crate::db::orchestration::provider_quota_reset_at(conn, &provider)?
                    } else {
                        None
                    };
                    Ok(crate::models::ProviderQuotaState {
                        provider: agent.clone(),
                        blocked,
                        reset_at,
                    })
                })
                .collect::<Result<Vec<_>>>()
        })
        .await
        .unwrap_or_default()
}

/// One connection's advertised envelope for one modality.
///
/// Kept beside the catalogue rather than inside it so the builder stays pure:
/// reading a provider catalogue is a network call, and the two other bounded
/// preflights above are already the shape this follows.
pub struct MediaCapabilityEntry {
    pub connection_id: String,
    pub modality: crate::models::MediaModality,
    pub model: String,
    pub capabilities: crate::agents::media_capabilities::MediaModelCapabilities,
}

/// What each configured media model advertises, read exactly like the launcher.
///
/// An agent that cannot see the supported durations discovers them by being
/// refused — a wasted turn, and on some providers a billed one. Every failure
/// degrades to "not listed": a slow or unreachable catalogue must never hold
/// `agent_list` open. The slots are read concurrently and the ten-minute cache
/// is shared with the launcher, so a warm sweep costs nothing at all.
async fn bounded_media_capabilities(
    state: &AppState,
    connections: &[crate::models::ExternalApiConnection],
) -> Vec<MediaCapabilityEntry> {
    let wanted = connections
        .iter()
        .flat_map(|connection| {
            [
                crate::models::MediaModality::Image,
                crate::models::MediaModality::Video,
            ]
            .into_iter()
            .filter_map(move |modality| {
                crate::api::media::configured_media_model(connection, modality)
                    .map(|model| (connection, modality, model))
            })
        })
        .collect::<Vec<_>>();
    if wanted.is_empty() {
        return Vec::new();
    }

    // One window per slot rather than one for the sweep: a single provider
    // holding its connection open must cost its own entry, not everyone's.
    futures::future::join_all(
        wanted
            .into_iter()
            .map(|(connection, modality, model)| async move {
                let capabilities = tokio::time::timeout(
                    std::time::Duration::from_secs(8),
                    crate::api::media::advertised_capabilities(state, connection, &model, modality),
                )
                .await
                .ok()??;
                Some(MediaCapabilityEntry {
                    connection_id: connection.id.clone(),
                    modality,
                    model,
                    capabilities,
                })
            }),
    )
    .await
    .into_iter()
    .flatten()
    .collect()
}

/// The connections the catalogue lists, plus the local ComfyUI when a media
/// slot is set (KT-1381): it is reached like a connection, never stored.
fn with_local_media_source(
    mut connections: Vec<crate::models::ExternalApiConnection>,
    config: &crate::models::AppConfig,
) -> Vec<crate::models::ExternalApiConnection> {
    connections.extend(crate::api::comfyui::local_connection(&config.comfyui));
    connections
}

pub async fn task_worker_catalogue_for_discussion(
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
    let mut connections = state
        .db
        .with_read_conn(crate::db::external_api_connections::list)
        .await
        .unwrap_or_default();
    let mut detections = crate::agents::detect_all_cached(false).await;
    let config = state.config.read().await.clone();
    connections = with_local_media_source(connections, &config);
    crate::agents::apply_configured_status(&mut detections, &config);
    // The network reads share one window rather than adding up.
    let (reachability, connection_reachability, media_capabilities) = tokio::join!(
        bounded_http_worker_reachability(state),
        bounded_connection_reachability(&connections),
        bounded_media_capabilities(state, &connections),
    );
    let cli_preflight = bounded_cli_worker_preflight(&detections).await;
    let quota_state = bounded_provider_quota_state(state).await;
    let tier_verdicts = bounded_tier_verdicts(&state.db, &config.agents.model_tiers).await;
    Ok(build_task_worker_catalogue(
        &config,
        &detections,
        &joined,
        &connections,
        &WorkerPreflight {
            http_reachability: &reachability,
            cli: &cli_preflight,
            quota_exhausted: &quota_state,
            media_capabilities: &media_capabilities,
            tier_verdicts: &tier_verdicts,
            connection_reachability: &connection_reachability,
        },
    ))
}

pub async fn target_aware_task_worker_catalogue_for_discussion(
    state: &AppState,
    parent_discussion_id: &str,
) -> Result<crate::models::TaskWorkerCatalogue> {
    let mut catalogue = task_worker_catalogue_for_discussion(state, parent_discussion_id).await?;
    let parent = parent_discussion_id.to_string();
    let project = state
        .db
        .with_read_conn(move |conn| {
            crate::db::discussions::get_discussion(conn, &parent)?
                .and_then(|discussion| discussion.project_id)
                .map(|project_id| crate::db::projects::get_project(conn, &project_id))
                .transpose()
                .map(Option::flatten)
        })
        .await?;
    if let Some(project) = project.as_ref() {
        apply_project_worker_refusals(&mut catalogue, project);
    }
    Ok(catalogue)
}

/// Browser-only quota signal for Config > Agents. This route is not exposed by
/// the MCP worker surface, so an agent cannot re-arm a provider autonomously.
pub async fn provider_quota_states(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<crate::models::ProviderQuotaState>>> {
    Json(ApiResponse::ok(
        bounded_provider_quota_details(&state).await,
    ))
}

#[derive(Deserialize)]
pub struct RearmProviderQuotaRequest {
    /// The browser only sends this after its confirmation dialog. Keeping the
    /// acknowledgement in the request makes an accidental POST fail closed.
    pub confirmed: bool,
    pub idempotency_key: String,
}

/// Human-confirmed browser action. It only advances the provider
/// acknowledgement watermark; it never launches, cancels or rewrites an
/// execution.
pub async fn rearm_provider_quota(
    State(state): State<AppState>,
    Path(provider): Path<AgentType>,
    Json(request): Json<RearmProviderQuotaRequest>,
) -> Json<ApiResponse<bool>> {
    let key = request.idempotency_key.trim().to_string();
    if !request.confirmed || key.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "a confirmed re-arm and idempotency_key are required",
        ));
    }
    let provider = crate::db::orchestration::agent_type_to_db(&provider);
    match state
        .db
        .with_conn(move |conn| {
            crate::db::orchestration::rearm_provider_quota(
                conn,
                &provider,
                &key,
                PlanningActorKind::Human.as_str(),
                Some("web-operator"),
            )
        })
        .await
    {
        Ok(changed) => Json(ApiResponse::ok(changed)),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

/// MCP-only worker discovery. Caller identity is injected by the bridge and
/// authorized against the principal room before any room/session catalogue is
/// returned.
/// The agents and connections a discussion can reach, and the media each
/// connection generates. A read, like `disc_meta`: nothing in it is secret or
/// scoped to one party. It used to sit behind the room-membership guard the
/// delegation tools need, which an agent Kronn launches in a plain discussion
/// can never pass, so `media_generate` had no way to learn a connection id.
/// `task_exec_prepare` and `task_exec_launch` keep that guard.
pub async fn task_worker_catalogue(
    State(state): State<AppState>,
    Json(request): Json<TaskWorkerCatalogueRequest>,
) -> Json<ApiResponse<crate::models::TaskWorkerCatalogue>> {
    let parent = request.parent_discussion_id.trim().to_string();
    if parent.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "parent_discussion_id is required",
        ));
    }
    let exists = {
        let parent = parent.clone();
        state
            .db
            .with_read_conn(move |conn| {
                Ok(crate::db::discussions::get_discussion(conn, &parent)?.is_some())
            })
            .await
    };
    if !matches!(exists, Ok(true)) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            format!(
                "discussion `{parent}` not found: agent_list reads the catalogue of the \
                 discussion this bridge is bound to, so bind or reopen that discussion"
            ),
        ));
    }
    match target_aware_task_worker_catalogue_for_discussion(&state, &parent).await {
        Ok(catalogue) => Json(ApiResponse::ok(catalogue)),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

/// MCP-only preflight. Caller identity is injected by the bridge and checked in
/// the backend before task/project/worker details are returned.
pub async fn task_exec_prepare(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Json(mut request): Json<TaskExecPrepareRequest>,
) -> Json<ApiResponse<crate::models::TaskExecutionPreparation>> {
    let caller = match principal_caller(
        &request.source_agent,
        &request.source_session_id,
        request.room_agent.take(),
    ) {
        Ok(caller) => caller,
        Err(message) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, message)),
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
    let bridge = bridge.as_ref().map(|extension| &extension.0);
    if !bridge_holds_room_principal(bridge, &caller, &parent) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            PRINCIPAL_ROOM_REFUSAL,
        ));
    }
    let task = request.task_reference.trim().to_string();
    let worker = request.worker;
    let base_rev = request.base_rev.clone();
    let scope_refusal = worker_scope_refusal(&worker, request.worker_scope.as_ref());
    let validations_refusal = validations_refusal(&request.validations);
    let full_suite_warnings = full_suite_validation_warnings(&request.validations);
    let rounds_refusal = match review_rounds_argument(request.max_review_rounds.as_ref()) {
        Ok(_) => None,
        Err(detail) => Some(preparation_reason("invalid_max_review_rounds", detail)),
    };
    let default_rounds = state
        .config
        .read()
        .await
        .workers
        .effective_max_review_rounds();
    let (catalog_refusal, catalog_warning) = worker_catalog_verdict(&state, &worker).await;
    reclaim_finished_worktrees_for_launch(&state.db, &task, &worker).await;
    let result = state
        .db
        .with_conn(move |conn| {
            if !principal_is_authorized(conn, &parent, &caller)? {
                bail!(PRINCIPAL_ROOM_REFUSAL);
            }
            let mut preparation = prepare_task_execution(conn, &task, &parent, &worker)?;
            integration_target_preparation(conn, &mut preparation, base_rev.as_deref())?;
            preparation.warnings.extend(catalog_warning);
            preparation.warnings.extend(full_suite_warnings);
            preparation.default_max_review_rounds =
                Some(crate::models::review_limit_json(default_rounds));
            for reason in [
                scope_refusal,
                validations_refusal,
                rounds_refusal,
                catalog_refusal,
            ]
            .into_iter()
            .flatten()
            {
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
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Json(mut request): Json<TaskExecLaunchRequest>,
) -> Json<ApiResponse<TaskExecution>> {
    let caller = match principal_caller(
        &request.source_agent,
        &request.source_session_id,
        request.room_agent.take(),
    ) {
        Ok(caller) => caller,
        Err(message) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, message)),
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
    let cli_principal = match &caller {
        PrincipalCaller::Cli { agent, session_id } => Some((agent.clone(), session_id.clone())),
        PrincipalCaller::RoomAgent(_) => None,
    };
    let bridge = bridge.as_ref().map(|extension| &extension.0);
    let authorized = bridge_holds_room_principal(bridge, &caller, &parent) && {
        let parent = parent.clone();
        matches!(
            state
                .db
                .with_conn(move |conn| principal_is_authorized(conn, &parent, &caller))
                .await,
            Ok(true)
        )
    };
    if !authorized {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            PRINCIPAL_ROOM_REFUSAL,
        ));
    }
    if let Some(reason) = worker_scope_refusal(&request.worker, request.worker_scope.as_ref()) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            format!("{}: {}", reason.code, reason.detail),
        ));
    }
    let max_review_rounds = match review_rounds_argument(request.max_review_rounds.as_ref()) {
        Ok(rounds) => rounds,
        Err(detail) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, detail)),
    };
    let (catalog_refusal, _) = worker_catalog_verdict(&state, &request.worker).await;
    let preflight = {
        let task = request.task_reference.trim().to_string();
        let principal = parent.clone();
        let worker = request.worker.clone();
        reclaim_finished_worktrees_for_launch(&state.db, &task, &worker).await;
        state
            .db
            .with_conn(move |conn| {
                let mut preparation = prepare_task_execution(conn, &task, &principal, &worker)?;
                if let Some(reason) = catalog_refusal {
                    preparation.launchable = false;
                    preparation.reasons.push(reason);
                }
                Ok(preparation)
            })
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
    match provision_single_task_execution_with_options(
        &state,
        ProvisionInput {
            task_reference: request.task_reference,
            parent_discussion_id: parent,
            worker: request.worker,
            base_rev: request.base_rev,
            idempotency_key: request.idempotency_key,
        },
        request.worker_scope,
        request.validations,
        SingleTaskLaunchOptions {
            max_review_rounds,
            quota_fallback_agents: request.quota_fallback_agents,
        },
    )
    .await
    {
        Ok(execution) => {
            if let Some((agent, session_id)) = cli_principal {
                pin_cli_principal(&state.db, &execution.id, &agent, &session_id).await;
            }
            Json(ApiResponse::ok(execution))
        }
        Err(error) => Json(provision_error_to_response(error)),
    }
}

/// Bounds of the blocking `task_exec_status` wait (KT-790). The ceiling stays
/// below the bridge's 180 s HTTP timeout, like `disc_wait_for_peer`'s.
const EXEC_WAIT_DEFAULT_SECS: u64 = 60;
const EXEC_WAIT_MAX_SECS: u64 = 170;
const EXEC_WAIT_POLL_MS: u64 = 500;

/// How a `wait_for` status read ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ExecutionWaitOutcome {
    pub matched: bool,
    pub timed_out: bool,
    pub waited_ms: u64,
}

/// Hold until the execution is in one of `wait_for`, becomes terminal or the
/// bounded timeout expires. A state already reached returns at once.
async fn wait_for_execution_status(
    db: &Database,
    exec_id: &str,
    wait_for: &[TaskExecutionStatus],
    timeout_secs: Option<u64>,
) -> Result<ExecutionWaitOutcome> {
    let budget = std::time::Duration::from_secs(
        timeout_secs
            .unwrap_or(EXEC_WAIT_DEFAULT_SECS)
            .clamp(1, EXEC_WAIT_MAX_SECS),
    );
    let started = std::time::Instant::now();
    loop {
        let id = exec_id.to_string();
        let status = db
            .with_read_conn(move |conn| {
                let status: String = conn.query_row(
                    "SELECT status FROM task_executions WHERE id = ?1",
                    [&id],
                    |row| row.get(0),
                )?;
                status.parse::<TaskExecutionStatus>()
            })
            .await?;
        let elapsed = started.elapsed();
        let matched = wait_for.contains(&status);
        if matched || status.is_terminal() || elapsed >= budget {
            return Ok(ExecutionWaitOutcome {
                matched,
                timed_out: !matched && !status.is_terminal(),
                waited_ms: elapsed.as_millis() as u64,
            });
        }
        let poll = std::time::Duration::from_millis(EXEC_WAIT_POLL_MS);
        tokio::time::sleep(poll.min(budget - elapsed)).await;
    }
}

/// Which projection `task_exec_status` returns. `Full` stays the default: worker
/// briefs and reviews read its lineage, attempts and manifests (KT-791).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskExecStatusView {
    #[default]
    Full,
    Compact,
}

/// The compact view is measured as the bridge prints it (2-space JSON) and
/// trimmed until it fits under 1 000 characters.
pub const COMPACT_STATUS_MAX_CHARS: usize = 960;
const COMPACT_ERROR_MAX_CHARS: usize = 160;
const COMPACT_COMMAND_MAX_CHARS: usize = 60;
const COMPACT_VALIDATIONS_MAX: usize = 3;
const COMPACT_SESSIONS_MAX: usize = 2;

#[derive(Debug, Clone, Serialize)]
pub struct CompactValidation {
    pub command: String,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompactNextAction {
    pub tool: Option<&'static str>,
    pub reason: &'static str,
}

/// What a polling principal needs to decide its next call, in under 1 000
/// characters; `view: full` keeps the whole projection.
#[derive(Debug, Clone, Serialize)]
pub struct TaskExecutionCompactStatus {
    pub id: String,
    pub task: String,
    pub status: TaskExecutionStatus,
    pub attempt: u32,
    pub review_rounds: String,
    /// KT-1688 — the branch the integration will advance, and the worktree that
    /// holds it, so a wrong target is visible before integration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integration_worktree: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integration_target_state: Option<crate::models::IntegrationTargetState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub validations: Vec<CompactValidation>,
    /// The worker's most recent CLI sessions, newest last: the names of its
    /// transcripts (KT-911). `view: full` lists them all, attempts included.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub worker_sessions: Vec<String>,
    /// KT-1632 — why a launch waits for its worker to start: the task-worker
    /// cap is full. Absent once the worker runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued: Option<String>,
    pub next_action: Option<CompactNextAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait: Option<ExecutionWaitOutcome>,
}

fn clip_chars(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut clipped: String = flat.chars().take(max.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

/// The same resumable holds the bridge announces on the full view, plus the
/// call each other state expects from the principal.
pub fn compact_next_action(
    detail: &crate::models::TaskExecutionDetail,
) -> Option<CompactNextAction> {
    use crate::models::ExecutionRecoveryAction as Action;
    use TaskExecutionStatus::*;
    let execution = &detail.lineage.execution;
    let integration_recovery = detail.recovery.as_ref().is_some_and(|recovery| {
        recovery.pending
            && matches!(
                recovery.recovery_action,
                Action::RebuildCandidate
                    | Action::RunValidations
                    | Action::ApplyFastForward
                    | Action::IdempotentClose
            )
    });
    // KT-1540 — a worker that ended without delivering is parked for one resume.
    if execution.status == Interrupted
        && detail.recovery.as_ref().is_some_and(|recovery| {
            recovery.pending
                && recovery.recovery_action == Action::ResumeWorker
                && crate::db::orchestration::is_undelivered_worker_reason(&recovery.recovery_reason)
        })
    {
        return Some(CompactNextAction {
            tool: Some("task_exec_resume"),
            reason: "the worker ended without delivering: resume it once (its worktree is kept), or reassign / cancel",
        });
    }
    let applying_hold = execution.blocked_from_status == Some(Applying);
    let resumable = (execution.status == Blocked && applying_hold)
        || (execution.status == Interrupted
            && execution.interrupted_from_status == Some(Blocked)
            && applying_hold)
        || (execution.status == Interrupted && integration_recovery)
        || (execution.status == Approved && execution.blocked_reason_code.is_some());
    if resumable {
        return Some(CompactNextAction {
            tool: Some("task_exec_resume"),
            reason: "integration hold: resume once its cause is fixed",
        });
    }
    let (tool, reason) = match execution.status {
        Done | Failed | Cancelled => return None,
        AwaitingReview => (Some("task_exec_review"), "review the delivery at head_sha"),
        Blocked
            if execution.blocked_reason_code
                == Some(crate::models::BlockedReasonCode::AwaitingWorkerAcceptance) =>
        {
            (
                Some("task_exec_status"),
                "the worker has not accepted its offer yet; wait_for Working",
            )
        }
        Blocked | Interrupted => (
            None,
            "decide: read last_error and view full, then reassign or cancel",
        ),
        Escalated => (None, "a human decision is required; see view full"),
        Pending | Provisioning | Working | ChangesRequested | Approved | Integrating
        | Validating | Applying => (
            Some("task_exec_status"),
            "in progress: wait_for AwaitingReview, Done or Blocked",
        ),
    };
    Some(CompactNextAction { tool, reason })
}

/// The sentence a queued launch reports in place of a silent `Working`: the
/// cap that holds it, how full it is and its place in the queue.
fn task_worker_queue_reason(progress: &crate::models::TaskExecutionProgress) -> Option<String> {
    if progress.phase != crate::models::TaskExecutionProgressPhase::Queued {
        return None;
    }
    let counts = progress
        .reason
        .as_deref()?
        .strip_prefix(crate::db::agent_dispatch::TASK_WORKER_QUEUED_DETAIL_PREFIX)?;
    let (running, cap) = counts.split_once('/')?;
    let place = progress
        .queue_position
        .map(|position| format!(", position {position}"))
        .unwrap_or_default();
    Some(format!(
        "queued: {running} of {cap} local task workers running (max_concurrent_task_workers){place}; starts when a slot frees"
    ))
}

fn compact_last_error(detail: &crate::models::TaskExecutionDetail) -> Option<String> {
    let execution = &detail.lineage.execution;
    let outcome = execution
        .outcome_reason
        .as_deref()
        .filter(|_| execution.status != TaskExecutionStatus::Done);
    let dispatch = (detail.progress.phase == crate::models::TaskExecutionProgressPhase::Failed)
        .then_some(detail.progress.reason.as_deref())
        .flatten();
    // An undelivered-worker reason only says the turn ended without a delivery;
    // a failed dispatch (a refused preflight…) is the cause.
    let recovery = detail
        .recovery
        .as_ref()
        .filter(|recovery| recovery.pending)
        .map(|recovery| recovery.recovery_reason.as_str())
        .filter(|reason| {
            dispatch.is_none() || !crate::db::orchestration::is_undelivered_worker_reason(reason)
        });
    // KT-1402 — a refused delivery is the cause; the recovery reason only says
    // the worker ended without one.
    let refusal = detail
        .last_delivery_refusal
        .as_ref()
        .map(|detail| format!("delivery refused: {detail}"));
    execution
        .blocked_reason
        .clone()
        .or(refusal)
        .or_else(|| outcome.map(str::to_string))
        .or_else(|| recovery.map(str::to_string))
        .or_else(|| dispatch.map(str::to_string))
        .filter(|text| !text.trim().is_empty())
        .map(|text| clip_chars(&text, COMPACT_ERROR_MAX_CHARS))
}

/// Latest candidate's validation runs, most recent last.
fn compact_validations(detail: &crate::models::TaskExecutionDetail) -> Vec<CompactValidation> {
    let candidate = detail
        .lineage
        .execution
        .candidate_merge_sha
        .clone()
        .or_else(|| {
            detail
                .validation_runs
                .last()
                .and_then(|run| run.candidate_merge_sha.clone())
        });
    let runs: Vec<_> = detail
        .validation_runs
        .iter()
        .filter(|run| run.candidate_merge_sha == candidate)
        .collect();
    runs[runs.len().saturating_sub(COMPACT_VALIDATIONS_MAX)..]
        .iter()
        .map(|run| CompactValidation {
            command: clip_chars(&run.command, COMPACT_COMMAND_MAX_CHARS),
            exit_code: run.exit_code,
            duration_ms: run.duration_ms,
        })
        .collect()
}

pub fn compact_status_chars(compact: &TaskExecutionCompactStatus) -> usize {
    serde_json::to_string_pretty(compact)
        .map(|text| text.chars().count())
        .unwrap_or(usize::MAX)
}

pub fn compact_execution_status(
    detail: &crate::models::TaskExecutionDetail,
    wait: Option<ExecutionWaitOutcome>,
) -> TaskExecutionCompactStatus {
    let execution = &detail.lineage.execution;
    let mut compact = TaskExecutionCompactStatus {
        id: execution.id.clone(),
        task: detail.lineage.task_reference.clone(),
        status: execution.status,
        attempt: execution.attempt_no,
        review_rounds: format!(
            "{}/{}",
            execution.review_rounds,
            crate::models::review_limit_label(execution.max_review_rounds)
        ),
        target_branch: detail.target_branch.clone(),
        integration_worktree: detail
            .integration_target
            .as_ref()
            .and_then(|target| target.worktree.clone()),
        integration_target_state: detail
            .integration_target
            .as_ref()
            .map(|target| target.state),
        head_sha: detail
            .attempts
            .iter()
            .rev()
            .find_map(|attempt| attempt.delivery.as_ref())
            .map(|delivery| delivery.head_sha.clone()),
        last_error: compact_last_error(detail),
        validations: compact_validations(detail),
        worker_sessions: detail.worker_sessions[detail
            .worker_sessions
            .len()
            .saturating_sub(COMPACT_SESSIONS_MAX)..]
            .iter()
            .map(|session| session.session_id.clone())
            .collect(),
        queued: task_worker_queue_reason(&detail.progress),
        next_action: compact_next_action(detail),
        wait,
    };
    while compact_status_chars(&compact) > COMPACT_STATUS_MAX_CHARS {
        // The decision fields outrank the transcript names: drop those first.
        if !compact.worker_sessions.is_empty() {
            compact.worker_sessions.remove(0);
        } else if !compact.validations.is_empty() {
            compact.validations.remove(0);
        } else if let Some(error) = compact.last_error.take() {
            let shorter = error.chars().count() / 2;
            if shorter >= 16 {
                compact.last_error = Some(clip_chars(&error, shorter));
            }
        } else {
            break;
        }
    }
    compact
}

/// Body of `POST /api/orchestration/tool/executions/{id}/status`.
#[derive(Deserialize, Default)]
pub struct TaskExecStatusRequest {
    #[serde(flatten)]
    pub caller: TaskExecCallerRequest,
    #[serde(default)]
    pub view: TaskExecStatusView,
    /// Block until the execution reaches one of these states (KT-790).
    #[serde(default)]
    pub wait_for: Vec<TaskExecutionStatus>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

pub async fn task_exec_status(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Json(request): Json<TaskExecStatusRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let TaskExecStatusRequest {
        caller: request,
        view,
        wait_for,
        timeout_secs,
    } = request;
    let bridge = bridge.map(|extension| extension.0);
    // KT-1408 — a spawned worker is told how close the execution limit is.
    let mut worker_limit_clock: Option<(AgentType, String)> = None;
    let execution_id = if let Some(caller) = request.spawned_agent {
        if !request.source_agent.trim().is_empty()
            || !request.source_session_id.trim().is_empty()
            || request.room_agent.is_some()
        {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                "choose exactly one status identity mode",
            ));
        }
        let (agent_type, discussion_id, dispatch_job_id, source_message_id) =
            match spawned_native_caller(&caller) {
                Ok(caller) => caller,
                Err(message) => {
                    return Json(ApiResponse::err_coded(
                        ApiErrorCode::Validation,
                        message.to_string(),
                    ))
                }
            };
        let alias = crate::db::orchestration::agent_type_to_db(&agent_type);
        worker_limit_clock = Some((agent_type.clone(), dispatch_job_id.to_string()));
        match spawned_native_worker_execution_for_caller(
            &state.db,
            &exec_id,
            dispatch_job_id,
            NativeExecutionCaller {
                discussion_id,
                agent_type: &agent_type,
                source_message_id: Some(source_message_id),
                alias: &alias,
                actor_session_id: Some(source_message_id),
            },
            bridge.as_ref(),
        )
        .await
        {
            Ok(Some(execution)) => execution.id,
            Ok(None) => {
                return Json(ApiResponse::err_coded(
                    ApiErrorCode::NotFound,
                    "execution not found or caller is not a party",
                ))
            }
            Err(error) => {
                let (code, message) = provision_error_parts(&error);
                return Json(ApiResponse::err_coded(code, message));
            }
        }
    } else if let Some(room) = request.room_agent {
        // The room's native agent follows the executions it launched as their
        // principal, exactly like a joined CLI session in the parent room.
        let caller = match principal_caller(
            &request.source_agent,
            &request.source_session_id,
            Some(room),
        ) {
            Ok(caller) => caller,
            Err(message) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, message)),
        };
        let authorized = state
            .db
            .with_conn(move |conn| {
                let execution = resolve_task_execution_reference(conn, &exec_id)?
                    .context("execution not found or caller is not a party")?;
                if !principal_is_authorized(conn, &execution.parent_discussion_id, &caller)?
                    || !bridge_holds_execution_principal(bridge.as_ref(), &caller, &execution)
                {
                    bail!("execution not found or caller is not a party");
                }
                Ok(execution.id)
            })
            .await;
        match authorized {
            Ok(id) => id,
            Err(error) => {
                return Json(ApiResponse::err_coded(
                    ApiErrorCode::NotFound,
                    error.to_string(),
                ))
            }
        }
    } else {
        let Some((agent, session_id)) =
            caller_fields(&request.source_agent, &request.source_session_id)
        else {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                "durable source_agent and source_session_id are required",
            ));
        };
        let authorized = state
            .db
            .with_conn(move |conn| {
                let execution = resolve_task_execution_reference(conn, &exec_id)?
                    .context("execution not found or caller is not a party")?;
                if !execution_party_is_authorized(conn, &execution, &agent, &session_id)?
                    || !bridge_is_execution_party(bridge.as_ref(), &execution)
                {
                    bail!("execution not found or caller is not a party");
                }
                Ok(execution.id)
            })
            .await;
        match authorized {
            Ok(id) => id,
            Err(error) => {
                return Json(ApiResponse::err_coded(
                    ApiErrorCode::NotFound,
                    error.to_string(),
                ))
            }
        }
    };
    let wait = if wait_for.is_empty() {
        None
    } else {
        match wait_for_execution_status(&state.db, &execution_id, &wait_for, timeout_secs).await {
            Ok(outcome) => Some(outcome),
            Err(error) => {
                return Json(ApiResponse::err_coded(
                    ApiErrorCode::Internal,
                    error.to_string(),
                ))
            }
        }
    };
    let detail = state
        .db
        .with_conn(move |conn| execution_detail(conn, &execution_id))
        .await;
    let mut detail = match detail {
        Ok(detail) => detail,
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                error.to_string(),
            ))
        }
    };
    attach_runtime_liveness(&state, &mut detail);
    let body = match view {
        TaskExecStatusView::Compact => {
            serde_json::to_value(compact_execution_status(&detail, wait))
        }
        TaskExecStatusView::Full => serde_json::to_value(&detail).map(|mut body| {
            if let (Some(wait), Some(object)) = (wait, body.as_object_mut()) {
                object.insert("wait".into(), serde_json::json!(wait));
            }
            body
        }),
    };
    let time_limit = match worker_limit_clock {
        Some((agent_type, dispatch_job_id)) => {
            crate::api::worker_time_limit::worker_time_limit_notice(
                &state,
                &dispatch_job_id,
                &agent_type,
            )
            .await
        }
        None => None,
    };
    match body {
        Ok(mut body) => {
            if let (Some(time_limit), Some(object)) = (time_limit, body.as_object_mut()) {
                object.insert("time_limit".into(), time_limit);
            }
            Json(ApiResponse::ok(body))
        }
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            error.to_string(),
        )),
    }
}

pub async fn task_exec_resume(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Json(request): Json<TaskExecCallerRequest>,
) -> Json<ApiResponse<ExecutionRecoveryView>> {
    if request.spawned_agent.is_some() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "task execution resume is available only to its principal",
        ));
    }
    let caller = match principal_caller(
        &request.source_agent,
        &request.source_session_id,
        request.room_agent,
    ) {
        Ok(caller) => caller,
        Err(message) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, message)),
    };
    let bridge = bridge.as_ref().map(|extension| &extension.0);
    let Some(execution) = authorize_execution_principal(&state.db, &exec_id, &caller, bridge).await
    else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "execution not found or caller is not its principal",
        ));
    };
    pin_principal_caller(&state.db, &execution.id, &caller).await;
    resume_execution(State(state), Path(execution.id)).await
}

pub async fn task_exec_cancel(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Json(request): Json<TaskExecCancelRequest>,
) -> Json<ApiResponse<ExecutionRecoveryView>> {
    let caller = match principal_caller(
        &request.source_agent,
        &request.source_session_id,
        request.room_agent,
    ) {
        Ok(caller) => caller,
        Err(message) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, message)),
    };
    let bridge = bridge.as_ref().map(|extension| &extension.0);
    let Some(execution) = authorize_execution_principal(&state.db, &exec_id, &caller, bridge).await
    else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "execution not found or caller is not its principal",
        ));
    };
    cancel_execution(
        State(state),
        Path(execution.id),
        Json(CancelExecutionRequest {
            reason: request.reason,
            cleanup_policy: request.cleanup_policy,
        }),
    )
    .await
}

/// The route handler: the bridge extension is what tells a Kronn-launched agent
/// (which may be the execution's own worker) from the operator or a browser.
pub async fn task_exec_reassign_http(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Json(request): Json<TaskExecReassignRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    task_exec_reassign_as(
        state,
        exec_id,
        request,
        bridge.as_ref().map(|extension| &extension.0),
    )
    .await
}

/// The same call without a bridge token (no Kronn-launched agent behind it).
pub async fn task_exec_reassign(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
    Json(request): Json<TaskExecReassignRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    task_exec_reassign_as(state, exec_id, request, None).await
}

/// Whether a bridge-token caller is this execution's principal, read from what
/// the token was minted for and never from the body: it owns the execution's
/// parent room and is not its worker (a worker's token owns the child room).
///
/// A token that owns BOTH rooms is refused too: that is what a principal's token
/// looks like after it adopted the child room, and it is indistinguishable from a
/// worker that was also granted the parent. Such a principal extends through a
/// token that owns only the parent room, or through the human UI.
fn bridge_is_execution_principal(
    bridge: &crate::core::bridge_token::BridgeCaller,
    execution: &TaskExecution,
) -> bool {
    let owns = |room: &str| bridge.own_discussions.iter().any(|own| own == room);
    owns(&execution.parent_discussion_id)
        && !execution.sub_discussion_id.as_deref().is_some_and(owns)
        && bridge.own_execution.as_deref() != Some(execution.id.as_str())
}

/// KT-1962 — whether a bridge-token caller may use `caller` as the principal
/// of `execution`. The body's agent, session and room-agent context are facts
/// any reader of the room can copy, so the grant itself must be the
/// principal's; a room agent's must also be the grant of that room's agent.
/// No token (the operator, a host CLI, a browser) is not judged here.
fn bridge_holds_execution_principal(
    bridge: Option<&crate::core::bridge_token::BridgeCaller>,
    caller: &PrincipalCaller,
    execution: &TaskExecution,
) -> bool {
    bridge.is_none_or(|bridge| {
        bridge_is_execution_principal(bridge, execution)
            && match caller {
                PrincipalCaller::Cli { .. } => true,
                PrincipalCaller::RoomAgent(room) => {
                    bridge_is_room_agent(bridge, room, &execution.parent_discussion_id)
                }
            }
    })
}

/// KT-1962 — when an execution's worker is a CLI session of the principal's
/// own room, a grant that runs in that room may be the worker's: the session a
/// token names as the principal's must then be one that very launch joined or
/// resumed. Every other shape is told apart by the rooms alone.
fn principal_session_is_the_grants(
    conn: &rusqlite::Connection,
    bridge: Option<&crate::core::bridge_token::BridgeCaller>,
    caller: &PrincipalCaller,
    execution: &TaskExecution,
) -> Result<bool> {
    let (Some(bridge), PrincipalCaller::Cli { agent, session_id }) = (bridge, caller) else {
        return Ok(true);
    };
    if !worker_shares_the_principal_room(conn, execution)? {
        return Ok(true);
    }
    // The launch that holds the worker's session is the worker, whatever
    // other session it joined and names.
    if grant_holds_the_worker_session(bridge, execution) {
        return Ok(false);
    }
    // The worker's own session lives in that room too: holding it proves the
    // worker, never the principal.
    Ok(
        crate::db::discussion_sessions::find_active_session(conn, agent, session_id)?.is_some_and(
            |session| {
                Some(session.id) != execution.worker_cli_session_id
                    && bridge.own_sessions.contains(&session.id)
            },
        ),
    )
}

fn grant_holds_the_worker_session(
    bridge: &crate::core::bridge_token::BridgeCaller,
    execution: &TaskExecution,
) -> bool {
    execution
        .worker_cli_session_id
        .is_some_and(|worker| bridge.own_sessions.contains(&worker))
}

fn worker_shares_the_principal_room(
    conn: &rusqlite::Connection,
    execution: &TaskExecution,
) -> Result<bool> {
    let Some(worker) = execution.worker_cli_session_id else {
        return Ok(false);
    };
    let room: Option<String> = conn
        .query_row(
            "SELECT disc_id FROM discussion_sessions WHERE id = ?1",
            [worker],
            |row| row.get(0),
        )
        .optional()?;
    Ok(room.as_deref() == Some(execution.parent_discussion_id.as_str()))
}

/// Whether a grant is the one of `room_id`'s own agent as `room` declares it:
/// the launch is that room's agent, and of the provider the turn names (two
/// agents may run in one room; the provider comes from the launch, never from
/// the body).
fn bridge_is_room_agent(
    bridge: &crate::core::bridge_token::BridgeCaller,
    room: &SpawnedAgentCaller,
    room_id: &str,
) -> bool {
    bridge.room_agent_of.iter().any(|own| own == room_id)
        && bridge.agent.as_ref().is_some_and(|launched| {
            crate::db::orchestration::agent_type_from_db(room.agent_type.trim())
                .is_ok_and(|declared| declared == launched.agent_type)
        })
}

/// KT-1962 — the same before an execution exists (prepare, launch): the grant
/// runs in the room the caller names as the principal's.
fn bridge_holds_room_principal(
    bridge: Option<&crate::core::bridge_token::BridgeCaller>,
    caller: &PrincipalCaller,
    parent_discussion_id: &str,
) -> bool {
    bridge.is_none_or(|bridge| {
        let room = parent_discussion_id.to_string();
        bridge.own_discussions.contains(&room)
            && match caller {
                PrincipalCaller::Cli { .. } => true,
                PrincipalCaller::RoomAgent(agent) => bridge_is_room_agent(bridge, agent, &room),
            }
    })
}

/// KT-1962 — whether a bridge-token caller may act as the worker of
/// `execution` by the session it names: a grant minted for an execution works
/// for that execution only, and any other grant runs in the room that session
/// is joined to (a CLI of the principal's room that accepted an offer keeps
/// working from there, KT-837).
fn bridge_may_be_execution_worker(
    bridge: Option<&crate::core::bridge_token::BridgeCaller>,
    execution: &TaskExecution,
    session_room: &str,
) -> bool {
    bridge.is_none_or(|bridge| match bridge.own_execution.as_deref() {
        Some(own) => own == execution.id,
        None => bridge.own_discussions.iter().any(|own| own == session_room),
    })
}

/// KT-1962 — whether a bridge-token caller may read `execution` as one of its
/// parties: it runs in one of its two rooms or is its worker.
fn bridge_is_execution_party(
    bridge: Option<&crate::core::bridge_token::BridgeCaller>,
    execution: &TaskExecution,
) -> bool {
    bridge.is_none_or(|bridge| {
        let owns = |room: &str| bridge.own_discussions.iter().any(|own| own == room);
        bridge.own_execution.as_deref() == Some(execution.id.as_str())
            || owns(&execution.parent_discussion_id)
            || execution.sub_discussion_id.as_deref().is_some_and(owns)
    })
}

pub async fn task_exec_reassign_as(
    state: AppState,
    exec_id: String,
    request: TaskExecReassignRequest,
    bridge: Option<&crate::core::bridge_token::BridgeCaller>,
) -> Json<ApiResponse<serde_json::Value>> {
    let caller = match principal_caller(
        &request.source_agent,
        &request.source_session_id,
        request.room_agent,
    ) {
        Ok(caller) => caller,
        Err(message) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, message)),
    };
    // The grant is judged below, so the review-round refusal keeps its own
    // wording for a principal whose token adopted the child room.
    let Some(execution) = authorize_execution_principal(&state.db, &exec_id, &caller, None).await
    else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "execution not found or caller is not its principal",
        ));
    };
    let principal_grant = bridge_holds_execution_principal(bridge, &caller, &execution) && {
        let (grant, declared, execution) = (bridge.cloned(), caller.clone(), execution.clone());
        state
            .db
            .with_conn(move |conn| {
                principal_session_is_the_grants(conn, grant.as_ref(), &declared, &execution)
            })
            .await
            .unwrap_or(false)
    };
    let exec_id = execution.id;
    let amendment = match execution_amendment(
        request.worker,
        request.validations,
        request.extend_review_rounds.as_ref(),
    ) {
        Ok(amendment) => amendment,
        Err(refusal) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, refusal)),
    };
    if !principal_grant && !matches!(amendment, ExecutionAmendment::ExtendReviewRounds(_)) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "execution not found or caller is not its principal",
        ));
    }
    match amendment {
        ExecutionAmendment::Reassign(target) => {
            pin_principal_caller(&state.db, &exec_id, &caller).await;
            let Json(response) = reassign_execution(
                State(state),
                Path(exec_id),
                Json(ReassignExecutionRequest {
                    worker: crate::models::CampaignWorkerSelection {
                        target,
                        model: None,
                        profile_id: None,
                    },
                    reason: request.reason,
                }),
            )
            .await;
            Json(ApiResponse {
                success: response.success,
                data: response
                    .data
                    .and_then(|view| serde_json::to_value(view).ok()),
                error: response.error,
                error_code: response.error_code,
                notice: None,
                readiness: None,
            })
        }
        ExecutionAmendment::ExtendReviewRounds(extension) => {
            // The body's `source_agent`/`source_session_id` are declared by the
            // caller, so a worker could name its principal in them: with a bridge
            // token, the grant itself must be the principal's (KT-1345).
            if !principal_grant {
                return Json(ApiResponse::err_coded(
                    ApiErrorCode::Validation,
                    "only the execution's principal or a human can extend its review-round \
                     limit: a worker cannot raise its own, and a token that also owns the \
                     child room (e.g. after adopting it) counts as the worker's",
                ));
            }
            pin_principal_caller(&state.db, &exec_id, &caller).await;
            match extend_execution_review_rounds(
                &state,
                &exec_id,
                extension,
                &request.reason,
                principal_actor(&caller),
            )
            .await
            {
                Ok(value) => Json(ApiResponse::ok(value)),
                Err((code, message)) => Json(ApiResponse::err_coded(code, message)),
            }
        }
        ExecutionAmendment::ReplaceValidations(validations) => match replace_execution_validations(
            &state.db,
            &exec_id,
            validations,
            &request.reason,
            principal_actor(&caller),
        )
        .await
        {
            Ok(replacement) => match serde_json::to_value(replacement) {
                Ok(value) => Json(ApiResponse::ok(value)),
                Err(error) => Json(ApiResponse::err(error.to_string())),
            },
            Err((code, message)) => Json(ApiResponse::err_coded(code, message)),
        },
    }
}

/// Replace the validations of an existing execution (KT-839), for both principal
/// channels. The set is held to the same rules as at launch — a fix that could
/// never run would only move the failure back to integration — and the swap is
/// journaled with its actor and reason, so nothing is relaunched and nothing
/// about the earlier gates is lost.
pub async fn replace_execution_validations(
    db: &Database,
    exec_id: &str,
    validations: Vec<crate::models::ValidationSpec>,
    reason: &str,
    actor: OrchestrationActor,
) -> Result<crate::db::orchestration::ValidationsReplacement, (ApiErrorCode, String)> {
    let reason = reason.trim().to_string();
    if reason.is_empty() {
        return Err((
            ApiErrorCode::Validation,
            "reason is required: it is journaled with the swap".into(),
        ));
    }
    if let Err(detail) = validate_new_validation_specs(&validations) {
        return Err((
            ApiErrorCode::Validation,
            format!("invalid validations: {detail}"),
        ));
    }
    let id = exec_id.to_string();
    db.with_conn(move |conn| {
        crate::db::orchestration::replace_execution_validations(
            conn,
            &id,
            &validations,
            &actor,
            &reason,
        )
    })
    .await
    .map_err(|error| (ApiErrorCode::Conflict, error.to_string()))
}

/// Raise the review-round limit of an existing execution (KT-1345), for every
/// principal channel (MCP bridge, native tool, browser). An execution the limit
/// itself escalated is also given its findings again and re-offered to its
/// current worker: raising the number alone would leave it parked. Nothing is
/// relaunched, the room, worktree and earlier rounds stay.
pub async fn extend_execution_review_rounds(
    state: &AppState,
    exec_id: &str,
    extension: crate::db::orchestration::ReviewRoundsExtension,
    reason: &str,
    actor: OrchestrationActor,
) -> Result<serde_json::Value, (ApiErrorCode, String)> {
    let reason = reason.trim().to_string();
    if reason.is_empty() {
        return Err((
            ApiErrorCode::Validation,
            "reason is required: it is journaled with the new limit".into(),
        ));
    }
    let id = exec_id.to_string();
    let journal_reason = reason.clone();
    let extended = state
        .db
        .with_conn(move |conn| {
            crate::db::orchestration::extend_review_rounds(
                conn,
                &id,
                extension,
                &actor,
                &journal_reason,
            )
        })
        .await
        .map_err(|error| (ApiErrorCode::Conflict, error.to_string()))?;
    let mut reoffered = false;
    // KT-1345 — the token budget outranks the limit: a spent budget raises the number but
    // schedules no further attempt.
    if extended.escalated_by_review_budget && extended.token_budget_stop.is_none() {
        reoffer_after_review_budget_extension(state, exec_id, &reason)
            .await
            .map_err(|error| {
                (
                    ApiErrorCode::Conflict,
                    format!(
                        "the limit is now {} but the execution could not be re-offered \
                         (task_exec_reassign with its worker does it): {error}",
                        crate::models::review_limit_label(extended.max_review_rounds)
                    ),
                )
            })?;
        reoffered = true;
    }
    Ok(serde_json::json!({
        "task_execution_id": exec_id,
        "previous_limit": crate::models::review_limit_json(extended.previous_limit),
        "max_review_rounds": crate::models::review_limit_json(extended.max_review_rounds),
        "review_rounds": extended.review_rounds,
        "reoffered_to_worker": reoffered,
        "stop_reason": extended
            .token_budget_stop
            .map(|_| crate::db::orchestration::TOKEN_BUDGET_EXHAUSTED),
        "token_budget": extended.token_budget_stop.map(|stop| stop.json()),
    }))
}

/// The findings the budget-exhausted `request_changes` never delivered go to
/// the worker in its child room, then the execution takes the ordinary
/// re-offer path of a reassignment to the SAME worker.
async fn reoffer_after_review_budget_extension(
    state: &AppState,
    exec_id: &str,
    reason: &str,
) -> Result<()> {
    let id = exec_id.to_string();
    let execution = state
        .db
        .with_conn(move |conn| {
            crate::db::orchestration::get_task_execution(conn, &id)?
                .context("execution vanished before its re-offer")
        })
        .await?;
    let lang = room_lang(&state.db, &execution.parent_discussion_id).await;
    let child = execution
        .sub_discussion_id
        .clone()
        .context("execution has no child discussion for its findings")?;
    let id = exec_id.to_string();
    let attempt = execution.attempt_no;
    let review = state
        .db
        .with_conn(move |conn| crate::db::worker_reviews::get_review(conn, &id, attempt))
        .await?;
    if let Some(review) = review {
        if let Ok(decision) = serde_json::from_str::<ReviewDecisionV1>(&review.decision_json) {
            let message =
                build_review_findings_message(&execution.id, attempt, &decision, &child, lang);
            let child_id = child.clone();
            // A message already posted for this attempt is the same findings.
            let _ = state
                .db
                .with_conn(move |conn| {
                    crate::db::discussions::insert_message(conn, &child_id, &message)
                })
                .await;
        }
    }
    let target = worker_target_from_execution(&execution)?;
    let Json(response) = reassign_execution(
        State(state.clone()),
        Path(exec_id.to_string()),
        Json(ReassignExecutionRequest {
            worker: crate::models::CampaignWorkerSelection {
                target,
                model: execution.worker_model.clone(),
                profile_id: execution.worker_profile_id.clone(),
            },
            reason: format!("review-round limit raised: {reason}"),
        }),
    )
    .await;
    if response.success {
        Ok(())
    } else {
        bail!(
            "{}",
            response
                .error
                .unwrap_or_else(|| "the re-offer was refused".into())
        )
    }
}

/// Map a launch-saga refusal/failure to a stable `(code, message)`. `ProvisionError` has
/// no `Display` on purpose (each variant carries structured fields), so both handlers
/// share this. A mid-saga workspace failure is `Conflict` (resumable), never a bare 500 —
/// DoD-6's "no silent orphan" surfaced to the caller.
pub fn provision_error_parts(error: &ProvisionError) -> (ApiErrorCode, String) {
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
        ProvisionError::DiskPressure(reason) => (
            ApiErrorCode::Conflict,
            format!("launch refused, disk pressure (resumable): {reason}"),
        ),
    }
}

pub fn provision_error_to_response(error: ProvisionError) -> ApiResponse<TaskExecution> {
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
    let max_review_rounds = match review_rounds_argument(request.max_review_rounds.as_ref()) {
        Ok(rounds) => rounds,
        Err(detail) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, detail)),
    };
    match provision_single_task_execution_with_options(
        &state,
        input,
        request.worker_scope,
        request.validations,
        SingleTaskLaunchOptions {
            max_review_rounds,
            quota_fallback_agents: request.quota_fallback_agents,
        },
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

/// The accepted execution and its direct worker instructions. The child id remains the
/// durable task/evidence room; it is not a request to transfer the caller's room binding.
#[derive(Serialize)]
pub struct AcceptOfferResponse {
    pub child_discussion_id: String,
    pub execution: TaskExecution,
    pub worker_instructions: String,
}

/// Map the accept outcome to an HTTP response. Pure, so the anti-oracle fusion is unit
/// tested without an `AppState`: an unknown offer and a wrong caller collapse into ONE
/// opaque refusal, so a non-target session cannot enumerate live offer ids or probe
/// which session an offer targets. The other refusals are only reachable AFTER the
/// caller == target check, so naming the real state there is not an oracle.
pub fn accept_outcome_to_response(
    outcome: AcceptAttachOutcome,
) -> ApiResponse<AcceptOfferResponse> {
    match outcome {
        AcceptAttachOutcome::Attached {
            child_discussion_id,
            execution,
            worker_instructions,
        } => ApiResponse::ok(AcceptOfferResponse {
            child_discussion_id,
            execution,
            worker_instructions,
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
/// control offer. Identity is derived server-side from the bridge-supplied live pair and
/// reload-stable binding, but neither live membership nor durable room ownership moves;
/// the execution's pinned session id carries worker authority. The model passes only
/// `offer_id`.
pub async fn accept_offer(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
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
    // KT-1962 — a session id is readable in its room: a bridge token accepts
    // only for a session that runs in one of the grant's own rooms.
    if let Some(axum::Extension(bridge)) = &bridge {
        let (agent, session) = (source_agent.clone(), source_session_id.clone());
        let room = state
            .db
            .with_conn(move |conn| {
                Ok(
                    crate::db::discussion_sessions::find_active_session(conn, &agent, &session)?
                        .map(|session| session.disc_id),
                )
            })
            .await
            .ok()
            .flatten();
        if !room.is_some_and(|room| bridge.own_discussions.contains(&room)) {
            return Json(accept_outcome_to_response(
                AcceptAttachOutcome::WrongAcceptor,
            ));
        }
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
#[derive(Debug, Clone, Deserialize)]
pub struct SpawnedAgentCaller {
    pub discussion_id: String,
    pub agent_type: String,
    pub dispatch_job_id: String,
    pub source_message_id: String,
}

fn spawned_native_caller(
    caller: &SpawnedAgentCaller,
) -> Result<(AgentType, &str, &str, &str), &'static str> {
    let discussion_id = caller.discussion_id.trim();
    let dispatch_job_id = caller.dispatch_job_id.trim();
    let source_message_id = caller.source_message_id.trim();
    let agent_type = crate::db::orchestration::agent_type_from_db(caller.agent_type.trim())
        .map_err(|_| "spawned agent context is incomplete")?;
    if discussion_id.is_empty() || dispatch_job_id.is_empty() || source_message_id.is_empty() {
        return Err("spawned agent context is incomplete");
    }
    Ok((
        agent_type,
        discussion_id,
        dispatch_job_id,
        source_message_id,
    ))
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
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Json(request): Json<SpawnedWorkerCommitRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let bridge = bridge.as_ref().map(|extension| &extension.0);
    let exec_id = request.task_execution_id.trim().to_string();
    if exec_id.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "task_execution_id is required".to_string(),
        ));
    }
    let (agent_type, discussion_id, dispatch_job_id, source_message_id) =
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
    let execution = match spawned_native_worker_execution_for_caller(
        &state.db,
        &exec_id,
        dispatch_job_id,
        NativeExecutionCaller {
            discussion_id,
            agent_type: &agent_type,
            source_message_id: Some(source_message_id),
            alias: &alias,
            actor_session_id: Some(source_message_id),
        },
        bridge,
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
    let lease_execution_id = execution.id.clone();
    let lease_dispatch_job_id = dispatch_job_id.to_string();
    let lease = state
        .db
        .with_conn(move |conn| {
            crate::db::orchestration::acquire_spawned_commit_lease(
                conn,
                &lease_execution_id,
                &lease_dispatch_job_id,
            )
        })
        .await;
    let lease_id = match lease {
        Ok(crate::db::orchestration::SpawnedCommitLeaseOutcome::Acquired { lease_id }) => lease_id,
        Ok(crate::db::orchestration::SpawnedCommitLeaseOutcome::AlreadySettled) => {
            return Json(ApiResponse::ok(serde_json::json!({
                "already_committed": true,
                "message": "the current worker commit already completed; continue with delivery"
            })));
        }
        Ok(crate::db::orchestration::SpawnedCommitLeaseOutcome::NotCurrent) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "execution not found or not addressed to this worker".to_string(),
            ))
        }
        Ok(crate::db::orchestration::SpawnedCommitLeaseOutcome::Busy) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                "a commit is already in progress for this worker".to_string(),
            ))
        }
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                format!("unable to reserve worker commit authority: {error}"),
            ))
        }
    };
    let files = request.files;
    let message = request.message;
    let data_dir_lock = state.data_dir_lock.clone();
    // The pinned base the doc citations are compared with (KT-1721).
    let base_sha = execution.base_sha.clone();
    // Which hooks Kronn may run for this commit (KT-1745): the project's policy.
    let hooks = worker_commit_hooks_of(&state.db, Some(&workspace.project_id)).await;
    // For the notice a project without a commit identity gets (KT-1750).
    let identity_notice_for = (
        execution.id.clone(),
        discussion_id.to_string(),
        execution.parent_discussion_id.clone(),
        root.clone(),
    );
    let supervisor = spawn_spawned_commit_supervisor(
        state.db.clone(),
        execution.id,
        dispatch_job_id.to_string(),
        lease_id,
        move || {
            crate::api::agent_workspace_tools::git_commit_payload_reanchoring_docs_with_hooks(
                &root,
                &files,
                &message,
                data_dir_lock.as_deref(),
                base_sha.as_deref(),
                hooks,
            )
        },
    );
    let time_limit = crate::api::worker_time_limit::worker_time_limit_notice(
        &state,
        dispatch_job_id,
        &agent_type,
    )
    .await;
    match supervisor.await {
        Ok((committed, release_error)) => {
            if matches!(committed, Ok(Ok(_))) {
                let (execution_id, worker_room, principal_room, root) = identity_notice_for;
                let policy = tokio::task::spawn_blocking(move || {
                    crate::core::commit_policy::CommitPolicy::resolve(&root)
                })
                .await;
                if let Some(policy) = policy
                    .ok()
                    .filter(crate::core::commit_policy::CommitPolicy::uses_global_identity)
                {
                    announce_global_commit_identity(
                        &state.db,
                        &execution_id,
                        &worker_room,
                        &principal_room,
                        &policy.identity.email,
                    )
                    .await;
                }
            }
            spawned_commit_response(committed, release_error, time_limit)
        }
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("worker commit supervisor failed: {error}"),
        )),
    }
}

pub async fn retry_spawned_commit_lease_release<F, Fut>(mut release: F) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<bool>>,
{
    let delays = [0, 50, 250, 1_000];
    let mut last_error = None;
    for delay_ms in delays {
        if delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }
        match release().await {
            Ok(true) => return Ok(()),
            Ok(false) => {
                return Err("spawned commit lease token no longer owns the execution".to_string())
            }
            Err(error) => last_error = Some(error.to_string()),
        }
    }
    Err(last_error.unwrap_or_else(|| "spawned commit lease release failed".to_string()))
}

type SpawnedCommitTaskResult = Result<Result<serde_json::Value, String>, tokio::task::JoinError>;

pub fn spawn_spawned_commit_supervisor<F>(
    db: std::sync::Arc<Database>,
    execution_id: String,
    dispatch_job_id: String,
    lease_id: String,
    git_operation: F,
) -> tokio::task::JoinHandle<(SpawnedCommitTaskResult, Option<String>)>
where
    F: FnOnce() -> Result<serde_json::Value, String> + Send + 'static,
{
    tokio::spawn(async move {
        let committed = tokio::task::spawn_blocking(git_operation).await;
        if matches!(committed, Ok(Ok(_))) {
            let settle_db = db.clone();
            let settle_execution_id = execution_id.clone();
            let settle_dispatch_job_id = dispatch_job_id.clone();
            let settle_lease_id = lease_id.clone();
            if let Err(error) = retry_spawned_commit_lease_release(|| {
                let db = settle_db.clone();
                let execution_id = settle_execution_id.clone();
                let dispatch_job_id = settle_dispatch_job_id.clone();
                let lease_id = settle_lease_id.clone();
                async move {
                    db.with_conn(move |conn| {
                        crate::db::orchestration::settle_spawned_commit_lease(
                            conn,
                            &execution_id,
                            &dispatch_job_id,
                            &lease_id,
                        )
                    })
                    .await
                }
            })
            .await
            {
                tracing::warn!("unable to mark successful worker commit lease settled: {error}");
            }
        }
        let release_error = retry_spawned_commit_lease_release(|| {
            let db = db.clone();
            let execution_id = execution_id.clone();
            let dispatch_job_id = dispatch_job_id.clone();
            let lease_id = lease_id.clone();
            async move {
                db.with_conn(move |conn| {
                    crate::db::orchestration::release_spawned_commit_lease(
                        conn,
                        &execution_id,
                        &dispatch_job_id,
                        &lease_id,
                    )
                })
                .await
            }
        })
        .await
        .err();
        (committed, release_error)
    })
}

pub fn spawned_commit_response(
    committed: Result<Result<serde_json::Value, String>, tokio::task::JoinError>,
    release_error: Option<String>,
    time_limit: Option<serde_json::Value>,
) -> Json<ApiResponse<serde_json::Value>> {
    if let Some(error) = release_error {
        tracing::warn!(
            "unable to release worker commit authority: {error}; a settled lease can be reclaimed by reassignment or backend restart recovery"
        );
    }
    match committed {
        Ok(Ok(mut payload)) => {
            if let (Some(time_limit), Some(object)) = (time_limit, payload.as_object_mut()) {
                object.insert("time_limit".into(), time_limit);
            }
            Json(ApiResponse::ok(payload))
        }
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
pub fn deliver_outcome_to_response(outcome: DeliverOutcome) -> ApiResponse<DeliverResponse> {
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
            match status {
                // KT-1540 — say what to do instead of leaving the worker to retry.
                TaskExecutionStatus::Interrupted => format!(
                    "execution not deliverable ({}): do not continue the work, its principal \
                     must resume it (task_exec_resume) first",
                    status.as_str()
                ),
                _ => format!("execution not deliverable ({})", status.as_str()),
            },
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
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Json(request): Json<DeliverRequest>,
) -> Json<ApiResponse<DeliverResponse>> {
    let bridge = bridge.as_ref().map(|extension| &extension.0);
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
        let (agent_type, discussion_id, dispatch_job_id, source_message_id) =
            match spawned_native_caller(&caller) {
                Ok(caller) => caller,
                Err(_) => {
                    return Json(ApiResponse::err_coded(
                        ApiErrorCode::Validation,
                        "spawned agent delivery context is incomplete".to_string(),
                    ))
                }
            };
        let alias = crate::db::orchestration::agent_type_to_db(&agent_type);
        let execution = match spawned_native_worker_execution_for_caller(
            &state.db,
            &exec_id,
            dispatch_job_id,
            NativeExecutionCaller {
                discussion_id,
                agent_type: &agent_type,
                source_message_id: Some(source_message_id),
                alias: &alias,
                actor_session_id: Some(source_message_id),
            },
            bridge,
        )
        .await
        {
            Ok(execution) => execution,
            Err(error) => {
                let (code, message) = provision_error_parts(&error);
                return Json(ApiResponse::err_coded(code, message));
            }
        };
        match execution {
            Some(execution) => {
                deliver_authorized_worker_manifest(
                    &state.db,
                    execution,
                    &alias,
                    Some(source_message_id.to_string()),
                    crate::db::orchestration::DeliveryDispatchExpectation::Exact(dispatch_job_id),
                    &manifest_json,
                    DeliveryManifestAuthorship::NativeProjection,
                )
                .await
            }
            None => Ok(DeliverOutcome::NotAddressed),
        }
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
        deliver_worker_manifest_as(
            &state.db,
            &exec_id,
            &source_agent,
            &source_session_id,
            &manifest_json,
            bridge,
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
    let manifest: DeliveryManifestV1 = serde_json::from_str(manifest_json)
        .context("DeliveryManifest v1 : structure JSON non conforme au contrat typé")?;
    validate_tests_executed(&manifest)?;
    Ok(manifest)
}

/// KT-1246 — `tests_executed` must agree with the reported `tests`: a claim of
/// "executed" names what ran and is backed by a pass/fail entry, and "not
/// executed" names nothing. An absent field is accepted (legacy and joined-CLI
/// deliveries); the spawned-worker projection refuses it earlier.
fn validate_tests_executed(manifest: &DeliveryManifestV1) -> Result<()> {
    let Some(declared) = manifest.tests_executed.as_ref() else {
        return Ok(());
    };
    let ran = manifest.tests.iter().any(|test| {
        matches!(
            test.status,
            crate::models::TestStatus::Pass | crate::models::TestStatus::Fail
        )
    });
    if declared.executed {
        if declared
            .which
            .iter()
            .all(|command| command.trim().is_empty())
        {
            bail!("DeliveryManifest v1 : `tests_executed.executed` est vrai mais `which` ne nomme aucune commande");
        }
        if !ran {
            bail!("DeliveryManifest v1 : `tests_executed.executed` est vrai mais aucun test n'est rapporté `pass` ou `fail`");
        }
    } else {
        if !declared.which.is_empty() {
            bail!("DeliveryManifest v1 : `tests_executed.executed` est faux mais `which` n'est pas vide");
        }
        if ran {
            bail!("DeliveryManifest v1 : `tests_executed.executed` est faux mais des tests sont rapportés `pass` ou `fail`");
        }
    }
    Ok(())
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
    /// The room's own native agent, injected by the bridge from the runner's
    /// environment. Exclusive with `source_agent`/`source_session_id`; it is the
    /// same principal identity `task_exec_launch` accepts, so the agent that
    /// launched an execution can also follow, decide and stop it.
    #[serde(default)]
    pub room_agent: Option<SpawnedAgentCaller>,
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
        ApproveBlockReason::ForeignIdentityTrailers(detail) => detail.clone(),
    }
}

/// Map the review outcome to an HTTP response. Pure, so the anti-oracle fusion is unit tested
/// without an `AppState`: an unknown execution and a caller who is neither worker nor principal
/// collapse into ONE opaque refusal. `SelfReviewForbidden` / `NotReviewable` / `ApproveBlocked`
/// / `InvalidDecision` are reachable only AFTER the caller is established as a party, so naming
/// the real reason there is not an oracle.
pub fn review_outcome_to_response(outcome: ReviewOutcome) -> ApiResponse<ReviewResponse> {
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
        ReviewOutcome::StaleScope(detail) => ApiResponse::err_coded(ApiErrorCode::Conflict, detail),
    }
}

/// `POST /api/orchestration/review` — the principal decides a delivered attempt (approve |
/// request_changes). Identity is derived server-side from the bridge-supplied durable pair
/// or, for the room's native agent, from its runner-injected `room_agent` context; the
/// caller supplies only `task_execution_id` + `decision`.
pub async fn review(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Json(request): Json<ReviewRequest>,
) -> Json<ApiResponse<ReviewResponse>> {
    let bridge = bridge.as_ref().map(|extension| &extension.0);
    let clean = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    if let Some(room) = request.room_agent {
        if clean(&request.source_agent).is_some() || clean(&request.source_session_id).is_some() {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                "choose exactly one principal identity mode".to_string(),
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
        return match decide_room_agent_review(
            &state.db,
            request.task_execution_id.trim(),
            &decision_json,
            room,
            bridge,
        )
        .await
        {
            Ok(outcome) => Json(review_outcome_to_response(enqueue_approved_integration(
                &state, outcome,
            ))),
            Err(error) => {
                let (code, message) = provision_error_parts(&error);
                Json(ApiResponse::err_coded(code, message))
            }
        };
    }
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
    // Before the decision, so an escalation it raises addresses this reviewer;
    // never for a grant that cannot be the principal's.
    let may_pin = match bridge {
        None => true,
        Some(bridge) => {
            let (id, grant) = (exec_id.clone(), bridge.clone());
            let declared = PrincipalCaller::Cli {
                agent: source_agent.clone(),
                session_id: source_session_id.clone(),
            };
            state
                .db
                .with_conn(move |conn| {
                    let Some(execution) = crate::db::orchestration::get_task_execution(conn, &id)?
                    else {
                        return Ok(false);
                    };
                    Ok(bridge_is_execution_principal(&grant, &execution)
                        && principal_session_is_the_grants(
                            conn,
                            Some(&grant),
                            &declared,
                            &execution,
                        )?)
                })
                .await
                .unwrap_or(false)
        }
    };
    if may_pin {
        pin_cli_principal(&state.db, &exec_id, &source_agent, &source_session_id).await;
    }
    match decide_review_as(
        &state.db,
        &exec_id,
        &decision_json,
        &source_agent,
        &source_session_id,
        bridge,
    )
    .await
    {
        Ok(outcome) => Json(review_outcome_to_response(enqueue_approved_integration(
            &state, outcome,
        ))),
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
        Ok(outcome) => Json(review_outcome_to_response(enqueue_approved_integration(
            &state, outcome,
        ))),
        Err(error) => {
            let (code, message) = provision_error_parts(&error);
            Json(ApiResponse::err_coded(code, message))
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
pub mod tests {
    use super::*;
    use crate::models::{
        AgentType, AiAuditStatus, AiConfigStatus, CreatePlanningDodItem, CreatePlanningTaskRequest,
        PlanningTaskStatus, Project, ValidationSpec,
    };
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn a_room_native_agent_is_principal_only_for_its_own_running_turn() {
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            let now = "2026-09-24T00:00:00Z";
            conn.execute(
                "INSERT INTO discussions (id, title, agent, created_at, updated_at)
                 VALUES ('d-parent', 'Parent', 'ClaudeCode', ?1, ?1),
                        ('d-other', 'Other', 'ClaudeCode', ?1, ?1),
                        ('d-work', 'Work', 'ClaudeCode', ?1, ?1)",
                [now],
            )?;
            for (id, disc) in [
                ("msg-1", "d-parent"),
                ("msg-2", "d-other"),
                ("msg-3", "d-parent"),
            ] {
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order)
                     VALUES (?1, ?2, 'User', 'go', ?3, (SELECT COUNT(*) FROM messages))",
                    rusqlite::params![id, disc, now],
                )?;
            }
            for (id, disc, trigger) in [
                ("job-1", "d-parent", "msg-1"),
                ("job-2", "d-other", "msg-2"),
                ("job-done", "d-parent", "msg-3"),
            ] {
                crate::db::agent_dispatch::enqueue(
                    conn,
                    crate::db::agent_dispatch::NewAgentDispatchJob {
                        id,
                        discussion_id: disc,
                        trigger_message_id: trigger,
                        trigger_sort_order: 1,
                        dedupe_key: id,
                        agent_override: None,
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: None,
                        group_concurrency_limit: None,
                    },
                )?;
            }
            conn.execute(
                "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order)
                 VALUES ('msg-w', 'd-work', 'User', 'work', ?1, 99)",
                [now],
            )?;
            crate::db::agent_dispatch::enqueue(
                conn,
                crate::db::agent_dispatch::NewAgentDispatchJob {
                    id: "job-worker",
                    discussion_id: "d-work",
                    trigger_message_id: "msg-w",
                    trigger_sort_order: 99,
                    dedupe_key: "job-worker",
                    agent_override: None,
                    chain_prompt_ids: &[],
                    batch_item: None,
                    group_id: None,
                    group_concurrency_limit: None,
                },
            )?;
            conn.execute(
                "INSERT INTO planning_tasks (id, task_number, title, created_at, updated_at)
                 VALUES ('t-worker', 1, 'Worker task', ?1, ?1)",
                [now],
            )?;
            let input = crate::models::LaunchSingleTaskInput::new("t-worker", "d-work");
            let actor = crate::models::OrchestrationActor {
                kind: crate::models::PlanningActorKind::Backend,
                id: Some("room-agent-test".into()),
                session_id: None,
                source_message_id: None,
                token_declared: false,
            };
            let execution =
                crate::db::orchestration::launch_single_task(conn, &input, &actor)?.execution;
            crate::db::orchestration::attach_execution_dispatch(conn, &execution.id, "job-worker")?;
            conn.execute(
                "UPDATE agent_dispatch_jobs SET status = 'Running'
                 WHERE id IN ('job-1', 'job-2', 'job-worker')",
                [],
            )?;
            conn.execute(
                "UPDATE agent_dispatch_jobs SET status = 'Completed' WHERE id = 'job-done'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let room = |disc: &str, agent: &str, job: &str, msg: &str| SpawnedAgentCaller {
            discussion_id: disc.into(),
            agent_type: agent.into(),
            dispatch_job_id: job.into(),
            source_message_id: msg.into(),
        };
        let cases = [
            (
                room("d-parent", "ClaudeCode", "job-1", "msg-1"),
                true,
                "own running turn",
            ),
            (
                room("d-other", "ClaudeCode", "job-2", "msg-2"),
                false,
                "native agent of another room",
            ),
            (
                room("d-parent", "ClaudeCode", "job-2", "msg-2"),
                false,
                "job of another room",
            ),
            (
                room("d-parent", "Codex", "job-1", "msg-1"),
                false,
                "another provider",
            ),
            (
                room("d-parent", "ClaudeCode", "job-1", "msg-3"),
                false,
                "another trigger",
            ),
            (
                room("d-parent", "ClaudeCode", "job-done", "msg-3"),
                false,
                "finished turn",
            ),
            (
                room("d-parent", "ClaudeCode", "missing", "msg-1"),
                false,
                "unknown job",
            ),
        ];
        for (caller, expected, label) in cases {
            let caller = PrincipalCaller::RoomAgent(caller);
            let authorized = db
                .with_conn(move |conn| principal_is_authorized(conn, "d-parent", &caller))
                .await
                .unwrap();
            assert_eq!(authorized, expected, "{label}");
        }
        // A running job that is a delegated worker's dispatch never makes its
        // agent the principal of the room it runs in.
        let worker =
            PrincipalCaller::RoomAgent(room("d-work", "ClaudeCode", "job-worker", "msg-w"));
        let authorized = db
            .with_conn(move |conn| principal_is_authorized(conn, "d-work", &worker))
            .await
            .unwrap();
        assert!(!authorized, "a delegated worker's dispatch");
        let stranger = PrincipalCaller::Cli {
            agent: "Codex".into(),
            session_id: "not-a-member".into(),
        };
        let authorized = db
            .with_conn(move |conn| principal_is_authorized(conn, "d-parent", &stranger))
            .await
            .unwrap();
        assert!(
            !authorized,
            "a CLI session that is not a member stays refused"
        );
    }

    /// A parent room `d-parent` whose Codex turn `job-1` is running and launched
    /// `t-1`'s execution (returned), plus every room-agent identity that must NOT
    /// be its principal: a running turn of another room (`job-2`), finished and
    /// cancelled turns of the parent room, and the dispatch of another
    /// execution's worker in the parent room (`job-worker`, still pending).
    async fn room_agent_lifecycle_fixture(db: &std::sync::Arc<Database>) -> String {
        db.with_conn(|conn| {
            let now = "2026-10-14T00:00:00Z";
            conn.execute(
                "INSERT INTO discussions (id, title, agent, created_at, updated_at)
                 VALUES ('d-parent', 'Parent', 'Codex', ?1, ?1),
                        ('d-other', 'Other', 'Codex', ?1, ?1)",
                [now],
            )?;
            for (id, disc, job, status) in [
                ("msg-1", "d-parent", "job-1", "Running"),
                ("msg-2", "d-other", "job-2", "Running"),
                ("msg-3", "d-parent", "job-done", "Completed"),
                ("msg-4", "d-parent", "job-cancelled", "Cancelled"),
                // A room runs one turn at a time: the worker's dispatch becomes
                // `Running` only in the case that presents it.
                ("msg-w", "d-parent", "job-worker", "Pending"),
            ] {
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order)
                     VALUES (?1, ?2, 'User', 'go', ?3, (SELECT COUNT(*) FROM messages))",
                    rusqlite::params![id, disc, now],
                )?;
                crate::db::agent_dispatch::enqueue(
                    conn,
                    crate::db::agent_dispatch::NewAgentDispatchJob {
                        id: job,
                        discussion_id: disc,
                        trigger_message_id: id,
                        trigger_sort_order: 1,
                        dedupe_key: job,
                        agent_override: None,
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: None,
                        group_concurrency_limit: None,
                    },
                )?;
                conn.execute(
                    "UPDATE agent_dispatch_jobs SET status = ?1 WHERE id = ?2",
                    rusqlite::params![status, job],
                )?;
            }
            // Messages written by the code under test (the terminal notice a
            // cancellation posts in the parent room) take their order from the
            // discussion's sequence: move it past the rows inserted above.
            conn.execute(
                "UPDATE discussions SET next_message_seq = (
                     SELECT COALESCE(MAX(sort_order), -1) + 1 FROM messages
                     WHERE discussion_id = discussions.id)",
                [],
            )?;
            conn.execute(
                "INSERT INTO planning_tasks (id, task_number, title, created_at, updated_at)
                 VALUES ('t-1', 1, 'CI task', ?1, ?1), ('t-2', 2, 'Other task', ?1, ?1)",
                [now],
            )?;
            let actor = crate::models::OrchestrationActor {
                kind: crate::models::PlanningActorKind::Backend,
                id: Some("room-agent-lifecycle-test".into()),
                session_id: None,
                source_message_id: None,
                token_declared: false,
            };
            let launched = crate::db::orchestration::launch_single_task(
                conn,
                &crate::models::LaunchSingleTaskInput::new("t-1", "d-parent"),
                &actor,
            )?
            .execution
            .id;
            let other = crate::db::orchestration::launch_single_task(
                conn,
                &crate::models::LaunchSingleTaskInput::new("t-2", "d-parent"),
                &actor,
            )?
            .execution
            .id;
            crate::db::orchestration::attach_execution_dispatch(conn, &other, "job-worker")?;
            Ok(launched)
        })
        .await
        .unwrap()
    }

    fn lifecycle_room(disc: &str, job: &str, msg: &str) -> SpawnedAgentCaller {
        SpawnedAgentCaller {
            discussion_id: disc.into(),
            agent_type: "Codex".into(),
            dispatch_job_id: job.into(),
            source_message_id: msg.into(),
        }
    }

    /// Regression: a room's native agent launched an execution as its principal,
    /// then every follow-up (status, review, cancel) was refused as "not
    /// addressed to this session" because those paths accepted only a CLI
    /// session. The launching identity must be able to see and decide it.
    #[tokio::test]
    async fn a_room_native_agent_can_review_and_stop_the_execution_it_launched() {
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let execution_id = room_agent_lifecycle_fixture(&db).await;
        let room = lifecycle_room;

        let own = PrincipalCaller::RoomAgent(room("d-parent", "job-1", "msg-1"));
        let found = authorize_execution_principal(&db, &execution_id, &own, None).await;
        assert_eq!(found.map(|e| e.id), Some(execution_id.clone()));
        for (caller, label) in [
            (
                room("d-other", "job-2", "msg-2"),
                "native agent of another room",
            ),
            (room("d-parent", "job-done", "msg-3"), "finished turn"),
        ] {
            let caller = PrincipalCaller::RoomAgent(caller);
            assert!(
                authorize_execution_principal(&db, &execution_id, &caller, None)
                    .await
                    .is_none(),
                "{label}"
            );
        }
        assert!(
            authorize_execution_principal(&db, "missing-exec", &own, None)
                .await
                .is_none(),
            "unknown execution"
        );

        let decision = r#"{"version":"1","task_ref":"KT-1","decision":"approve"}"#;
        // Authorized: the refusal is now about the execution's state, not the caller.
        let outcome = decide_room_agent_review(
            &db,
            &execution_id,
            decision,
            room("d-parent", "job-1", "msg-1"),
            None,
        )
        .await
        .unwrap();
        assert!(
            !matches!(outcome, ReviewOutcome::NotAddressed),
            "the launching room agent is addressed"
        );
        let outcome = decide_room_agent_review(
            &db,
            &execution_id,
            decision,
            room("d-other", "job-2", "msg-2"),
            None,
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, ReviewOutcome::NotAddressed),
            "a stranger stays opaque"
        );
    }

    /// `(endpoint, success, error, error_code)` of one principal lifecycle call.
    type LifecycleAnswer = (&'static str, bool, Option<String>, Option<String>);

    /// Call status, review, cancel, resume and reassign on `exec_id` with one
    /// room-agent identity, optionally ALSO naming a CLI session (`mixed`).
    async fn every_principal_call(
        state: &AppState,
        exec_id: &str,
        room: SpawnedAgentCaller,
        mixed: bool,
    ) -> Vec<LifecycleAnswer> {
        let (agent, session) = if mixed {
            ("Codex".to_string(), "cli-session".to_string())
        } else {
            (String::new(), String::new())
        };
        let caller = || TaskExecCallerRequest {
            source_agent: agent.clone(),
            source_session_id: session.clone(),
            spawned_agent: None,
            room_agent: Some(room.clone()),
        };
        fn answer<T: Serialize>(name: &'static str, r: ApiResponse<T>) -> LifecycleAnswer {
            (name, r.success, r.error, r.error_code)
        }
        let Json(status) = task_exec_status(
            State(state.clone()),
            Path(exec_id.to_string()),
            None,
            Json(TaskExecStatusRequest {
                caller: caller(),
                ..Default::default()
            }),
        )
        .await;
        let Json(review) = review(
            State(state.clone()),
            None,
            Json(ReviewRequest {
                task_execution_id: exec_id.to_string(),
                decision: serde_json::json!({
                    "version": "1", "task_ref": "KT-1", "decision": "approve"
                }),
                source_agent: mixed.then(|| agent.clone()),
                source_session_id: mixed.then(|| session.clone()),
                room_agent: Some(room.clone()),
            }),
        )
        .await;
        let Json(cancel) = task_exec_cancel(
            State(state.clone()),
            Path(exec_id.to_string()),
            None,
            Json(TaskExecCancelRequest {
                source_agent: agent.clone(),
                source_session_id: session.clone(),
                room_agent: Some(room.clone()),
                reason: "stop".into(),
                cleanup_policy: None,
            }),
        )
        .await;
        let Json(resume) = task_exec_resume(
            State(state.clone()),
            Path(exec_id.to_string()),
            None,
            Json(caller()),
        )
        .await;
        let Json(reassign) = task_exec_reassign(
            State(state.clone()),
            Path(exec_id.to_string()),
            Json(TaskExecReassignRequest {
                source_agent: agent.clone(),
                source_session_id: session.clone(),
                room_agent: Some(room.clone()),
                worker: None,
                validations: Some(Vec::new()),
                extend_review_rounds: None,
                reason: "retry".into(),
            }),
        )
        .await;
        vec![
            answer("status", status),
            answer("review", review),
            answer("cancel", cancel),
            answer("resume", resume),
            answer("reassign", reassign),
        ]
    }

    /// The execution row plus its event and review journals: a refused call
    /// must leave all three exactly as they were.
    async fn execution_state(db: &std::sync::Arc<Database>, exec_id: &str) -> serde_json::Value {
        let id = exec_id.to_string();
        db.with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &id)?;
            let count = |table: &str| -> anyhow::Result<i64> {
                Ok(conn.query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE task_execution_id = ?1"),
                    [&id],
                    |row| row.get(0),
                )?)
            };
            Ok(serde_json::json!({
                "execution": execution,
                "events": count("task_execution_events")?,
                "reviews": count("task_execution_reviews")?,
            }))
        })
        .await
        .unwrap()
    }

    #[test]
    fn a_principal_request_names_exactly_one_identity() {
        let room = || {
            Some(SpawnedAgentCaller {
                discussion_id: "d".into(),
                agent_type: "ClaudeCode".into(),
                dispatch_job_id: "j".into(),
                source_message_id: "m".into(),
            })
        };
        assert!(matches!(
            principal_caller("", "", room()),
            Ok(PrincipalCaller::RoomAgent(_))
        ));
        assert!(matches!(
            principal_caller("Codex", "s", None),
            Ok(PrincipalCaller::Cli { .. })
        ));
        assert_eq!(
            principal_caller("Codex", "s", room()).err(),
            Some("choose exactly one principal identity mode")
        );
        assert!(principal_caller("", "", None)
            .err()
            .is_some_and(
                |error| error.contains("source_agent") && error.contains("source_session_id")
            ));
    }

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
                version_checked_at: None,
                version_check_error: None,
                version_source_url: None,
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
                shadowed_installs: None,
                fallback_command: None,
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

    fn native_detection() -> crate::models::AgentDetection {
        crate::models::AgentDetection {
            name: "Agent".into(),
            agent_type: AgentType::ClaudeCode,
            installed: true,
            enabled: true,
            path: None,
            version: None,
            latest_version: None,
            version_checked_at: None,
            version_check_error: None,
            version_source_url: None,
            origin: "test".into(),
            install_command: None,
            host_managed: false,
            host_label: None,
            runtime_available: true,
            auth_ready: Some(true),
            auth_setup_command: None,
            rtk_available: false,
            rtk_hook_configured: false,
            runtime_warning: None,
            shadowed_installs: None,
            fallback_command: None,
        }
    }

    /// A Claude catalogue where `claude-fable-5[1m]` disappeared with no
    /// replacement, as the launch preflight refuses it (KT-860).
    async fn seed_refused_claude_alias(db: &Database) {
        let listed = |id: &str| crate::db::model_catalog::DiscoveredModel {
            model_id: id.into(),
            display_name: id.into(),
            resolved_model: None,
            description: None,
            capabilities: vec!["chat".into()],
            reasoning_modes: vec![],
            default_reasoning_mode: None,
        };
        db.with_conn(move |conn| {
            let target = crate::db::model_catalog::agent_runtime_target_id(&AgentType::ClaudeCode);
            crate::db::model_catalog::reconcile_live(
                conn,
                &target,
                &AgentType::ClaudeCode,
                &[listed("claude-fable-5[1m]"), listed("sonnet")],
            )?;
            crate::db::model_catalog::reconcile_live(
                conn,
                &target,
                &AgentType::ClaudeCode,
                &[listed("sonnet")],
            )
        })
        .await
        .unwrap();
    }

    /// KT-860 — `agent_list` reads the preflight's verdict: a tier it would
    /// refuse is listed with the reason, and a refused default tier makes the
    /// worker unavailable.
    #[tokio::test]
    async fn kt860_agent_list_never_lists_a_tier_the_preflight_refuses_as_available() {
        let db = Database::open_in_memory().unwrap();
        seed_refused_claude_alias(&db).await;
        let mut config = crate::core::config::default_config();
        config.agents.model_tiers.claude_code.reasoning = Some("claude-fable-5[1m]".into());
        config.agents.model_tiers.claude_code.default = Some("sonnet".into());
        let detection = native_detection();
        let claude =
            |config: &crate::models::AppConfig,
             verdicts: &[(AgentType, crate::models::CatalogTierVerdict)]| {
                build_task_worker_catalogue(
                    config,
                    std::slice::from_ref(&detection),
                    &[],
                    &[],
                    &WorkerPreflight {
                        tier_verdicts: verdicts,
                        ..Default::default()
                    },
                )
                .workers
                .into_iter()
                .find(|entry| entry.worker.agent_type == AgentType::ClaudeCode)
                .unwrap()
            };

        let verdicts = bounded_tier_verdicts(&db, &config.agents.model_tiers).await;
        let entry = claude(&config, &verdicts);
        let reasoning = entry
            .tiers
            .iter()
            .find(|tier| tier.tier == ModelTier::Reasoning)
            .unwrap();
        let refusal = reasoning
            .refusal
            .as_ref()
            .expect("the preflight refuses it");
        assert_eq!(refusal.code, "model_unavailable");
        assert!(
            refusal.detail.contains("`claude-fable-5[1m]`"),
            "{refusal:?}"
        );
        assert!(
            entry.available,
            "the default tier still launches: {entry:#?}"
        );

        // A replacement the preflight would run is what the listing shows.
        config.agents.model_tiers.claude_code.default = Some("claude-fable-5[1m]".into());
        let verdicts = bounded_tier_verdicts(&db, &config.agents.model_tiers).await;
        let entry = claude(&config, &verdicts);
        let default = entry
            .tiers
            .iter()
            .find(|tier| tier.tier == ModelTier::Default)
            .unwrap();
        assert_eq!(default.resolved_model.as_deref(), Some("sonnet"));
        assert_eq!(
            default.requested_model.as_deref(),
            Some("claude-fable-5[1m]")
        );
        assert!(entry.available, "{entry:#?}");

        // Without one, the worker's own tier is refused, so is the worker.
        db.with_conn(|conn| {
            crate::db::model_catalog::reconcile_live(
                conn,
                &crate::db::model_catalog::agent_runtime_target_id(&AgentType::ClaudeCode),
                &AgentType::ClaudeCode,
                &[],
            )
        })
        .await
        .unwrap();
        let verdicts = bounded_tier_verdicts(&db, &config.agents.model_tiers).await;
        let entry = claude(&config, &verdicts);
        assert!(!entry.available, "{entry:#?}");
        assert!(entry
            .reasons
            .iter()
            .any(|reason| reason.code == "model_unavailable"));
    }

    /// KT-667 — a Codex tier pinned to a model its complete listing omits is
    /// listed with a warning and stays available: a listing is no access proof.
    #[tokio::test]
    async fn kt667_agent_list_warns_about_a_pin_the_cli_listing_omits() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            let target = crate::db::model_catalog::agent_runtime_target_id(&AgentType::Codex);
            crate::db::model_catalog::insert_migrated_seed(
                conn,
                &AgentType::Codex,
                "gpt-5.6-sol",
                "gpt-5.6-sol",
                Some(ModelTier::Default),
                &["chat".into()],
                &[],
            )?;
            crate::db::model_catalog::reconcile_listing(
                conn,
                &target,
                &AgentType::Codex,
                &[],
                crate::db::model_catalog::Refutation::Complete(["gpt-6-astra"].into()),
            )
        })
        .await
        .unwrap();
        let config = crate::core::config::default_config();
        let verdicts = bounded_tier_verdicts(&db, &config.agents.model_tiers).await;
        let mut detection = native_detection();
        detection.agent_type = AgentType::Codex;
        let entry = build_task_worker_catalogue(
            &config,
            std::slice::from_ref(&detection),
            &[],
            &[],
            &WorkerPreflight {
                tier_verdicts: &verdicts,
                ..Default::default()
            },
        )
        .workers
        .into_iter()
        .find(|entry| entry.worker.agent_type == AgentType::Codex)
        .unwrap();
        assert!(entry.available, "{entry:#?}");
        let tier = entry
            .tiers
            .iter()
            .find(|tier| tier.tier == ModelTier::Default)
            .unwrap();
        assert!(tier.refusal.is_none());
        assert_eq!(
            tier.warning.as_ref().map(|w| w.code.as_str()),
            Some("model_not_listed")
        );
        assert!(entry
            .warnings
            .iter()
            .any(|warning| warning.code == "model_not_listed"));
    }

    /// KT-1381 — the local ComfyUI shows up in agent_list with its media and
    /// the envelope its workflows accept, and only once a slot is set.
    #[test]
    fn the_local_comfyui_is_listed_with_its_media_once_a_slot_is_set() {
        let mut config = crate::core::config::default_config();
        assert!(with_local_media_source(Vec::new(), &config).is_empty());
        config.comfyui.image_model = Some("RealVisXL_V5.0_fp16.safetensors".into());
        let connections = with_local_media_source(Vec::new(), &config);
        assert_eq!(connections.len(), 1);
        let capabilities = vec![MediaCapabilityEntry {
            connection_id: crate::core::comfyui::CONNECTION_ID.into(),
            modality: crate::models::MediaModality::Image,
            model: "RealVisXL_V5.0_fp16.safetensors".into(),
            capabilities: crate::api::comfyui::capabilities(
                "RealVisXL_V5.0_fp16.safetensors",
                crate::models::MediaModality::Image,
            ),
        }];
        let catalogue = build_task_worker_catalogue(
            &config,
            &[],
            &[],
            &connections,
            &WorkerPreflight {
                media_capabilities: &capabilities,
                ..Default::default()
            },
        );
        let entry = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.connection_id.as_deref() == Some("local:comfyui"))
            .expect("the local ComfyUI entry");
        assert_eq!(entry.label, "ComfyUI (local)");
        assert_eq!(entry.media.len(), 1);
        assert_eq!(entry.media[0].model, "RealVisXL_V5.0_fp16.safetensors");
        let envelope = entry.media[0]
            .capabilities
            .as_ref()
            .expect("the local envelope");
        assert!(envelope.aspect_ratios.contains(&"16:9".to_string()));
        assert!(
            entry.tiers.iter().all(|tier| tier.resolved_model.is_none()),
            "no chat tier"
        );
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
                version_checked_at: None,
                version_check_error: None,
                version_source_url: None,
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
                shadowed_installs: None,
                fallback_command: None,
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
        let mut config = crate::core::config::default_config();
        config.agents.model_tiers.ollama.default = Some("operator-ollama".into());
        let catalogue = build_task_worker_catalogue(
            &config,
            &detections,
            &joined,
            &[],
            &WorkerPreflight {
                http_reachability: &[
                    (AgentType::Ollama, true),
                    (AgentType::LiteLlm, false),
                    (AgentType::Nvidia, false),
                ],
                ..Default::default()
            },
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
            tier.tier == ModelTier::Default
                && tier.resolved_model.as_deref() == Some("operator-ollama")
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

    fn media_connection(
        image: Option<&str>,
        video: Option<&str>,
    ) -> crate::models::ExternalApiConnection {
        crate::models::ExternalApiConnection {
            id: "conn-or".into(),
            display_name: "OpenRouter".into(),
            mention_alias: "openrouter".into(),
            endpoint: Some("https://openrouter.ai/api".into()),
            credential_slug: "openrouter".into(),
            origin_preset: crate::models::ExternalApiConnectionPreset::OpenRouter,
            economy_model: Some("qwen/qwen3.8-max".into()),
            default_model: None,
            reasoning_model: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            image_model: image.map(str::to_string),
            video_model: video.map(str::to_string),
            decision_model: None,
            tools_enabled: None,
            media_endpoint: None,
        }
    }

    fn catalogue_with(
        connections: &[crate::models::ExternalApiConnection],
    ) -> crate::models::TaskWorkerCatalogue {
        catalogue_with_capabilities(connections, &[])
    }

    fn catalogue_with_capabilities(
        connections: &[crate::models::ExternalApiConnection],
        media_capabilities: &[MediaCapabilityEntry],
    ) -> crate::models::TaskWorkerCatalogue {
        build_task_worker_catalogue(
            &crate::core::config::default_config(),
            &[],
            &[],
            connections,
            &WorkerPreflight {
                media_capabilities,
                ..Default::default()
            },
        )
    }

    fn observed(
        outcome: crate::core::endpoint_reachability::ProbeOutcome,
        age: chrono::Duration,
    ) -> Vec<(
        String,
        crate::core::endpoint_reachability::EndpointObservation,
    )> {
        vec![(
            "conn-or".to_string(),
            crate::core::endpoint_reachability::EndpointObservation {
                outcome,
                checked_at: chrono::Utc::now() - age,
            },
        )]
    }

    fn catalogue_observing(
        connections: &[crate::models::ExternalApiConnection],
        connection_reachability: &[(
            String,
            crate::core::endpoint_reachability::EndpointObservation,
        )],
        http_reachability: &[(AgentType, bool)],
    ) -> crate::models::TaskWorkerCatalogue {
        build_task_worker_catalogue(
            &crate::core::config::default_config(),
            &[],
            &[],
            connections,
            &WorkerPreflight {
                connection_reachability,
                http_reachability,
                ..Default::default()
            },
        )
    }

    fn connection_entry(
        catalogue: &crate::models::TaskWorkerCatalogue,
    ) -> &crate::models::TaskWorkerCatalogueEntry {
        catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.connection_id.as_deref() == Some("conn-or"))
            .expect("the configured connection must stay listed")
    }

    fn reason_codes(entry: &crate::models::TaskWorkerCatalogueEntry) -> Vec<&str> {
        entry.reasons.iter().map(|r| r.code.as_str()).collect()
    }

    /// KT-697: an address that does not resolve was reported reachable and
    /// available, while the legacy family on the same address said
    /// `endpoint_unreachable`. Both now say the same thing.
    #[test]
    fn a_named_connection_failing_dns_is_unreachable_like_the_legacy_family() {
        use crate::core::endpoint_reachability::{ProbeOutcome, UnreachableReason};
        let catalogue = catalogue_observing(
            &[media_connection(None, Some("google/veo-3.1-lite"))],
            &observed(
                ProbeOutcome::Unreachable {
                    reason: UnreachableReason::Dns,
                    http_status: None,
                },
                chrono::Duration::zero(),
            ),
            &[(AgentType::LiteLlm, false)],
        );
        let entry = connection_entry(&catalogue);
        assert!(entry.configured);
        assert!(!entry.reachable, "{entry:#?}");
        assert!(!entry.available, "{entry:#?}");
        assert!(reason_codes(entry).contains(&"endpoint_unreachable"));
        let connectivity = entry.connectivity.as_ref().unwrap();
        assert_eq!(
            connectivity.state,
            crate::models::WorkerConnectivityState::Unreachable
        );
        assert_eq!(connectivity.unreachable_reason.as_deref(), Some("dns"));
        assert!(
            !entry.media.is_empty(),
            "the media slot stays discoverable while the network is down"
        );

        let legacy = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::LiteLlm)
            .unwrap();
        assert_eq!(legacy.reachable, entry.reachable);
        assert!(reason_codes(legacy).contains(&"endpoint_unreachable"));
        assert_eq!(
            legacy.connectivity.as_ref().unwrap().state,
            crate::models::WorkerConnectivityState::Unreachable
        );
    }

    #[test]
    fn a_named_connection_is_reachable_only_on_a_fresh_verified_probe() {
        use crate::core::endpoint_reachability::ProbeOutcome;
        let reachable = ProbeOutcome::Reachable { http_status: 401 };
        let fresh = catalogue_observing(
            &[media_connection(None, None)],
            &observed(reachable, chrono::Duration::zero()),
            &[],
        );
        let entry = connection_entry(&fresh);
        assert!(entry.reachable && entry.available, "{entry:#?}");
        let connectivity = entry.connectivity.as_ref().unwrap();
        assert_eq!(
            connectivity.state,
            crate::models::WorkerConnectivityState::Verified
        );
        assert!(connectivity.checked_at.is_some());

        // Unknown: no probe ran. Never presented as connectivity.
        let unknown = catalogue_observing(&[media_connection(None, None)], &[], &[]);
        let entry = connection_entry(&unknown);
        assert!(!entry.reachable && !entry.available, "{entry:#?}");
        assert_eq!(reason_codes(entry), vec!["connectivity_unverified"]);
        assert_eq!(
            entry.connectivity.as_ref().unwrap().state,
            crate::models::WorkerConnectivityState::Unverified
        );

        // Stale: a success older than the freshness bound vouches for nothing.
        let stale = catalogue_observing(
            &[media_connection(None, None)],
            &observed(reachable, chrono::Duration::minutes(10)),
            &[],
        );
        let entry = connection_entry(&stale);
        assert!(!entry.reachable, "{entry:#?}");
        let connectivity = entry.connectivity.as_ref().unwrap();
        assert_eq!(
            connectivity.state,
            crate::models::WorkerConnectivityState::Unverified
        );
        assert!(connectivity.checked_at.is_some(), "the age stays visible");
    }

    /// End to end through the real probe: a stub that answers, an address
    /// that does not resolve carrying a credential, and a recovery.
    #[tokio::test]
    async fn the_catalogue_reads_real_probes_without_leaking_the_address() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = [0u8; 1024];
                let _ = socket.read(&mut buffer).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .await;
            }
        });
        let mut up = media_connection(None, None);
        up.endpoint = Some(format!("http://{address}"));
        let mut down = media_connection(None, None);
        down.id = "conn-down".into();
        down.endpoint = Some("https://romu:sk-kt697-secret@kt697-no-such-host.invalid/api".into());

        let started = std::time::Instant::now();
        let observations = bounded_connection_reachability(&[up.clone(), down.clone()]).await;
        server.abort();
        assert!(started.elapsed() < std::time::Duration::from_secs(8));
        let catalogue = catalogue_observing(&[up, down], &observations, &[]);

        let up = connection_entry(&catalogue);
        assert!(up.reachable && up.available, "{up:#?}");
        let down = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.connection_id.as_deref() == Some("conn-down"))
            .unwrap();
        assert!(!down.reachable && !down.available, "{down:#?}");
        assert_eq!(
            down.connectivity
                .as_ref()
                .unwrap()
                .unreachable_reason
                .as_deref(),
            Some("dns")
        );

        let wire = serde_json::to_string(&catalogue).unwrap();
        for secret in [
            "sk-kt697-secret",
            "romu",
            "kt697-no-such-host",
            &address.to_string(),
        ] {
            assert!(!wire.contains(secret), "{secret} leaked: {wire}");
        }
    }

    #[test]
    fn an_external_connection_is_a_worker_of_the_catalogue() {
        // Regression: `Custom` has no local binary, so detection produced
        // nothing for it and a configured OpenRouter was absent from the
        // catalogue entirely — impossible to delegate to.
        let catalogue = catalogue_observing(
            &[media_connection(None, None)],
            &observed(
                crate::core::endpoint_reachability::ProbeOutcome::Reachable { http_status: 200 },
                chrono::Duration::zero(),
            ),
            &[],
        );
        let entry = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::Custom)
            .expect("the connection must be a worker");
        assert_eq!(entry.label, "OpenRouter");
        assert_eq!(entry.worker.connection_id.as_deref(), Some("conn-or"));
        assert!(entry.available, "a connection with a text model is usable");
        assert_eq!(
            entry
                .tiers
                .iter()
                .find(|tier| tier.tier == crate::models::ModelTier::Economy)
                .and_then(|tier| tier.resolved_model.as_deref()),
            Some("qwen/qwen3.8-max")
        );
    }

    /// An agent Kronn launches in a plain discussion has no room session. The
    /// catalogue used to demand one, so on a live instance an agent asked for a
    /// video, was refused by `agent_list` three times, and never learned the
    /// connection id `media_generate` needed.
    #[tokio::test]
    async fn a_plain_discussion_reads_its_worker_catalogue_without_a_room_session() {
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let mut connection = media_connection(None, Some("google/veo-3.1-lite"));
        // Closed on purpose: the reachability probe must fail at once.
        connection.endpoint = Some("http://127.0.0.1:1".into());
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at) \
                 VALUES ('disc-plain', 'Test image', '2026-09-18T00:00:00Z', '2026-09-18T00:00:00Z')",
                [],
            )?;
            crate::db::external_api_connections::insert(conn, &connection)?;
            Ok(())
        })
        .await
        .unwrap();
        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db,
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let read = |discussion: &str| {
            task_worker_catalogue(
                State(state.clone()),
                Json(TaskWorkerCatalogueRequest {
                    parent_discussion_id: discussion.into(),
                }),
            )
        };

        let Json(response) = read("disc-plain").await;
        let catalogue = response
            .data
            .unwrap_or_else(|| panic!("{:?}", response.error));
        let entry = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.connection_id.as_deref() == Some("conn-or"))
            .expect("the configured connection must be listed");
        assert!(
            entry.media.iter().any(|slot| {
                slot.modality == crate::models::MediaModality::Video
                    && slot.model == "google/veo-3.1-lite"
            }),
            "its video slot is what media_generate needs: {:?}",
            entry.media
        );
        // KT-697: the route runs the probe; a closed port is not reachable.
        assert!(!entry.reachable, "{entry:#?}");
        assert_eq!(
            entry
                .connectivity
                .as_ref()
                .and_then(|c| c.unreachable_reason.as_deref()),
            Some("refused")
        );

        let Json(missing) = read("disc-gone").await;
        let error = missing.error.unwrap();
        assert!(
            error.contains("not found") && error.contains("agent_list"),
            "a refusal says what to do: {error}"
        );
    }

    #[test]
    fn media_modalities_are_listed_only_when_configured() {
        let none = catalogue_with(&[media_connection(None, None)]);
        let entry = none
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::Custom)
            .unwrap();
        assert!(
            entry.media.is_empty(),
            "nothing configured must announce nothing"
        );

        let image_only = catalogue_with(&[media_connection(Some("black-forest-labs/flux"), None)]);
        let entry = image_only
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::Custom)
            .unwrap();
        assert_eq!(entry.media.len(), 1, "one modality configured, one listed");
        assert_eq!(entry.media[0].modality, crate::models::MediaModality::Image);
        assert_eq!(entry.media[0].model, "black-forest-labs/flux");
    }

    fn video_envelope(model: &str) -> MediaCapabilityEntry {
        MediaCapabilityEntry {
            connection_id: "conn-or".into(),
            modality: crate::models::MediaModality::Video,
            model: model.into(),
            capabilities: crate::agents::media_capabilities::MediaModelCapabilities {
                model: model.into(),
                modality: crate::models::MediaModality::Video,
                durations_secs: vec![4, 5, 6],
                resolutions: vec!["720p".into()],
                aspect_ratios: vec!["16:9".into()],
                frame_positions: vec![
                    crate::agents::media_capabilities::MediaFramePosition::FirstFrame,
                ],
                max_input_references: Some(1),
                reference_support: None,
                generate_audio: None,
            },
        }
    }

    #[test]
    fn a_listed_modality_carries_the_advertised_envelope() {
        // Regression: an agent asked for a 15 s clip and learned the supported
        // durations only from the provider's refusal — a turn spent, and on
        // some providers a billed attempt. The launcher had the list all along.
        let catalogue = catalogue_with_capabilities(
            &[media_connection(None, Some("bytedance/seedance-2.0-mini"))],
            &[video_envelope("bytedance/seedance-2.0-mini")],
        );
        let entry = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::Custom)
            .unwrap();
        let envelope = entry.media[0]
            .capabilities
            .as_ref()
            .expect("the advertised envelope reaches the agent");
        assert_eq!(envelope.durations_secs, vec![4, 5, 6]);
        assert_eq!(envelope.resolutions, vec!["720p".to_string()]);
        assert_eq!(envelope.aspect_ratios, vec!["16:9".to_string()]);
    }

    #[test]
    fn an_envelope_read_for_another_model_is_not_attached() {
        // The connection's model can change between the sweep and the build.
        // Silence is the only honest answer: attaching the old envelope would
        // advertise durations the new model never named.
        let catalogue = catalogue_with_capabilities(
            &[media_connection(None, Some("bytedance/seedance-2.0-pro"))],
            &[video_envelope("bytedance/seedance-2.0-mini")],
        );
        let entry = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::Custom)
            .unwrap();
        assert_eq!(entry.media[0].model, "bytedance/seedance-2.0-pro");
        assert!(
            entry.media[0].capabilities.is_none(),
            "a stale envelope must not be presented as this model's"
        );
    }

    #[test]
    fn an_unreadable_catalogue_leaves_the_envelope_unstated() {
        // Degrading to `None` rather than to an empty envelope: empty lists
        // would read as "this model supports no duration at all".
        let catalogue =
            catalogue_with(&[media_connection(None, Some("bytedance/seedance-2.0-mini"))]);
        let entry = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::Custom)
            .unwrap();
        assert!(entry.media[0].capabilities.is_none());
    }

    #[test]
    fn both_modalities_are_listed_when_both_are_configured() {
        let catalogue = catalogue_with(&[media_connection(
            Some("black-forest-labs/flux"),
            Some("bytedance/seedance-2.0-mini"),
        )]);
        let entry = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::Custom)
            .unwrap();
        let modalities: Vec<_> = entry.media.iter().map(|m| m.modality).collect();
        assert_eq!(
            modalities,
            vec![
                crate::models::MediaModality::Image,
                crate::models::MediaModality::Video
            ]
        );
    }

    #[test]
    fn a_connection_without_an_endpoint_is_not_a_worker() {
        let mut connection = media_connection(None, None);
        connection.endpoint = None;
        assert!(catalogue_with(&[connection])
            .workers
            .iter()
            .all(|entry| entry.worker.agent_type != AgentType::Custom));
    }

    #[test]
    fn a_connection_with_no_model_at_all_says_so_instead_of_looking_usable() {
        let mut connection = media_connection(None, None);
        connection.economy_model = None;
        let catalogue = catalogue_with(&[connection]);
        let entry = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::Custom)
            .unwrap();
        assert!(!entry.available);
        assert!(entry.reasons.iter().any(|r| r.code == "model_unconfigured"));
    }

    #[test]
    fn native_acp_agents_are_refused_as_launched_workers_with_a_clear_reason() {
        for agent in [
            AgentType::GeminiCli,
            AgentType::CopilotCli,
            AgentType::Kiro,
            AgentType::OpenCode,
        ] {
            let reason = worker_static_refusal(&MessageTarget::agent(agent.clone()))
                .unwrap_or_else(|| panic!("{agent:?} launched worker must be refused"));
            assert_eq!(reason.code, "worker_capability");
            assert_eq!(
                reason.detail,
                crate::agents::runner::NATIVE_ACP_TASK_WORKER_REFUSAL
            );
            // An exact joined CLI session carries its own delivery context.
            assert!(worker_static_refusal(&MessageTarget::cli(agent.clone(), 7)).is_none());
        }
        assert!(worker_static_refusal(&MessageTarget::agent(AgentType::ClaudeCode)).is_none());
        assert!(worker_static_refusal(&MessageTarget::agent(AgentType::Codex)).is_none());
    }

    /// The worker catalogue starts no Copilot CLI to probe an account: a native
    /// Copilot worker is refused anyway.
    #[tokio::test]
    async fn the_worker_catalogue_never_starts_copilot_to_probe_it() {
        let _saved = crate::core::config::test_saved_access::set(&AgentType::CopilotCli, true);
        let detection = crate::models::AgentDetection {
            name: "GitHub Copilot".into(),
            agent_type: AgentType::CopilotCli,
            installed: true,
            enabled: true,
            path: Some("copilot".into()),
            version: None,
            latest_version: None,
            version_checked_at: None,
            version_check_error: None,
            version_source_url: None,
            origin: "test".into(),
            install_command: None,
            host_managed: false,
            host_label: None,
            runtime_available: true,
            auth_ready: Some(true),
            auth_setup_command: None,
            rtk_available: false,
            rtk_hook_configured: false,
            runtime_warning: None,
            shadowed_installs: None,
            fallback_command: None,
        };
        assert!(
            bounded_cli_worker_preflight(std::slice::from_ref(&detection))
                .await
                .is_empty()
        );
    }

    #[test]
    fn worker_catalogue_refuses_copilot_until_its_account_preflight_succeeds() {
        let detection = crate::models::AgentDetection {
            name: "GitHub Copilot".into(),
            agent_type: AgentType::CopilotCli,
            installed: true,
            enabled: true,
            path: Some("copilot".into()),
            version: None,
            latest_version: None,
            version_checked_at: None,
            version_check_error: None,
            version_source_url: None,
            origin: "test".into(),
            install_command: None,
            host_managed: false,
            host_label: None,
            runtime_available: true,
            auth_ready: Some(true),
            auth_setup_command: None,
            rtk_available: false,
            rtk_hook_configured: false,
            runtime_warning: None,
            shadowed_installs: None,
            fallback_command: None,
        };
        let catalogue = build_task_worker_catalogue(
            &crate::core::config::default_config(),
            std::slice::from_ref(&detection),
            &[],
            &[],
            &WorkerPreflight {
                cli: &[(
                    AgentType::CopilotCli,
                    crate::agents::runner::CopilotTaskWorkerPreflight::AuthInvalid,
                )],
                ..Default::default()
            },
        );
        let copilot = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::CopilotCli)
            .expect("Copilot entry");
        assert!(copilot.configured && copilot.reachable);
        assert!(!copilot.available);
        assert!(copilot
            .reasons
            .iter()
            .any(|reason| reason.code == "copilot_auth_invalid"));
        assert!(copilot
            .reasons
            .iter()
            .all(|reason| !reason.detail.contains("token")));

        let timed_out = build_task_worker_catalogue(
            &crate::core::config::default_config(),
            &[detection],
            &[],
            &[],
            &WorkerPreflight {
                cli: &[(
                    AgentType::CopilotCli,
                    crate::agents::runner::CopilotTaskWorkerPreflight::TimedOut,
                )],
                ..Default::default()
            },
        );
        let copilot = timed_out
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::CopilotCli)
            .expect("Copilot entry");
        assert!(!copilot.available);
        assert!(copilot.reasons.iter().any(|reason| {
            reason.code == "copilot_preflight_timed_out"
                && reason
                    .detail
                    .contains("did not finish its bounded account probe")
        }));
    }

    // ── KT-515 — honest availability while a real dispatch reported quota
    // exhaustion. `build_task_worker_catalogue` is pure, so these assert the
    // catalogue reaction to the bounded `quota_exhausted` signal directly; the
    // DB-derived signal itself is covered in `db::orchestration_tests`. ──

    fn codex_detection(installed: bool) -> crate::models::AgentDetection {
        crate::models::AgentDetection {
            name: "Codex".into(),
            agent_type: AgentType::Codex,
            installed,
            enabled: true,
            path: Some("codex".into()),
            version: None,
            latest_version: None,
            version_checked_at: None,
            version_check_error: None,
            version_source_url: None,
            origin: "test".into(),
            install_command: None,
            host_managed: false,
            host_label: None,
            runtime_available: installed,
            auth_ready: Some(true),
            auth_setup_command: None,
            rtk_available: false,
            rtk_hook_configured: false,
            runtime_warning: None,
            shadowed_installs: None,
            fallback_command: None,
        }
    }

    #[test]
    fn worker_catalogue_refuses_codex_while_its_quota_escalation_is_open() {
        let detections = [codex_detection(true)];
        let catalogue = build_task_worker_catalogue(
            &crate::core::config::default_config(),
            &detections,
            &[],
            &[],
            &WorkerPreflight {
                quota_exhausted: &[(AgentType::Codex, true)],
                ..Default::default()
            },
        );
        let codex = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::Codex)
            .expect("Codex entry");
        assert!(!codex.available);
        assert!(
            codex.configured && codex.reachable,
            "transport/auth are fine — only the quota signal must gate availability"
        );
        assert!(codex
            .reasons
            .iter()
            .any(|reason| reason.code == "quota_exhausted"));
        for reason in &codex.reasons {
            assert!(
                !reason.detail.to_lowercase().contains("api key"),
                "quota reason must never leak a secret-shaped detail"
            );
        }
    }

    #[test]
    fn worker_catalogue_offers_codex_once_no_open_quota_escalation_remains() {
        let detections = [codex_detection(true)];
        let catalogue = build_task_worker_catalogue(
            &crate::core::config::default_config(),
            &detections,
            &[],
            &[],
            &WorkerPreflight {
                quota_exhausted: &[(AgentType::Codex, false)],
                ..Default::default()
            },
        );
        let codex = catalogue
            .workers
            .iter()
            .find(|entry| entry.worker.agent_type == AgentType::Codex)
            .expect("Codex entry");
        assert!(
            codex.available,
            "recovered quota must not stay hidden: {codex:#?}"
        );
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
                verification: Default::default(),
                evidence: None,
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
        // KT-1278 — over ACP the brief is the tail of a single message: it
        // ends on its own start directive, never on a reference section.
        let last_section = brief.rsplit("\n## ").next().unwrap();
        assert!(
            last_section.starts_with("Commence maintenant"),
            "{last_section}"
        );
        assert!(brief.contains("outils natifs de ton CLI"), "{brief}");
        assert!(!brief.contains("Premier appel : `search_text`"), "{brief}");
        // KT-788: a CLI worker commits itself, so the brief names the sign-off command.
        assert!(brief.contains("avec `git commit -s`"), "{brief}");
        assert!(
            brief.contains("N'écris jamais de trailer d'identité"),
            "{brief}"
        );
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
                verification: Default::default(),
                evidence: None,
            }],
            "/wt/kt436",
            "kronn/task/KT-436",
            "abc1234",
            true,
            true,
            None,
        );
        assert!(brief.contains("outils natifs de ton CLI"), "{brief}");
        assert!(brief.contains("Lance les tests **ciblés**"), "{brief}");
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

    /// KT-839 — on KT-847 a worker told to "run the validations" started the full
    /// `cargo test` in the background (its shell tool cuts a command at 600 s),
    /// handed the turn back to wait for it and ended WITHOUT delivering, twice
    /// (`worker_completed_without_delivery`). The brief now says who runs the long
    /// gates, and that the worker never waits on a background command.
    #[test]
    fn shell_worker_brief_leaves_the_long_validations_to_kronn_and_delivers_in_one_turn() {
        for (spawned_host_cli, label) in [(true, "spawned host CLI"), (false, "joined CLI")] {
            let brief = worker_brief_markdown(
                "KT-839",
                "Validations inexécutables",
                "Refuser au lancement",
                &[],
                "/wt/kt839",
                "kronn/task/KT-839",
                "abc1234",
                true,
                spawned_host_cli,
                None,
            );
            for promise in [
                // the worker runs the targeted tests…
                "Lance les tests **ciblés**",
                // …the persisted long ones are Kronn's, at integration…
                "Les validations longues persistées par le principal",
                "jouées par Kronn à l'intégration",
                // …and it never waits on a background command…
                "Ne termine jamais ton tour pendant qu'une commande tourne en arrière-plan",
                "plafond par commande de Settings > Workers",
                "`timeout` explicite",
                // …but commits and delivers within the same turn.
                "Enchaîne dans le même tour : tests ciblés, commit, puis `task_exec_deliver`",
            ] {
                assert!(brief.contains(promise), "{label}: `{promise}` in {brief}");
            }
            // The old instruction is what pushed the worker into the long suite.
            assert!(
                !brief.contains("Exécute les commandes de validation"),
                "{label}: {brief}"
            );
            assert!(
                !brief.contains("Exécute les validations pertinentes"),
                "{label}: {brief}"
            );
            // The promise sits in the Tests section, before the delivery format.
            let tests = brief.find("## Tests").expect("tests section");
            let contract = brief.find("jouées par Kronn à l'intégration").unwrap();
            let delivery = brief.find("## Format de livraison").expect("delivery");
            assert!(tests < contract && contract < delivery, "{label}: {brief}");
        }

        // A worker with no shell has no long command to wait on, and is never
        // told it can run one: its own contract (report `skipped`) is unchanged.
        let http = worker_brief_markdown(
            "KT-839",
            "Sous-tâche Ollama",
            "Refuser au lancement",
            &[],
            "/wt/kt839",
            "kronn/task/KT-839",
            "abc1234",
            false,
            true,
            None,
        );
        assert!(http.contains("Tu n'as pas de shell"), "{http}");
        assert!(!http.contains("arrière-plan"), "{http}");
        assert!(!http.contains("plafond par commande"), "{http}");
    }

    /// KT-854: the brief says how to bring the target branch in without erasing
    /// the merge state, for each way a worker can commit — and never tells a
    /// worker without a shell to run a merge it cannot run.
    #[test]
    fn worker_brief_explains_how_to_integrate_the_target_without_erasing_merge_state() {
        let brief_for = |can_run_shell: bool, native_delivery_projection: bool| {
            worker_brief_markdown(
                "KT-854",
                "Intégration",
                "Objectif",
                &[],
                "/wt/kt854",
                "kronn/task/KT-854",
                "abc1234",
                can_run_shell,
                native_delivery_projection,
                None,
            )
        };

        let mediated = brief_for(true, true);
        for needle in [
            "## Intégrer la branche cible",
            "git merge --no-commit --no-ff <cible>",
            "Appelle `task_exec_commit` avec les fichiers résolus",
            "deux parents",
            "N'efface jamais `.git/MERGE_HEAD`",
            "ajoutés des deux côtés",
        ] {
            assert!(mediated.contains(needle), "missing `{needle}`: {mediated}");
        }
        assert!(
            mediated.contains("pas de merge vers la cible"),
            "the blanket merge ban must not contradict the integration section: {mediated}"
        );

        let self_committing = brief_for(true, false);
        assert!(
            self_committing.contains("## Intégrer la branche cible"),
            "{self_committing}"
        );
        assert!(
            self_committing.contains("git merge --signoff <cible>"),
            "{self_committing}"
        );
        assert!(
            self_committing.contains("git commit -s --no-edit"),
            "{self_committing}"
        );
        assert!(
            self_committing.contains("N'efface jamais `.git/MERGE_HEAD`"),
            "{self_committing}"
        );
        assert!(
            !self_committing.contains("task_exec_commit"),
            "a worker that commits itself has no task_exec_commit: {self_committing}"
        );

        let no_shell = brief_for(false, true);
        assert!(
            !no_shell.contains("## Intégrer la branche cible"),
            "{no_shell}"
        );
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "orchestration_trust_tests.rs"]
mod orchestration_trust_tests;

#[cfg(all(test, feature = "full-tests"))]
#[path = "orchestration_ui_capture_tests.rs"]
mod orchestration_ui_capture_tests;

#[cfg(all(test, feature = "full-tests"))]
#[path = "orchestration_generic_brief_tests.rs"]
mod orchestration_generic_brief_tests;
