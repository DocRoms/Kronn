// Durable agent-dispatch runtime. Every detached run is represented by an
// `agent_dispatch_jobs` row before execution, claimed atomically, and only
// completed after the agent's background stream has actually terminated.

use crate::AppState;

use anyhow::Context;
use axum::response::sse::{Event, Sse};

use super::streaming::{
    make_agent_stream_tracked, make_agent_stream_tracked_with_initial_event, AgentExecutionOutcome,
};
use super::SseStream;

const RUNTIME_UNAVAILABLE_RETRY_DELAY_SECONDS: i64 = 30;

struct DispatchHandoffGuard {
    state: Option<AppState>,
    job_id: String,
}

impl DispatchHandoffGuard {
    fn new(state: AppState, job_id: String) -> Self {
        Self {
            state: Some(state),
            job_id,
        }
    }

    fn disarm(&mut self) {
        self.state = None;
    }
}

impl Drop for DispatchHandoffGuard {
    fn drop(&mut self) {
        let Some(state) = self.state.take() else {
            return;
        };
        let job_id = self.job_id.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let retry_id = job_id.clone();
                match state
                    .db
                    .with_conn(move |conn| {
                        crate::db::agent_dispatch::release_unstarted_claim(
                            conn,
                            &retry_id,
                            "claim_handoff_dropped",
                        )
                    })
                    .await
                {
                    Ok(true) => {
                        tracing::warn!("Requeued dispatch {job_id} after dropped claim handoff");
                        state.agent_dispatch_notify.notify_one();
                    }
                    Ok(false) => {}
                    Err(error) => {
                        tracing::error!("Unable to requeue dropped dispatch {job_id}: {error}")
                    }
                }
            });
        }
    }
}

pub(crate) struct DispatchSettlement {
    changed: bool,
    batch_run: Option<crate::models::WorkflowRun>,
}

/// Most refusals the undelivered-worker notice and the delivery summary repeat.
const REFUSED_TOOL_CALLS_MAX: usize = 3;

/// KT-1541 — the tool calls the worker's CLI refused, read back from its
/// transcript (the lines `parse_claude_worker_stream_line` writes), most recent
/// first and without repeats. A worker that stops after a refused edit says
/// nothing else, so the principal otherwise sees only "no delivery"; one that
/// delivers anyway worked around a refusal its report should name. Empty when
/// nothing was refused. The reasons come first-hand, not a count.
///
/// Only the current attempt counts: the worker's room outlives its review
/// rounds, and each round opens with a User message (the brief, then the
/// reviewer's findings), so a refusal from before the last one was met by an
/// earlier attempt. And only a line that starts with the exact prefix the writer
/// emits counts, so a worker that quotes the words in its prose creates none.
pub(crate) fn refused_tool_calls_of_worker(
    conn: &rusqlite::Connection,
    discussion_id: &str,
) -> anyhow::Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT content FROM messages
         WHERE discussion_id = ?1 AND role = 'Agent' AND content LIKE ?2
           AND sort_order > COALESCE(
               (SELECT MAX(sort_order) FROM messages
                 WHERE discussion_id = ?1 AND role = 'User'), -1)
         ORDER BY sort_order DESC LIMIT 20",
    )?;
    let prefix = crate::agents::runner::REFUSED_TOOL_CALL_LINE_PREFIX;
    let contents = statement
        .query_map(
            rusqlite::params![discussion_id, format!("%{prefix}%")],
            |row| row.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let mut reasons: Vec<String> = Vec::new();
    for content in contents {
        for line in content.lines().rev() {
            let Some(reason) = line.strip_prefix(prefix) else {
                continue;
            };
            let reason = reason.trim().to_string();
            if !reason.is_empty() && !reasons.contains(&reason) {
                reasons.push(reason);
            }
        }
    }
    reasons.truncate(REFUSED_TOOL_CALLS_MAX);
    Ok(reasons)
}

/// The refusals as a sentence for the undelivered-worker notice; empty when the
/// worker was refused nothing.
fn refused_tool_calls_sentence(
    conn: &rusqlite::Connection,
    discussion_id: &str,
) -> anyhow::Result<String> {
    let reasons = refused_tool_calls_of_worker(conn, discussion_id)?;
    if reasons.is_empty() {
        return Ok(String::new());
    }
    let list = reasons
        .iter()
        .map(|reason| format!("\n- {reason}"))
        .collect::<String>();
    Ok(format!(
        " Appels d'outil refusés au worker (du plus récent au plus ancien) :{list}\n"
    ))
}

/// Persist a dispatch terminal state and its parent batch progress as one
/// atomic unit. If either write fails, the job remains runnable and recovery
/// can retry without leaving a batch permanently stuck at n-1/N.
pub(crate) fn persist_dispatch_settlement(
    conn: &rusqlite::Connection,
    job_id: &str,
    discussion_id: &str,
    group_id: Option<&str>,
    child_outcome: crate::db::workflows::BatchChildOutcome,
    error: Option<&str>,
    failure_kind: Option<&str>,
) -> anyhow::Result<DispatchSettlement> {
    let transaction = conn.unchecked_transaction()?;
    let changed = if let Some(error) = error {
        if let Some(failure_kind) = failure_kind {
            crate::db::agent_dispatch::mark_failed_with_kind(
                &transaction,
                job_id,
                error,
                failure_kind,
            )?
        } else {
            crate::db::agent_dispatch::mark_failed(&transaction, job_id, error)?
        }
    } else {
        crate::db::agent_dispatch::mark_completed(&transaction, job_id)?
    };
    if changed && failure_kind == Some("quota_exhausted") {
        crate::db::agent_jobs::mark_completion_dispatch_failure(
            &transaction,
            job_id,
            crate::models::AgentResumeJobStatus::QuotaExhausted,
            crate::models::AgentResumeFailureKind::QuotaExhausted,
            error.unwrap_or("provider quota exhausted"),
        )?;
    }
    if changed && failure_kind != Some("quota_exhausted") {
        let interruption_reason = if error.is_some() {
            "worker_failed_without_delivery"
        } else {
            "worker_completed_without_delivery"
        };
        let actor = crate::models::PlanningActor {
            kind: crate::models::PlanningActorKind::Backend,
            id: Some("orchestrator".into()),
            session_id: None,
            source_message_id: None,
        };
        if let Some(interrupted) =
            crate::db::orchestration::interrupt_undelivered_execution_for_dispatch(
                &transaction,
                job_id,
                interruption_reason,
                &actor,
            )?
        {
            let parent = crate::db::discussions::get_discussion(
                &transaction,
                &interrupted.parent_discussion_id,
            )?
            .context("parent discussion vanished before undelivered-worker notice")?;
            let target = crate::db::orchestration::principal_notice_target(
                &transaction,
                &interrupted.execution_id,
                parent.agent,
            )?;
            let (title, instruction) = if error.is_some() {
                (
                    "Worker échoué sans livraison durable",
                    "Examine l'erreur du dispatch dans la sous-discussion, puis réaffecte ou annule explicitement l'exécution.",
                )
            } else {
                (
                    "Worker terminé sans livraison durable",
                    "Relis le bilan du worker dans la sous-discussion, puis reprends l'exécution une fois (`task_exec_resume`, le worktree est conservé), ou réaffecte / annule-la explicitement.",
                )
            };
            let message_id = format!("orch-worker-undelivered:{}", interrupted.execution_id);
            // KT-402 — the deterministic id exists so a resume can never
            // double-post; the insert has to tolerate its own replay for the
            // same reason. Measured: a stream timeout failed the dispatch, the
            // watchdog requeued it, the requeue failed too, and the second
            // settlement died on UNIQUE(messages.id) — leaving the dispatch row
            // unsettled.
            let already_posted: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1)",
                [&message_id],
                |row| row.get(0),
            )?;
            if !already_posted {
                let refused = refused_tool_calls_sentence(&transaction, discussion_id)?;
                let message = crate::api::orchestration::orchestrator_message(
                    message_id,
                    format!(
                        "**{title}**\n\nLe dispatch `{job_id}` de la tâche `{}` s'est terminé sans appel valide à `task_exec_deliver`. L'exécution `{}` est maintenant `Interrupted` au lieu de rester faussement `Working`.{refused}{refusal} {instruction}",
                        interrupted.task_id,
                        interrupted.execution_id,
                        refusal = interrupted
                            .delivery_refusal
                            .as_deref()
                            .map(|detail| format!(
                                " Cause : la livraison du worker a été refusée — {detail}"
                            ))
                            .unwrap_or_default(),
                    ),
                );
                crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
                    &transaction,
                    &interrupted.parent_discussion_id,
                    &message,
                    &[target],
                    &[],
                    None,
                )?;
            }
        }
    }
    let mut batch_run = None;
    if changed {
        let still_awaiting =
            crate::db::agent_dispatch::has_active_for_discussion(&transaction, discussion_id)?;
        crate::db::discussions::set_awaiting_agent(&transaction, discussion_id, still_awaiting)?;
        // This is the authoritative terminal settlement for a tracked/plural QP
        // turn: `clear_awaiting_after_terminal` deliberately defers to us while a
        // dispatch is still active. Once the last dispatch is terminal, purge any
        // retention=0 QP ciphertext here so tracked launches do not keep it
        // forever. Errors propagate (never silently swallowed) so a purge
        // failure rolls the settlement back instead of leaking the secret.
        if !still_awaiting {
            crate::db::execution_variable_snapshots::purge_run_lifetime_snapshot(
                &transaction,
                "quick_prompt",
                discussion_id,
                chrono::Utc::now(),
            )?;
            crate::db::execution_variable_snapshots::purge_run_lifetime_snapshot(
                &transaction,
                "quick_prompt_batch_item",
                discussion_id,
                chrono::Utc::now(),
            )?;
        }
        if let Some(run_id) = group_id {
            batch_run = crate::db::workflows::increment_batch_progress_with_outcome(
                &transaction,
                run_id,
                child_outcome,
            )?;
        }
    }
    transaction.commit()?;
    Ok(DispatchSettlement { changed, batch_run })
}

/// Start the process-wide durable dispatch worker.
///
/// The periodic scan is a crash-safe fallback; normal producers also notify
/// the worker immediately through `agent_dispatch_notify`.
pub fn start_agent_dispatcher(state: AppState) {
    tokio::spawn(async move {
        loop {
            let exhausted = state
                .db
                .with_conn(|conn| crate::db::agent_dispatch::list_exhausted_ids(conn, 64))
                .await
                .unwrap_or_else(|error| {
                    tracing::error!("Agent dispatch exhausted-job scan failed: {error}");
                    Vec::new()
                });
            for id in exhausted {
                let lookup_id = id.clone();
                match state
                    .db
                    .with_conn(move |conn| crate::db::agent_dispatch::get(conn, &lookup_id))
                    .await
                {
                    Ok(Some(job)) => {
                        fail_dispatch_job(&state, &job, "maximum dispatch attempts exhausted").await
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::error!("Unable to load exhausted dispatch {id}: {error}")
                    }
                }
            }

            let runnable = state
                .db
                .with_conn(|conn| crate::db::agent_dispatch::list_runnable_ids(conn, 64))
                .await;
            let ids = match runnable {
                Ok(ids) => ids,
                Err(error) => {
                    tracing::error!("Agent dispatch scan failed: {error}");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    continue;
                }
            };

            if ids.is_empty() {
                tokio::select! {
                    _ = state.agent_dispatch_notify.notified() => {}
                    _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
                }
                continue;
            }

            for id in ids {
                let worker_state = state.clone();
                tokio::spawn(async move {
                    dispatch_job_by_id(worker_state, id).await;
                });
            }
            // Give the spawned claim transactions time to run before scanning
            // the same Pending rows again. Duplicate execution is still
            // prevented by the atomic Pending -> Running claim.
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    });
}

/// Spawn an agent run and wait for its durable job to reach a terminal state.
pub async fn spawn_agent_run_background(state: AppState, discussion_id: String) {
    spawn_agent_run_with_chain(state, discussion_id, Vec::new(), None).await;
}

/// Spawn an agent run and, after it completes, execute chained Quick Prompts
/// sequentially inside the SAME discussion. Each chain step:
///
/// 1. Load the QP → render its `prompt_template` with the batch item value
///    substituted for the first variable (if any) → insert as a User message
/// 2. Re-fire the agent (via `make_agent_stream`)
/// 3. Wait for the agent to finish
///
/// The batch progress hook fires only after the final chain step.
///
/// `chain_prompt_ids` is the list of QP IDs to fire AFTER the initial run.
/// Empty = no chain, same as `spawn_agent_run_background`.
///
/// `batch_item` is the raw item value (e.g. "EW-1234") that the primary
/// QP consumed. When `Some`, every chain QP with a first variable gets
/// that variable filled with the same value — so `analyse → review →
/// summary` on ticket EW-1234 all receive `EW-1234` in their respective
/// first var. When `None` (non-batch context), chain QPs are inserted
/// verbatim; templates with unfilled `{{var}}` will reach the agent as-is.
pub async fn spawn_agent_run_with_chain(
    state: AppState,
    discussion_id: String,
    chain_prompt_ids: Vec<String>,
    batch_item: Option<String>,
) {
    let did = discussion_id.clone();
    let job_id = uuid::Uuid::new_v4().to_string();
    let dedupe_key = format!("runtime:{did}:{job_id}");
    let enqueue_id = job_id.clone();
    let enqueue_did = did.clone();
    let enqueue_chain = chain_prompt_ids.clone();
    let enqueue_item = batch_item.clone();
    let job = match state
        .db
        .with_conn(move |conn| {
            if let Some(active) =
                crate::db::agent_dispatch::find_active_for_discussion(conn, &enqueue_did)?
            {
                return Ok(active);
            }
            let transaction = conn.unchecked_transaction()?;
            let job = crate::db::agent_dispatch::enqueue_for_latest_user(
                &transaction,
                crate::db::agent_dispatch::NewLatestUserDispatch {
                    id: &enqueue_id,
                    discussion_id: &enqueue_did,
                    dedupe_key: &dedupe_key,
                    agent_override: None,
                    chain_prompt_ids: &enqueue_chain,
                    batch_item: enqueue_item.as_deref(),
                    group_id: None,
                    group_concurrency_limit: None,
                },
            )?;
            crate::db::discussions::set_awaiting_agent(&transaction, &enqueue_did, true)?;
            transaction.commit()?;
            Ok(job)
        })
        .await
    {
        Ok(job) => job,
        Err(error) => {
            tracing::error!("Unable to enqueue agent run for {discussion_id}: {error}");
            return;
        }
    };

    state.agent_dispatch_notify.notify_one();
    wait_for_dispatch_job(state, job.id).await;
}

async fn wait_for_dispatch_job(state: AppState, job_id: String) {
    loop {
        // The caller also attempts the claim so workflow-local concurrency
        // permits cover the real run even if the global worker has not polled
        // yet. Claim remains exclusive with the process-wide worker.
        dispatch_job_by_id(state.clone(), job_id.clone()).await;

        let lookup_id = job_id.clone();
        let status = state
            .db
            .with_conn(move |conn| crate::db::agent_dispatch::get(conn, &lookup_id))
            .await;
        match status {
            Ok(Some(job))
                if matches!(
                    job.status,
                    crate::db::agent_dispatch::DispatchStatus::Completed
                        | crate::db::agent_dispatch::DispatchStatus::Failed
                        | crate::db::agent_dispatch::DispatchStatus::Cancelled
                ) =>
            {
                return;
            }
            Ok(Some(_)) => {}
            Ok(None) => {
                tracing::error!("Agent dispatch job {job_id} disappeared");
                return;
            }
            Err(error) => {
                tracing::error!("Agent dispatch job {job_id} lookup failed: {error}");
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

async fn dispatch_job_by_id(state: AppState, job_id: String) {
    if let Some(stream) = stream_dispatch_job(state, job_id, None).await {
        // The completion monitor owns the actual run. A background worker has
        // no SSE client, so dropping only the receiver is intentional.
        drop(stream);
    }
}

/// Claim a durable dispatch job and expose its live SSE stream when the caller
/// is an HTTP request. The spawned completion monitor owns the power lease and
/// terminal DB transition, so disconnecting the browser cannot orphan work.
pub(crate) async fn stream_dispatch_job(
    state: AppState,
    job_id: String,
    initial_event: Option<Event>,
) -> Option<Sse<SseStream>> {
    let claim_id = job_id.clone();
    // Admission is where a local slot is actually granted: the claim refuses a
    // job whose agent already has all of its runs in flight, atomically, so two
    // dispatch scans cannot both believe the slot is free.
    let limits = {
        let config = state.config.read().await;
        crate::agents::runner::dispatch_concurrency_limits(&config)
    };
    let job = match state
        .db
        .with_conn(move |conn| {
            crate::db::agent_dispatch::claim_with_limits(conn, &claim_id, Some(&limits))
        })
        .await
    {
        Ok(Some(job)) => job,
        Ok(None) => return None,
        Err(error) => {
            tracing::error!("Agent dispatch claim failed for {job_id}: {error}");
            return None;
        }
    };
    stream_claimed_dispatch_job(state, job, initial_event).await
}

pub(crate) async fn stream_claimed_dispatch_job(
    state: AppState,
    job: crate::db::agent_dispatch::AgentDispatchJob,
    initial_event: Option<Event>,
) -> Option<Sse<SseStream>> {
    let mut handoff_guard = DispatchHandoffGuard::new(state.clone(), job.id.clone());
    // If the process crashed after persisting the answer but before marking
    // the job complete, do not spend tokens a second time.
    let existing = if job.attempts > 1 {
        let recovery_job = job.clone();
        state
            .db
            .with_conn(move |conn| {
                crate::db::agent_dispatch::latest_completed_agent_message(conn, &recovery_job)
            })
            .await
    } else {
        Ok(None)
    };
    match existing {
        Ok(Some((_message_id, content, succeeded))) => {
            finish_dispatch_turn(&state, job, content, succeeded).await;
            handoff_guard.disarm();
            return None;
        }
        Ok(None) => {}
        Err(error) => {
            fail_dispatch_job(&state, &job, &format!("response recovery failed: {error}")).await;
            handoff_guard.disarm();
            return None;
        }
    }

    let status_id = job.id.clone();
    let current_status = state
        .db
        .with_conn(move |conn| crate::db::agent_dispatch::get(conn, &status_id))
        .await;
    match current_status {
        Ok(Some(current))
            if current.status == crate::db::agent_dispatch::DispatchStatus::Running => {}
        Ok(_) => {
            handoff_guard.disarm();
            return None;
        }
        Err(error) => {
            tracing::error!("Unable to verify claimed dispatch {}: {error}", job.id);
            return None;
        }
    }

    if hold_validation_during_audit(&state, &job).await {
        handoff_guard.disarm();
        return None;
    }

    let power_lease = crate::core::power_guard::acquire();
    let target_tier = {
        let trigger_message_id = job.trigger_message_id.clone();
        let agent_override = job.agent_override.clone();
        let connection_id = job.connection_id.clone();
        state
            .db
            .with_read_conn(move |conn| {
                let targets =
                    crate::db::discussions::list_message_targets(conn, &trigger_message_id)?;
                Ok(dispatch_target_tier(
                    &targets,
                    agent_override.as_ref(),
                    connection_id.as_deref(),
                ))
            })
            .await
            .ok()
            .flatten()
    };
    let (stream, completion) = match initial_event {
        Some(event) => {
            make_agent_stream_tracked_with_initial_event(
                state.clone(),
                job.discussion_id.clone(),
                job.agent_override.clone(),
                target_tier,
                job.id.clone(),
                event,
            )
            .await
        }
        None => {
            make_agent_stream_tracked(
                state.clone(),
                job.discussion_id.clone(),
                job.agent_override.clone(),
                target_tier,
                job.id.clone(),
            )
            .await
        }
    };
    tokio::spawn(async move {
        let _power_lease = power_lease;
        let outcome =
            completion
                .await
                .unwrap_or_else(|_| AgentExecutionOutcome::RuntimeUnavailable {
                    reason: "agent_completion_channel_dropped".to_string(),
                });
        // KT-1111 — this dispatch's reply is written: its turn values go.
        state.db.assistant_guard().end_turn(&job.id);
        // KT-1266 — whatever a task worker started (a backend, a Vite) ends
        // with its dispatch, however the dispatch ended.
        reap_task_worker_processes(&state, &job.id).await;
        let status_id = job.id.clone();
        let cancelled = state
            .db
            .with_conn(move |conn| crate::db::agent_dispatch::get(conn, &status_id))
            .await
            .ok()
            .flatten()
            .is_some_and(|current| {
                current.status == crate::db::agent_dispatch::DispatchStatus::Cancelled
            });
        if cancelled {
            // stop_agent settles the Cancelled job and its parent batch in
            // one transaction. The completion observer must only wake
            // waiters; incrementing here would double-count the child.
            state.agent_dispatch_notify.notify_waiters();
            return;
        }
        let reported_success = match outcome {
            AgentExecutionOutcome::Finished { success } => success,
            AgentExecutionOutcome::RuntimeUnavailable { reason } => {
                defer_runtime_unavailable(&state, &job, &reason).await;
                return;
            }
            AgentExecutionOutcome::PreflightFailed { diagnostic } => {
                fail_dispatch_job(&state, &job, &diagnostic).await;
                ensure_failure_visible(&state, &job, &diagnostic).await;
                return;
            }
        };

        let completed_job = job.clone();
        let response = state
            .db
            .with_conn(move |conn| {
                crate::db::agent_dispatch::latest_completed_agent_message(conn, &completed_job)
            })
            .await;
        match response {
            Ok(Some((_message_id, content, succeeded))) => {
                if succeeded != reported_success {
                    tracing::warn!(
                        "Agent dispatch {} completion signal disagrees with durable reply metadata",
                        job.id
                    );
                }
                finish_dispatch_turn(&state, job, content, succeeded).await
            }
            Ok(None) => {
                fail_dispatch_job_with_outcome(
                    &state,
                    &job,
                    "agent finished without a durable reply",
                    crate::db::workflows::BatchChildOutcome::NoResponse,
                )
                .await;
                ensure_failure_visible(&state, &job, "agent finished without a durable reply")
                    .await;
            }
            Err(error) => {
                fail_dispatch_job(&state, &job, &format!("reply lookup failed: {error}")).await
            }
        }
    });
    handoff_guard.disarm();
    Some(stream)
}

/// Stop the processes a task worker's dispatch left behind (KT-1266). Only a
/// dispatch that is a worker's launch has any; an ordinary turn costs one read.
pub(crate) async fn reap_task_worker_processes(state: &AppState, dispatch_job_id: &str) {
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

fn dispatch_target_tier(
    targets: &[crate::models::MessageTarget],
    agent_override: Option<&crate::models::AgentType>,
    connection_id: Option<&str>,
) -> Option<crate::models::ModelTier> {
    targets
        .iter()
        .find(|target| match agent_override {
            Some(agent) => {
                target.kind == crate::models::MessageTargetKind::Agent
                    && &target.agent_type == agent
                    && target.connection_id.as_deref() == connection_id
            }
            None => {
                target.kind == crate::models::MessageTargetKind::DiscussionAgent
                    && target.connection_id.as_deref() == connection_id
            }
        })
        .and_then(|target| target.tier)
}

fn select_quota_alternative(
    allowed_agents: &[crate::models::AgentType],
    current: &crate::models::AgentType,
    available_agents: &[crate::models::AgentType],
) -> Option<crate::models::AgentType> {
    // An empty allow-list means "no explicit fallback policy", not "all agents".
    // Ordering is the persisted KT-321 policy order, so the choice is stable and
    // explainable rather than dependent on detection order.
    allowed_agents
        .iter()
        .find(|candidate| *candidate != current && available_agents.contains(candidate))
        .cloned()
}

/// KT-1564 — a provider fallback on quota exhaustion needs the campaign's own,
/// explicit `quota_fallback` opt-in. `allowed_agents` only lists the providers
/// a worker may be given; it is never read as "switch to them when the quota
/// is spent". The move keeps the execution's tier and profile (a model name is
/// provider-specific and is dropped), remembers the worker it leaves and the
/// reset the refusal announced, and the watchdog hands it back after that reset.
async fn try_quota_provider_fallback(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    announced_reset: Option<chrono::DateTime<chrono::Utc>>,
) -> anyhow::Result<Option<(crate::models::AgentType, String)>> {
    let available_agents = crate::api::orchestration::available_agent_types_for_state(state).await;
    try_quota_provider_fallback_among(state, job, announced_reset, available_agents).await
}

/// [`try_quota_provider_fallback`] with the providers known to be available
/// given, so the policy is testable without the CLIs installed on the host.
pub(crate) async fn try_quota_provider_fallback_among(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    announced_reset: Option<chrono::DateTime<chrono::Utc>>,
    available_agents: Vec<crate::models::AgentType>,
) -> anyhow::Result<Option<(crate::models::AgentType, String)>> {
    let dispatch_id = job.id.clone();
    let candidate = state
        .db
        .with_conn(move |conn| {
            let Some(execution) =
                crate::db::orchestration::get_execution_for_dispatch(conn, &dispatch_id)?
            else {
                return Ok(None);
            };
            if execution.status.is_terminal() {
                return Ok(None);
            }
            let current = execution
                .worker_agent_type
                .as_deref()
                .map(crate::db::orchestration::agent_type_from_db)
                .transpose()?
                .context("quota fallback execution has no current provider")?;
            let run = crate::db::orchestration::get_orchestration_run(
                conn,
                &execution.orchestration_run_id,
            )?
            .context("quota fallback orchestration run vanished")?;
            if !run.quota_fallback {
                return Ok(None);
            }
            // A launch that named its own alternatives falls back among those;
            // a campaign among the allow-list it opted in with.
            let candidates = if run.quota_fallback_agents.is_empty() {
                &run.allowed_agents
            } else {
                &run.quota_fallback_agents
            };
            // The provider that just refused is remembered with the reset it
            // announced: a fallback leaves no escalation behind, and a later
            // one must not walk back into it before it is back.
            crate::db::orchestration::note_provider_quota_refusal(
                conn,
                &crate::db::orchestration::agent_type_to_db(&current),
                announced_reset,
            )?;
            // Never a candidate: the worker the execution was first given (the
            // sweep hands it back once its reset has passed, a fallback does
            // not), and any provider whose refusal has not reset yet.
            let origin = crate::db::orchestration::quota_origin(conn, &execution.id)?;
            let now = chrono::Utc::now();
            // Providers that already refused this execution during the chain.
            // A refusal that announced no reset leaves nothing the global
            // clock can test: without this, two such providers would hand the
            // execution back and forth forever.
            let refused =
                crate::db::orchestration::execution_refused_providers(conn, &execution.id, now)?;
            let mut usable = Vec::new();
            for candidate in candidates {
                let provider = crate::db::orchestration::agent_type_to_db(candidate);
                if origin
                    .as_ref()
                    .is_some_and(|origin| origin.provider == provider)
                    || refused.contains(&provider)
                    || crate::db::orchestration::provider_quota_unavailable_at(
                        conn, &provider, None, now,
                    )?
                {
                    continue;
                }
                usable.push(candidate.clone());
            }
            let Some(alternative) = select_quota_alternative(&usable, &current, &available_agents)
            else {
                return Ok(None);
            };
            let tier = execution
                .worker_model_tier
                .as_deref()
                .and_then(crate::api::orchestration::model_tier_from_db);
            Ok(Some((
                execution.id,
                alternative,
                tier,
                execution.worker_profile_id,
            )))
        })
        .await?;
    let Some((execution_id, alternative, tier, profile_id)) = candidate else {
        return Ok(None);
    };
    let mut target = crate::models::MessageTarget::agent(alternative.clone());
    target.tier = tier;
    let selection = crate::models::CampaignWorkerSelection {
        target,
        model: None,
        profile_id,
    };
    let view = crate::api::orchestration::reassign_native_execution_as(
        state,
        &execution_id,
        selection,
        "automatic quota fallback: the campaign opted into a provider fallback on quota exhaustion",
        crate::api::orchestration::ReassignCause::QuotaFallback {
            reset_at: announced_reset,
        },
    )
    .await?;
    state.agent_dispatch_notify.notify_one();
    Ok(Some((alternative, view.execution.parent_discussion_id)))
}

async fn finish_dispatch_turn(
    state: &AppState,
    job: crate::db::agent_dispatch::AgentDispatchJob,
    response: String,
    execution_succeeded: bool,
) {
    if !execution_succeeded && super::is_hard_quota_exhausted(&response) {
        let detail = response.trim().chars().take(800).collect::<String>();
        let durable_error = if detail.is_empty() {
            "quota_exhausted: provider plan or credit limit reached".to_string()
        } else {
            format!("quota_exhausted: {detail}")
        };
        fail_dispatch_job_with_outcome_kind(
            state,
            &job,
            &durable_error,
            crate::db::workflows::BatchChildOutcome::Failed,
            Some("quota_exhausted"),
        )
        .await;
        return;
    }
    if super::message_matches_silent_crash(&response) && job.turn_attempts < 2 {
        tracing::warn!(
            "Discussion {} matched silent-crash pattern — retrying once",
            job.discussion_id
        );
        let retry_id = job.id.clone();
        let retry_discussion_id = job.discussion_id.clone();
        let retried = state
            .db
            .with_conn(move |conn| {
                let transaction = conn.unchecked_transaction()?;
                let queued = crate::db::agent_dispatch::retry_after(
                    &transaction,
                    &retry_id,
                    5,
                    "silent_agent_crash",
                )?;
                anyhow::ensure!(queued, "silent-crash dispatch is no longer running");
                crate::db::discussions::delete_dispatch_reply_messages(
                    &transaction,
                    &retry_discussion_id,
                    &retry_id,
                )?;
                crate::db::discussions::set_awaiting_agent(
                    &transaction,
                    &retry_discussion_id,
                    true,
                )?;
                transaction.commit()?;
                Ok(())
            })
            .await;
        if let Err(error) = retried {
            fail_dispatch_job(state, &job, &format!("silent-crash retry failed: {error}")).await;
        } else {
            state.agent_dispatch_notify.notify_one();
        }
        return;
    }

    // A saturated provider is the one failure worth waiting out. Same shape as
    // the silent-crash retry above, with a longer, growing delay: retrying a
    // 529 immediately just meets the same saturation. The last attempt falls
    // through without deleting the reply, so the room keeps the provider's own
    // message instead of going quiet (issue 202).
    if !execution_succeeded
        && super::is_transient_provider_overload(&response)
        && job.turn_attempts < 2
    {
        let delay_seconds = 15 * (i64::from(job.turn_attempts) + 1);
        tracing::warn!(
            "Discussion {} hit a transient provider overload — retrying in {}s",
            job.discussion_id,
            delay_seconds
        );
        let retry_id = job.id.clone();
        let retry_discussion_id = job.discussion_id.clone();
        let retried = state
            .db
            .with_conn(move |conn| {
                let transaction = conn.unchecked_transaction()?;
                let queued = crate::db::agent_dispatch::retry_after(
                    &transaction,
                    &retry_id,
                    delay_seconds,
                    "transient_provider_overload",
                )?;
                anyhow::ensure!(queued, "overloaded dispatch is no longer running");
                crate::db::discussions::delete_dispatch_reply_messages(
                    &transaction,
                    &retry_discussion_id,
                    &retry_id,
                )?;
                crate::db::discussions::set_awaiting_agent(
                    &transaction,
                    &retry_discussion_id,
                    true,
                )?;
                transaction.commit()?;
                Ok(())
            })
            .await;
        if let Err(error) = retried {
            fail_dispatch_job(
                state,
                &job,
                &format!("provider-overload retry failed: {error}"),
            )
            .await;
        } else {
            state.agent_dispatch_notify.notify_one();
        }
        return;
    }

    if !execution_succeeded {
        let dispatch_id = job.id.clone();
        let is_task_worker = state
            .db
            .with_conn(move |conn| {
                Ok(
                    crate::db::orchestration::get_execution_for_dispatch(conn, &dispatch_id)?
                        .is_some(),
                )
            })
            .await
            // On a read failure, prefer the wording that states the real cause
            // over one that sends the reader to a sub-discussion we can't confirm.
            .unwrap_or(false);
        fail_dispatch_job(
            state,
            &job,
            &unsuccessful_completion_diagnostic(&response, is_task_worker),
        )
        .await;
        return;
    }

    if let Some(qp_id) = job.chain_prompt_ids.get(job.next_chain_index) {
        let lookup_id = qp_id.clone();
        let discussion_id = job.discussion_id.clone();
        let qp = state
            .db
            .with_conn(move |conn| {
                crate::workflows::run_pins::chain_prompt_for(conn, &discussion_id, &lookup_id)
            })
            .await;
        let qp = match qp {
            Ok(Err(reason)) => {
                fail_dispatch_job(state, &job, &format!("chain QP '{qp_id}': {reason}")).await;
                return;
            }
            Ok(Ok(Some(qp))) => qp,
            Ok(Ok(None)) => {
                fail_dispatch_job(state, &job, &format!("chain QP '{qp_id}' not found")).await;
                return;
            }
            Err(error) => {
                fail_dispatch_job(state, &job, &format!("chain QP lookup failed: {error}")).await;
                return;
            }
        };
        let approval_prompt = qp.clone();
        if let Err(error) = state
            .db
            .with_read_conn(move |conn| {
                crate::core::repository_resources::ensure_quick_prompt_execution_approved(
                    conn,
                    &approval_prompt,
                )
                .map_err(anyhow::Error::msg)
            })
            .await
        {
            fail_dispatch_job(
                state,
                &job,
                &format!("chain QuickPrompt preflight failed: {error}"),
            )
            .await;
            return;
        }
        let message = crate::models::DiscussionMessage {
            recovered_partial: false,
            session_tokens_at_message: None,
            author_cli_ordinal: None,
            session_continuity: None,
            kind: Default::default(),
            model: None,
            lint_report: None,
            id: uuid::Uuid::new_v4().to_string(),
            role: crate::models::MessageRole::User,
            channel: crate::models::MessageChannel::Main,
            content: render_chain_qp_prompt(
                &qp.prompt_template,
                qp.variables.first().map(|variable| variable.name.as_str()),
                job.batch_item.as_deref(),
                &response,
            ),
            agent_type: None,
            timestamp: chrono::Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo: Some(format!("⚡ {}", qp.name)),
            author_avatar_email: None,
            source_msg_id: None,
            duration_ms: None,
            target_agent: None,
            reply_to_message_id: None,
        };
        let advance_id = job.id.clone();
        match state
            .db
            .with_conn(move |conn| {
                crate::db::agent_dispatch::advance_chain_trigger(conn, &advance_id, &message)
            })
            .await
        {
            Ok(true) => state.agent_dispatch_notify.notify_one(),
            Ok(false) => {
                tracing::warn!(
                    "Agent dispatch {} changed state before chain advance",
                    job.id
                )
            }
            Err(error) => {
                fail_dispatch_job(state, &job, &format!("chain advance failed: {error}")).await
            }
        }
        return;
    }

    let complete_id = job.id.clone();
    let complete_discussion_id = job.discussion_id.clone();
    let complete_group_id = job.group_id.clone();
    let completed = state
        .db
        .with_conn(move |conn| {
            persist_dispatch_settlement(
                conn,
                &complete_id,
                &complete_discussion_id,
                complete_group_id.as_deref(),
                crate::db::workflows::BatchChildOutcome::Success,
                None,
                None,
            )
        })
        .await;
    match completed {
        Ok(DispatchSettlement {
            changed: true,
            batch_run,
        }) => {
            if let Some(updated_run) = batch_run {
                super::streaming::broadcast_batch_progress(state, &job.discussion_id, &updated_run);
            }
        }
        Ok(DispatchSettlement { changed: false, .. }) => {
            settle_cancelled_dispatch(state, &job).await;
            return;
        }
        Err(error) => {
            tracing::error!("Unable to complete agent dispatch {}: {error}", job.id);
            return;
        }
    }
    state.agent_dispatch_notify.notify_waiters();
}

/// Durable `last_error` for a turn that ended unsuccessfully.
///
/// Two callers, two vocabularies. A task worker is expected to submit a
/// delivery manifest, so naming the manifest and `task_exec_reassign` is the
/// actionable next step. An ordinary room has neither, and the worker wording
/// sent readers of issue 202 looking for a sub-discussion that never existed —
/// while the provider's own reason (an `HTTP 529`, say) was dropped from the
/// one field an operator inspects after the fact. Keep the reason.
fn unsuccessful_completion_diagnostic(response: &str, is_task_worker: bool) -> String {
    let failure_kind = if response.trim().is_empty() {
        "no_visible_output"
    } else {
        "worker_reported_failure"
    };
    if is_task_worker {
        return format!(
            "task-worker startup/completion failed: phase=worker_completion; \
             failure_kind={failure_kind}; exit=unsuccessful. The worker did not submit a durable \
             delivery manifest. Inspect the worker sub-discussion, then use `task_exec_reassign` \
             to preserve this execution and its worktree while selecting another available worker."
        );
    }
    // The reply is already visible in the room, so carrying it here exposes
    // nothing new — it only puts the cause where an operator looks for it.
    let cause = response.trim().chars().take(800).collect::<String>();
    if cause.is_empty() {
        format!(
            "agent turn failed: failure_kind={failure_kind}; exit=unsuccessful. \
             The agent produced no visible output."
        )
    } else {
        format!("agent turn failed: failure_kind={failure_kind}; exit=unsuccessful. {cause}")
    }
}

async fn fail_dispatch_job(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    error: &str,
) {
    fail_dispatch_job_with_outcome(
        state,
        job,
        error,
        crate::db::workflows::BatchChildOutcome::Failed,
    )
    .await;
}

async fn fail_dispatch_job_with_outcome(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    error: &str,
    child_outcome: crate::db::workflows::BatchChildOutcome,
) {
    fail_dispatch_job_with_outcome_kind(state, job, error, child_outcome, None).await;
}

async fn fail_dispatch_job_with_outcome_kind(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    error: &str,
    child_outcome: crate::db::workflows::BatchChildOutcome,
    failure_kind: Option<&str>,
) {
    tracing::error!("Agent dispatch {} failed: {error}", job.id);
    let fail_id = job.id.clone();
    let fail_discussion_id = job.discussion_id.clone();
    let fail_group_id = job.group_id.clone();
    let fail_error = error.to_string();
    let quota_exhausted = failure_kind == Some("quota_exhausted");
    let failure_kind = failure_kind.map(str::to_string);
    let failed = state
        .db
        .with_conn(move |conn| {
            persist_dispatch_settlement(
                conn,
                &fail_id,
                &fail_discussion_id,
                fail_group_id.as_deref(),
                child_outcome,
                Some(&fail_error),
                failure_kind.as_deref(),
            )
        })
        .await;
    match failed {
        Ok(DispatchSettlement {
            changed: true,
            batch_run,
        }) => {
            if let Some(updated_run) = batch_run {
                super::streaming::broadcast_batch_progress(state, &job.discussion_id, &updated_run);
            }
        }
        Ok(DispatchSettlement { changed: false, .. }) => {
            settle_cancelled_dispatch(state, job).await;
            return;
        }
        Err(db_error) => {
            tracing::error!("Unable to persist failed dispatch {}: {db_error}", job.id);
            return;
        }
    }
    if quota_exhausted {
        let provider = job
            .agent_override
            .as_ref()
            .map(|agent| format!("{agent:?}"))
            .unwrap_or_else(|| "discussion provider".into());
        // The provider may have said when it resets (KT-838): a fallback keeps
        // it to come back, and the escalation below shows it.
        let announced_reset = super::announced_quota_reset(error, chrono::Utc::now());
        match try_quota_provider_fallback(state, job, announced_reset).await {
            Ok(Some((alternative, target_discussion))) => {
                let message = crate::api::orchestration::orchestrator_message(
                    format!("provider-quota-fallback:{}", job.id),
                    format!(
                        "**Bascule automatique — quota épuisé**\n\nLe provider `{provider}` a épuisé son quota sur le dispatch `{}`. La campagne a explicitement activé le repli quota (`quota_fallback`) et autorise `{alternative:?}`, disponible : Kronn a réassigné la même exécution dans sa sous-discussion et son worktree existants, sans perdre l'historique ni les constats. Le changement de provider apparaît dans la carte de livraison ; si la limite annonce sa réinitialisation, Kronn remet l'exécution sur le worker d'origine passé cet instant.",
                        job.id
                    ),
                );
                if let Err(db_error) = state
                    .db
                    .with_conn(move |conn| {
                        crate::db::discussions::insert_message(conn, &target_discussion, &message)?;
                        Ok(())
                    })
                    .await
                {
                    tracing::warn!(dispatch_job_id = %job.id, "quota fallback notice could not be persisted: {db_error}");
                }
            }
            fallback => {
                if let Err(error) = fallback {
                    tracing::warn!(dispatch_job_id = %job.id, "authorized quota fallback failed: {error}");
                }
                let dispatch_id = job.id.clone();
                let provider_for_db = provider.clone();
                let escalation = state
                    .db
                    .with_conn(move |conn| {
                        crate::db::orchestration::escalate_execution_for_dispatch_quota(
                            conn,
                            &dispatch_id,
                            &provider_for_db,
                            announced_reset,
                        )
                    })
                    .await;
                let target_discussion = match escalation {
                    Ok(Some((_execution_id, parent_discussion_id))) => parent_discussion_id,
                    Ok(None) => job.discussion_id.clone(),
                    Err(db_error) => {
                        tracing::error!(dispatch_job_id = %job.id, "quota escalation failed: {db_error}");
                        job.discussion_id.clone()
                    }
                };
                // KT-1564 — a refusal that announced its reset closes itself.
                let return_note = announced_reset
                    .map(|at| {
                        format!(
                            " La limite annonce sa réinitialisation à {} : passé cet instant, Kronn remet l'exécution sur `{provider}` (même niveau) sans changer de provider.",
                            at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                        )
                    })
                    .unwrap_or_default();
                let message = crate::api::orchestration::orchestrator_message(
                    format!("provider-quota-escalation:{}", job.id),
                    format!(
                        "**Intervention humaine requise — quota épuisé**\n\nLe provider `{provider}` a refusé le dispatch `{}` pour limite de plan/crédits. Aucun provider alternatif explicitement autorisé n'est disponible ; Kronn ne relancera pas le même compte avant la réinitialisation.{return_note} Sans échéance annoncée : bascule de compte ou réaffectation explicite requise.",
                        job.id
                    ),
                );
                if let Err(db_error) = state
                    .db
                    .with_conn(move |conn| {
                        crate::db::discussions::insert_message(conn, &target_discussion, &message)?;
                        Ok(())
                    })
                    .await
                {
                    tracing::warn!(dispatch_job_id = %job.id, "quota notice could not be persisted: {db_error}");
                }
            }
        }
    }
    state.agent_dispatch_notify.notify_waiters();
}

/// KT-1607 — a settled dispatch must leave something the room can read. A refusal
/// before the provider call and a run that wrote no reply used to settle the job
/// with a `last_error` only, so the discussion showed nothing: no reply, no error
/// and no system note. When nothing is linked to the dispatch, write the card that
/// says why, retryable from the room like any other refused start.
async fn ensure_failure_visible(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    reason: &str,
) {
    let lookup = job.clone();
    let loaded = state
        .db
        .with_read_conn(move |conn| {
            let has_message: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages
                 WHERE discussion_id = ?1 AND agent_dispatch_job_id = ?2)",
                rusqlite::params![lookup.discussion_id, lookup.id],
                |row| row.get(0),
            )?;
            if has_message {
                return Ok(None);
            }
            Ok(
                crate::db::discussions::get_discussion(conn, &lookup.discussion_id)?.map(
                    |discussion| {
                        (
                            discussion.agent,
                            discussion.tier,
                            discussion.model,
                            discussion.language,
                        )
                    },
                ),
            )
        })
        .await;
    let (agent, tier, model, language) = match loaded {
        Ok(Some(loaded)) => loaded,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(dispatch_job_id = %job.id, "unable to check the dispatch's visible trace: {error}");
            return;
        }
    };
    let agent = job.agent_override.clone().unwrap_or(agent);
    let content =
        super::start_refusal::unexplained_end_card(&language, reason, tier, Some(&job.id));
    super::start_refusal::record_refusal_card(
        state,
        super::start_refusal::RefusedStart {
            discussion_id: &job.discussion_id,
            agent_type: &agent,
            model,
            tier,
            dispatch_id: Some(&job.id),
            tracked_dispatch: true,
            reply_to: Some(job.trigger_message_id.clone()),
        },
        content,
    )
    .await;
}

/// KT-1134 — seconds an audit validation agent waits, queued, while an audit of
/// its project runs.
const VALIDATION_HOLD_RETRY_DELAY_SECONDS: i64 = 5;

/// KT-1134 — an agent answering in a validation discussion linked to an audit
/// run writes `docs/`: it never starts while an audit of the same project
/// holds the documentation lease. The claimed job goes back to the queue, its attempts intact.
async fn hold_validation_during_audit(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
) -> bool {
    let discussion_id = job.discussion_id.clone();
    let project = state
        .db
        .with_read_conn(move |conn| {
            crate::db::audit_runs::project_of_validation_discussion(conn, &discussion_id)
        })
        .await
        .ok()
        .flatten();
    let Some(project_id) = project else {
        return false;
    };
    let audit_running = state
        .audit_tracker
        .lock()
        .map(|tracker| {
            tracker.leased.contains(&project_id) || tracker.progress.contains_key(&project_id)
        })
        .unwrap_or(true);
    if !audit_running {
        return false;
    }
    let job_id = job.id.clone();
    let held = state
        .db
        .with_conn(move |conn| {
            crate::db::agent_dispatch::defer_runtime_unavailable(
                conn,
                &job_id,
                VALIDATION_HOLD_RETRY_DELAY_SECONDS,
                "audit_running: an audit of this project is writing docs/",
            )
        })
        .await;
    match held {
        Ok(true) => {
            tracing::info!(
                "Held validation dispatch {} for {}s: an audit of project {project_id} runs",
                job.id,
                VALIDATION_HOLD_RETRY_DELAY_SECONDS
            );
            true
        }
        Ok(false) => true,
        Err(error) => {
            tracing::error!("Unable to hold validation dispatch {}: {error}", job.id);
            false
        }
    }
}

async fn defer_runtime_unavailable(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    reason: &str,
) {
    let job_id = job.id.clone();
    let persisted_reason = format!("runtime_unavailable: {reason}");
    match state
        .db
        .with_conn(move |conn| {
            crate::db::agent_dispatch::defer_runtime_unavailable(
                conn,
                &job_id,
                RUNTIME_UNAVAILABLE_RETRY_DELAY_SECONDS,
                &persisted_reason,
            )
        })
        .await
    {
        Ok(true) => tracing::warn!(
            "Deferred dispatch {} for {}s because its runtime is unavailable: {}",
            job.id,
            RUNTIME_UNAVAILABLE_RETRY_DELAY_SECONDS,
            reason
        ),
        Ok(false) => {}
        Err(error) => tracing::error!(
            "Unable to defer dispatch {} after unavailable runtime: {}",
            job.id,
            error
        ),
    }
    state.agent_dispatch_notify.notify_waiters();
}

async fn settle_cancelled_dispatch(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
) {
    let status_id = job.id.clone();
    let cancelled = state
        .db
        .with_conn(move |conn| crate::db::agent_dispatch::get(conn, &status_id))
        .await
        .ok()
        .flatten()
        .is_some_and(|current| {
            current.status == crate::db::agent_dispatch::DispatchStatus::Cancelled
        });
    if cancelled {
        // The cancellation endpoint already advanced the parent batch in the
        // same transaction that marked this job Cancelled.
        state.agent_dispatch_notify.notify_waiters();
    }
}

/// Render a chain QP's prompt template, substituting:
///   - `{{previous_qp.output}}` → the previous agent reply (Phase 4)
///   - `{{<first_var_name>}}` → `batch_item` (existing Phase 2 behavior)
///
/// Pure helper extracted from `spawn_agent_run_with_chain` so the
/// substitution rules are unit-testable without a tokio runtime / DB.
/// Order matters: chain-var substitution runs FIRST so a user-controlled
/// `batch_item` value can't smuggle a literal `{{previous_qp.output}}`
/// past us (no double-substitution surface). When no previous agent
/// reply is available, the chain-var resolves to empty string —
/// template rendering must never fail the chain.
pub(crate) fn render_chain_qp_prompt(
    template: &str,
    first_var_name: Option<&str>,
    batch_item: Option<&str>,
    previous_output: &str,
) -> String {
    let mut out = template.replace("{{previous_qp.output}}", previous_output);
    if let (Some(item), Some(var)) = (batch_item, first_var_name) {
        let placeholder = format!("{{{{{}}}}}", var);
        out = out.replace(&placeholder, item);
    }
    out
}

#[cfg(all(test, feature = "full-tests"))]
mod chain_render_tests {
    use super::{
        dispatch_target_tier, ensure_failure_visible, persist_dispatch_settlement,
        render_chain_qp_prompt, select_quota_alternative, unsuccessful_completion_diagnostic,
        DispatchHandoffGuard,
    };
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[test]
    fn an_ordinary_room_failure_states_the_cause_not_a_worker_manifest() {
        let diagnostic = unsuccessful_completion_diagnostic(
            "[Agent provider error]\n\nAPI Error: 529 Overloaded\n\n(HTTP 529)",
            false,
        );

        // The reason must survive into `last_error`: it is where an operator
        // looks once the turn is over.
        assert!(
            diagnostic.contains("529 Overloaded"),
            "the provider reason must be preserved, got: {diagnostic}"
        );
        // And none of the task-worker vocabulary, which sent readers of issue
        // 202 looking for a sub-discussion that never existed.
        for absent in [
            "task-worker",
            "delivery manifest",
            "task_exec_reassign",
            "worker sub-discussion",
        ] {
            assert!(
                !diagnostic.contains(absent),
                "an ordinary room must not mention {absent:?}, got: {diagnostic}"
            );
        }
    }

    #[test]
    fn a_task_worker_failure_still_names_the_manifest_and_the_reassign_tool() {
        let diagnostic = unsuccessful_completion_diagnostic("worker gave up", true);

        assert!(diagnostic.contains("task-worker"));
        assert!(diagnostic.contains("delivery manifest"));
        assert!(diagnostic.contains("task_exec_reassign"));
        assert!(diagnostic.contains("failure_kind=worker_reported_failure"));
    }

    #[test]
    fn a_silent_ordinary_turn_says_so_without_inventing_a_cause() {
        let diagnostic = unsuccessful_completion_diagnostic("   \n  ", false);

        assert!(diagnostic.contains("failure_kind=no_visible_output"));
        assert!(diagnostic.contains("no visible output"));
        assert!(!diagnostic.contains("task-worker"));
    }

    #[test]
    fn a_long_cause_is_truncated_on_a_char_boundary() {
        // Accented text: a byte-index cut would panic mid-character.
        let long_cause = "é".repeat(2_000);
        let diagnostic = unsuccessful_completion_diagnostic(&long_cause, false);

        let carried = diagnostic.matches('é').count();
        assert_eq!(carried, 800, "exactly the first 800 chars are carried");
    }

    #[test]
    fn dispatch_uses_the_tier_of_its_exact_native_target() {
        use crate::models::{AgentType, MessageTarget, ModelTier};

        let targets = vec![
            MessageTarget::discussion_agent(AgentType::LiteLlm).with_tier(ModelTier::Reasoning),
            MessageTarget::agent(AgentType::Codex).with_tier(ModelTier::Economy),
            MessageTarget::cli(AgentType::Codex, 42),
        ];

        assert_eq!(
            dispatch_target_tier(&targets, None, None),
            Some(ModelTier::Reasoning)
        );
        assert_eq!(
            dispatch_target_tier(&targets, Some(&AgentType::Codex), None),
            Some(ModelTier::Economy)
        );
        assert_eq!(
            dispatch_target_tier(&targets, Some(&AgentType::Ollama), None),
            None
        );
    }

    #[test]
    fn dispatch_tier_distinguishes_named_custom_connections() {
        use crate::models::{AgentType, MessageTarget, ModelTier};

        let targets = vec![
            MessageTarget::agent(AgentType::Custom)
                .with_connection("openrouter")
                .with_tier(ModelTier::Reasoning),
            MessageTarget::agent(AgentType::Custom)
                .with_connection("groq")
                .with_tier(ModelTier::Economy),
        ];

        assert_eq!(
            dispatch_target_tier(&targets, Some(&AgentType::Custom), Some("openrouter")),
            Some(ModelTier::Reasoning)
        );
        assert_eq!(
            dispatch_target_tier(&targets, Some(&AgentType::Custom), Some("groq")),
            Some(ModelTier::Economy)
        );
    }

    #[test]
    fn quota_fallback_requires_an_explicit_available_alternative() {
        use crate::models::AgentType;

        assert_eq!(
            select_quota_alternative(
                &[AgentType::Codex, AgentType::Ollama, AgentType::LiteLlm],
                &AgentType::Codex,
                &[AgentType::Codex, AgentType::LiteLlm],
            ),
            Some(AgentType::LiteLlm)
        );
        assert_eq!(
            select_quota_alternative(
                &[AgentType::Codex, AgentType::Ollama],
                &AgentType::Codex,
                &[AgentType::Codex],
            ),
            None
        );
        assert_eq!(
            select_quota_alternative(&[], &AgentType::Codex, &[AgentType::Ollama]),
            None,
            "an empty allow-list is not implicit permission to switch providers"
        );
    }

    #[test]
    fn previous_qp_output_is_substituted() {
        // Phase 4 — chain QP consumes the previous agent reply via
        // `{{previous_qp.output}}`. Use case: "brief → plan → tickets".
        let out = render_chain_qp_prompt(
            "Make tickets from this plan:\n{{previous_qp.output}}",
            None,
            None,
            "Step 1: foo\nStep 2: bar",
        );
        assert!(out.contains("Step 1: foo\nStep 2: bar"));
        assert!(!out.contains("{{previous_qp.output}}"));
    }

    #[test]
    fn previous_qp_output_substituted_with_empty_when_no_previous() {
        // If the agent crashed before replying, the chain var must
        // resolve to empty string — never leave the placeholder syntax
        // exposed to the agent prompt.
        let out =
            render_chain_qp_prompt("Refine:\n{{previous_qp.output}}\n— done.", None, None, "");
        assert_eq!(out, "Refine:\n\n— done.");
    }

    #[test]
    fn first_var_substituted_with_batch_item() {
        // Phase 2 behavior — first user-defined var receives the batch
        // item value. Unchanged by Phase 4.
        let out = render_chain_qp_prompt("Analyse {{ticket}}", Some("ticket"), Some("EW-1234"), "");
        assert_eq!(out, "Analyse EW-1234");
    }

    #[test]
    fn previous_output_and_batch_item_both_substituted() {
        let out = render_chain_qp_prompt(
            "On {{ticket}}: refine the plan below.\n{{previous_qp.output}}",
            Some("ticket"),
            Some("EW-1234"),
            "Plan v1",
        );
        assert_eq!(out, "On EW-1234: refine the plan below.\nPlan v1",);
    }

    #[test]
    fn batch_item_carrying_chain_var_does_not_double_substitute() {
        // Regression guard: a malicious `batch_item` MUST NOT smuggle
        // a `{{previous_qp.output}}` placeholder that would then be
        // resolved post-hoc. Order in `render_chain_qp_prompt` runs
        // chain-var substitution FIRST, so by the time batch_item lands
        // the chain-var pass is already over. The literal text from the
        // batch item survives intact.
        let out = render_chain_qp_prompt(
            "Title: {{ticket}}",
            Some("ticket"),
            Some("{{previous_qp.output}}-EW-1"),
            "<<should-not-leak>>",
        );
        assert_eq!(
            out, "Title: {{previous_qp.output}}-EW-1",
            "batch_item value must not be re-rendered against the chain var"
        );
    }

    #[test]
    fn no_var_no_batch_item_returns_template_as_is() {
        let out = render_chain_qp_prompt("Static prompt", None, None, "");
        assert_eq!(out, "Static prompt");
    }

    /// KT-1607 — a dispatch that settled with a reason but wrote nothing leaves
    /// a System card naming it, once; a dispatch that already has a trace in the
    /// room gets no second one.
    #[tokio::test]
    async fn a_settled_dispatch_with_no_trace_in_the_room_gets_a_card_with_its_reason() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let job = db
            .with_conn(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO discussions (id, title, created_at, updated_at)
                     VALUES ('d-vis', 'Vis', ?1, ?1)",
                    [&now],
                )?;
                conn.execute(
                    "INSERT INTO messages
                     (id, discussion_id, role, content, timestamp, sort_order, received_at)
                     VALUES ('u-vis', 'd-vis', 'User', 'go', ?1, 1, ?1)",
                    [&now],
                )?;
                conn.execute(
                    "UPDATE discussions SET next_message_seq = 2 WHERE id = 'd-vis'",
                    [],
                )?;
                let job = crate::db::agent_dispatch::enqueue_for_latest_user(
                    conn,
                    crate::db::agent_dispatch::NewLatestUserDispatch {
                        id: "j-vis",
                        discussion_id: "d-vis",
                        dedupe_key: "message:u-vis",
                        agent_override: None,
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: None,
                        group_concurrency_limit: None,
                    },
                )?;
                Ok(job)
            })
            .await
            .unwrap();
        let state = crate::AppState::new_defaults(
            Arc::new(RwLock::new(crate::core::config::default_config())),
            db,
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let system_cards = |state: &crate::AppState| {
            let db = state.db.clone();
            async move {
                db.with_conn(|conn| {
                    let mut statement = conn.prepare(
                        "SELECT content FROM messages
                         WHERE discussion_id = 'd-vis' AND role = 'System'",
                    )?;
                    let rows = statement
                        .query_map([], |row| row.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(rows)
                })
                .await
                .unwrap()
            }
        };

        ensure_failure_visible(&state, &job, "agent finished without a durable reply").await;
        let cards = system_cards(&state).await;
        assert_eq!(cards.len(), 1, "{cards:?}");
        assert!(cards[0].starts_with("[kronn:agent-error]\n"), "{cards:?}");
        assert!(
            cards[0].contains("agent finished without a durable reply"),
            "{cards:?}"
        );
        assert!(cards[0].contains("j-vis"), "retryable from the room: {cards:?}");

        ensure_failure_visible(&state, &job, "agent finished without a durable reply").await;
        assert_eq!(
            system_cards(&state).await.len(),
            1,
            "a dispatch with a trace in the room gets no second card"
        );
    }

    #[tokio::test]
    async fn dropped_handoff_guard_requeues_the_unstarted_claim() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('d-guard', 'Guard', ?1, ?1)",
                [&now],
            )?;
            conn.execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at)
                 VALUES ('u-guard', 'd-guard', 'User', 'go', ?1, 1, ?1)",
                [&now],
            )?;
            crate::db::agent_dispatch::enqueue_for_latest_user(
                conn,
                crate::db::agent_dispatch::NewLatestUserDispatch {
                    id: "j-guard",
                    discussion_id: "d-guard",
                    dedupe_key: "message:u-guard",
                    agent_override: None,
                    chain_prompt_ids: &[],
                    batch_item: None,
                    group_id: None,
                    group_concurrency_limit: None,
                },
            )?;
            crate::db::agent_dispatch::claim(conn, "j-guard")?;
            Ok(())
        })
        .await
        .unwrap();
        let state = crate::AppState::new_defaults(
            Arc::new(RwLock::new(crate::core::config::default_config())),
            db,
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );

        drop(DispatchHandoffGuard::new(state.clone(), "j-guard".into()));

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let job = state
                    .db
                    .with_conn(|conn| crate::db::agent_dispatch::get(conn, "j-guard"))
                    .await
                    .unwrap()
                    .unwrap();
                if job.status == crate::db::agent_dispatch::DispatchStatus::Pending {
                    assert_eq!(job.attempts, 0);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("handoff guard requeues promptly");
    }

    #[tokio::test]
    async fn dispatch_and_batch_progress_commit_or_rollback_together() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            let now = chrono::Utc::now();
            let now_text = now.to_rfc3339();
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('d-atomic', 'Atomic batch', ?1, ?1)",
                [&now_text],
            )?;
            conn.execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at)
                 VALUES ('u-atomic', 'd-atomic', 'User', 'go', ?1, 1, ?1)",
                [&now_text],
            )?;
            crate::db::workflows::ensure_batch_placeholder_workflow(
                conn,
                "qp-atomic",
                "Atomic QP",
                None,
            )?;
            crate::db::workflows::insert_run(
                conn,
                &crate::models::WorkflowRun {
                    outcome: None,
                    id: "batch-atomic".into(),
                    workflow_id: "qp:qp-atomic".into(),
                    status: crate::models::RunStatus::Running,
                    trigger_context: None,
                    step_results: vec![],
                    tokens_used: 0,
                    workspace_path: None,
                    started_at: now,
                    finished_at: None,
                    run_type: "batch".into(),
                    batch_total: 1,
                    batch_completed: 0,
                    batch_failed: 0,
                    batch_no_response: 0,
                    batch_name: Some("Atomic batch".into()),
                    parent_run_id: None,
                    state: std::collections::HashMap::new(),
                    produced_branches: vec![],
                    concurrency_key: None,
                    triggered_by_run_id: None,
                    project_id: None,
                    parent_workflow_id: None,
                    parent_workflow_name: None,
                    parent_run_started_at: None,
                },
            )?;
            crate::db::agent_dispatch::enqueue_for_latest_user(
                conn,
                crate::db::agent_dispatch::NewLatestUserDispatch {
                    id: "j-atomic",
                    discussion_id: "d-atomic",
                    dedupe_key: "message:u-atomic",
                    agent_override: None,
                    chain_prompt_ids: &[],
                    batch_item: None,
                    group_id: Some("batch-atomic"),
                    group_concurrency_limit: None,
                },
            )?;
            crate::db::agent_dispatch::claim(conn, "j-atomic")?;
            conn.execute_batch(
                "CREATE TRIGGER reject_batch_progress
                 BEFORE UPDATE ON workflow_runs
                 BEGIN
                   SELECT RAISE(ABORT, 'forced batch progress failure');
                 END;",
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let failed = db
            .with_conn(|conn| {
                persist_dispatch_settlement(
                    conn,
                    "j-atomic",
                    "d-atomic",
                    Some("batch-atomic"),
                    crate::db::workflows::BatchChildOutcome::Success,
                    None,
                    None,
                )
            })
            .await;
        assert!(failed.is_err());

        db.with_conn(|conn| {
            let job = crate::db::agent_dispatch::get(conn, "j-atomic")?.unwrap();
            assert_eq!(
                job.status,
                crate::db::agent_dispatch::DispatchStatus::Running
            );
            let run = crate::db::workflows::get_run(conn, "batch-atomic")?.unwrap();
            assert_eq!(run.batch_completed, 0);
            assert_eq!(run.status, crate::models::RunStatus::Running);
            conn.execute_batch("DROP TRIGGER reject_batch_progress;")?;
            Ok(())
        })
        .await
        .unwrap();

        let settled = db
            .with_conn(|conn| {
                persist_dispatch_settlement(
                    conn,
                    "j-atomic",
                    "d-atomic",
                    Some("batch-atomic"),
                    crate::db::workflows::BatchChildOutcome::Success,
                    None,
                    None,
                )
            })
            .await
            .unwrap();
        assert!(settled.changed);
        let batch = settled.batch_run.unwrap();
        assert_eq!(batch.batch_completed, 1);
        assert_eq!(batch.status, crate::models::RunStatus::Success);
    }

    #[tokio::test]
    async fn tracked_qp_settlement_purges_zero_retention_ciphertext_at_terminal() {
        // A tracked QP dispatch settles here, not in `clear_awaiting_after_terminal`.
        // Once the last dispatch is terminal, retention=0 ciphertext must be purged
        // on this authoritative path too — otherwise tracked launches keep it forever.
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            let now = chrono::Utc::now();
            let now_text = now.to_rfc3339();
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('d-purge', 'Purge', ?1, ?1)",
                [&now_text],
            )?;
            conn.execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at)
                 VALUES ('u-purge', 'd-purge', 'User', 'go', ?1, 1, ?1)",
                [&now_text],
            )?;
            let values =
                std::collections::HashMap::from([("token".to_string(), "ephemeral".to_string())]);
            crate::db::execution_variable_snapshots::insert(
                conn,
                crate::db::execution_variable_snapshots::NewSnapshot {
                    run_kind: "quick_prompt",
                    run_id: "d-purge",
                    project_id: None,
                    environment_ref: "project_mcp_configs",
                    resolved_at: now,
                    retention_days: 0,
                    expires_at: None,
                    values: &values,
                    provenance: &[],
                },
                &[6u8; 32],
            )?;
            crate::db::agent_dispatch::enqueue_for_latest_user(
                conn,
                crate::db::agent_dispatch::NewLatestUserDispatch {
                    id: "j-purge",
                    discussion_id: "d-purge",
                    dedupe_key: "message:u-purge",
                    agent_override: None,
                    chain_prompt_ids: &[],
                    batch_item: None,
                    group_id: None,
                    group_concurrency_limit: None,
                },
            )?;
            crate::db::agent_dispatch::claim(conn, "j-purge")?;
            Ok(())
        })
        .await
        .unwrap();

        db.with_conn(|conn| {
            persist_dispatch_settlement(
                conn,
                "j-purge",
                "d-purge",
                None,
                crate::db::workflows::BatchChildOutcome::Success,
                None,
                None,
            )
        })
        .await
        .unwrap();

        db.with_conn(|conn| {
            let purged: bool = conn.query_row(
                "SELECT values_encrypted IS NULL FROM execution_variable_snapshots
                 WHERE run_kind='quick_prompt' AND run_id='d-purge'",
                [],
                |row| row.get(0),
            )?;
            assert!(
                purged,
                "zero-retention ciphertext must be purged at terminal settlement"
            );
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn completed_task_worker_without_delivery_interrupts_and_wakes_the_principal() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let execution_id = db
            .with_conn(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                for (id, title) in [("d-parent", "Parent"), ("d-child", "Child")] {
                    conn.execute(
                        "INSERT INTO discussions (id, title, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?3)",
                        rusqlite::params![id, title, now],
                    )?;
                }
                conn.execute(
                    "INSERT INTO planning_tasks
                     (id, task_number, title, status, created_at, updated_at)
                     VALUES ('t-undelivered', 9901, 'Undelivered', 'todo', ?1, ?1)",
                    [&now],
                )?;
                let actor = crate::models::PlanningActor {
                    kind: crate::models::PlanningActorKind::Backend,
                    id: Some("orchestrator".into()),
                    session_id: None,
                    source_message_id: None,
                };
                let execution = crate::db::orchestration::launch_single_task(
                    conn,
                    &crate::models::LaunchSingleTaskInput::new("t-undelivered", "d-parent"),
                    &actor,
                )?
                .execution;
                for status in [
                    crate::models::TaskExecutionStatus::Provisioning,
                    crate::models::TaskExecutionStatus::Working,
                ] {
                    crate::db::orchestration::transition_execution(
                        conn,
                        &execution.id,
                        status,
                        &actor,
                        serde_json::json!({}),
                    )?;
                }
                conn.execute(
                    "INSERT INTO messages
                     (id, discussion_id, role, content, timestamp, sort_order, received_at)
                     VALUES ('u-undelivered', 'd-child', 'User', 'go', ?1, 1, ?1)",
                    [&now],
                )?;
                crate::db::agent_dispatch::enqueue_for_latest_user(
                    conn,
                    crate::db::agent_dispatch::NewLatestUserDispatch {
                        id: "j-undelivered",
                        discussion_id: "d-child",
                        dedupe_key: "message:u-undelivered",
                        agent_override: Some(&crate::models::AgentType::Ollama),
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: None,
                        group_concurrency_limit: None,
                    },
                )?;
                crate::db::agent_dispatch::claim(conn, "j-undelivered")?;
                crate::db::orchestration::attach_execution_dispatch(
                    conn,
                    &execution.id,
                    "j-undelivered",
                )?;
                Ok(execution.id)
            })
            .await
            .unwrap();

        let settled = db
            .with_conn(|conn| {
                persist_dispatch_settlement(
                    conn,
                    "j-undelivered",
                    "d-child",
                    None,
                    crate::db::workflows::BatchChildOutcome::Success,
                    None,
                    None,
                )
            })
            .await
            .unwrap();
        assert!(settled.changed);

        db.with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                .expect("execution must remain queryable");
            assert_eq!(
                execution.status,
                crate::models::TaskExecutionStatus::Interrupted
            );
            let notice_id = format!("orch-worker-undelivered:{execution_id}");
            let notice: String = conn.query_row(
                "SELECT content FROM messages WHERE id = ?1 AND discussion_id = 'd-parent'",
                [&notice_id],
                |row| row.get(0),
            )?;
            assert!(notice.contains("sans livraison durable"));
            assert_eq!(
                crate::db::discussions::list_message_targets(conn, &notice_id)?.len(),
                1,
                "the principal wake must carry a typed target"
            );
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn failed_task_worker_without_delivery_interrupts_and_wakes_the_principal() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let execution_id = db
            .with_conn(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                for (id, title) in [("d-fail-parent", "Parent"), ("d-fail-child", "Child")] {
                    conn.execute(
                        "INSERT INTO discussions (id, title, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?3)",
                        rusqlite::params![id, title, now],
                    )?;
                }
                conn.execute(
                    "INSERT INTO planning_tasks
                     (id, task_number, title, status, created_at, updated_at)
                     VALUES ('t-failed-undelivered', 9902, 'Failed', 'todo', ?1, ?1)",
                    [&now],
                )?;
                let actor = crate::models::PlanningActor {
                    kind: crate::models::PlanningActorKind::Backend,
                    id: Some("orchestrator".into()),
                    session_id: None,
                    source_message_id: None,
                };
                let execution = crate::db::orchestration::launch_single_task(
                    conn,
                    &crate::models::LaunchSingleTaskInput::new(
                        "t-failed-undelivered",
                        "d-fail-parent",
                    ),
                    &actor,
                )?
                .execution;
                for status in [
                    crate::models::TaskExecutionStatus::Provisioning,
                    crate::models::TaskExecutionStatus::Working,
                ] {
                    crate::db::orchestration::transition_execution(
                        conn,
                        &execution.id,
                        status,
                        &actor,
                        serde_json::json!({}),
                    )?;
                }
                conn.execute(
                    "INSERT INTO messages
                     (id, discussion_id, role, content, timestamp, sort_order, received_at)
                     VALUES ('u-failed-undelivered', 'd-fail-child', 'User', 'go', ?1, 1, ?1)",
                    [&now],
                )?;
                crate::db::agent_dispatch::enqueue_for_latest_user(
                    conn,
                    crate::db::agent_dispatch::NewLatestUserDispatch {
                        id: "j-failed-undelivered",
                        discussion_id: "d-fail-child",
                        dedupe_key: "message:u-failed-undelivered",
                        agent_override: Some(&crate::models::AgentType::Ollama),
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: None,
                        group_concurrency_limit: None,
                    },
                )?;
                crate::db::agent_dispatch::claim(conn, "j-failed-undelivered")?;
                crate::db::orchestration::attach_execution_dispatch(
                    conn,
                    &execution.id,
                    "j-failed-undelivered",
                )?;
                Ok(execution.id)
            })
            .await
            .unwrap();

        let settled = db
            .with_conn(|conn| {
                persist_dispatch_settlement(
                    conn,
                    "j-failed-undelivered",
                    "d-fail-child",
                    None,
                    crate::db::workflows::BatchChildOutcome::Failed,
                    Some("provider crashed"),
                    Some("runtime_error"),
                )
            })
            .await
            .unwrap();
        assert!(settled.changed);

        db.with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                .expect("execution must remain queryable");
            assert_eq!(
                execution.status,
                crate::models::TaskExecutionStatus::Interrupted
            );
            let recovery = crate::db::orchestration::get_execution_recovery(conn, &execution_id)?
                .expect("failure must leave an explicit human recovery");
            assert_eq!(recovery.recovery_reason, "worker_failed_without_delivery");
            let notice_id = format!("orch-worker-undelivered:{execution_id}");
            let notice: String = conn.query_row(
                "SELECT content FROM messages WHERE id = ?1 AND discussion_id = 'd-fail-parent'",
                [&notice_id],
                |row| row.get(0),
            )?;
            assert!(notice.contains("Worker échoué sans livraison durable"));
            assert!(notice.contains("Examine l'erreur du dispatch"));
            assert_eq!(
                crate::db::discussions::list_message_targets(conn, &notice_id)?.len(),
                1
            );
            Ok(())
        })
        .await
        .unwrap();
    }

    /// KT-860 — a run the catalogue preflight refuses keeps the refused model
    /// and the reason in `last_error`, not only a generic sentence.
    #[tokio::test]
    async fn kt860_a_preflight_refusal_keeps_its_model_and_reason_in_last_error() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            let target = crate::db::model_catalog::agent_runtime_target_id(
                &crate::models::AgentType::ClaudeCode,
            );
            let listed = |id: &str| crate::db::model_catalog::DiscoveredModel {
                model_id: id.into(),
                display_name: id.into(),
                resolved_model: None,
                description: None,
                capabilities: vec!["chat".into()],
                reasoning_modes: vec![],
                default_reasoning_mode: None,
            };
            crate::db::model_catalog::reconcile_live(
                conn,
                &target,
                &crate::models::AgentType::ClaudeCode,
                &[listed("claude-fable-5[1m]"), listed("sonnet")],
            )?;
            crate::db::model_catalog::reconcile_live(
                conn,
                &target,
                &crate::models::AgentType::ClaudeCode,
                &[listed("sonnet")],
            )
        })
        .await
        .unwrap();
        let failure = crate::core::model_catalog::preflight_check(
            &db,
            None,
            crate::models::AgentType::ClaudeCode,
            crate::models::ModelTier::Reasoning,
            Some("claude-fable-5[1m]"),
            None,
        )
        .await
        .expect("the disappeared alias has no replacement");
        let diagnostic = crate::core::model_catalog::preflight_refusal_diagnostic(&failure);

        let execution_id = db
            .with_conn(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                for (id, title) in [("d-pf-parent", "Parent"), ("d-pf-child", "Child")] {
                    conn.execute(
                        "INSERT INTO discussions (id, title, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?3)",
                        rusqlite::params![id, title, now],
                    )?;
                }
                conn.execute(
                    "INSERT INTO planning_tasks
                     (id, task_number, title, status, created_at, updated_at)
                     VALUES ('t-preflight-refused', 9903, 'Refused', 'todo', ?1, ?1)",
                    [&now],
                )?;
                let actor = crate::models::PlanningActor {
                    kind: crate::models::PlanningActorKind::Backend,
                    id: Some("orchestrator".into()),
                    session_id: None,
                    source_message_id: None,
                };
                let execution = crate::db::orchestration::launch_single_task(
                    conn,
                    &crate::models::LaunchSingleTaskInput::new(
                        "t-preflight-refused",
                        "d-pf-parent",
                    ),
                    &actor,
                )?
                .execution;
                for status in [
                    crate::models::TaskExecutionStatus::Provisioning,
                    crate::models::TaskExecutionStatus::Working,
                ] {
                    crate::db::orchestration::transition_execution(
                        conn,
                        &execution.id,
                        status,
                        &actor,
                        serde_json::json!({}),
                    )?;
                }
                conn.execute(
                    "INSERT INTO messages
                     (id, discussion_id, role, content, timestamp, sort_order, received_at)
                     VALUES ('u-preflight-refused', 'd-pf-child', 'User', 'go', ?1, 1, ?1)",
                    [&now],
                )?;
                crate::db::agent_dispatch::enqueue_for_latest_user(
                    conn,
                    crate::db::agent_dispatch::NewLatestUserDispatch {
                        id: "j-preflight-refused",
                        discussion_id: "d-pf-child",
                        dedupe_key: "message:u-preflight-refused",
                        agent_override: Some(&crate::models::AgentType::ClaudeCode),
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: None,
                        group_concurrency_limit: None,
                    },
                )?;
                crate::db::agent_dispatch::claim(conn, "j-preflight-refused")?;
                crate::db::orchestration::attach_execution_dispatch(
                    conn,
                    &execution.id,
                    "j-preflight-refused",
                )?;
                Ok(execution.id)
            })
            .await
            .unwrap();

        // The settlement `PreflightFailed { diagnostic }` reaches, as the
        // dispatcher runs it for a refused preflight.
        let (tx, rx) = tokio::sync::oneshot::channel();
        super::super::streaming::finish_tracked_preflight(&mut Some(tx), &diagnostic);
        let super::AgentExecutionOutcome::PreflightFailed {
            diagnostic: settled,
        } = rx.await.unwrap()
        else {
            panic!("a refused preflight settles as PreflightFailed");
        };
        db.with_conn(move |conn| {
            persist_dispatch_settlement(
                conn,
                "j-preflight-refused",
                "d-pf-child",
                None,
                crate::db::workflows::BatchChildOutcome::Failed,
                Some(&settled),
                None,
            )
        })
        .await
        .unwrap();

        db.with_conn(move |conn| {
            let detail = crate::api::orchestration::execution_detail(conn, &execution_id)?;
            let compact = crate::api::orchestration::compact_execution_status(&detail, None);
            let last_error = compact.last_error.expect("the refusal is kept");
            assert!(last_error.contains("`claude-fable-5[1m]`"), "{last_error}");
            assert!(last_error.contains("disappeared"), "{last_error}");
            assert_ne!(last_error, "model catalogue preflight refused this model");
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn terminal_agent_without_reply_is_counted_as_no_response_failure() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('d-silent', 'Silent child', ?1, ?1)",
                [&now],
            )?;
            conn.execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at)
                 VALUES ('u-silent', 'd-silent', 'User', 'go', ?1, 1, ?1)",
                [&now],
            )?;
            crate::db::workflows::ensure_batch_placeholder_workflow(
                conn,
                "qp-silent",
                "Silent QP",
                None,
            )?;
            conn.execute(
                "INSERT INTO workflow_runs
                 (id, workflow_id, status, started_at, run_type, batch_total)
                 VALUES ('batch-silent', 'qp:qp-silent', 'Running', ?1, 'batch', 1)",
                [&now],
            )?;
            crate::db::agent_dispatch::enqueue_for_latest_user(
                conn,
                crate::db::agent_dispatch::NewLatestUserDispatch {
                    id: "j-silent",
                    discussion_id: "d-silent",
                    dedupe_key: "message:u-silent",
                    agent_override: None,
                    chain_prompt_ids: &[],
                    batch_item: None,
                    group_id: Some("batch-silent"),
                    group_concurrency_limit: None,
                },
            )?;
            crate::db::agent_dispatch::claim(conn, "j-silent")?;
            Ok(())
        })
        .await
        .unwrap();

        let settled = db
            .with_conn(|conn| {
                persist_dispatch_settlement(
                    conn,
                    "j-silent",
                    "d-silent",
                    Some("batch-silent"),
                    crate::db::workflows::BatchChildOutcome::NoResponse,
                    Some("agent finished without a durable reply"),
                    None,
                )
            })
            .await
            .unwrap();
        assert!(settled.changed);

        db.with_conn(|conn| {
            let job = crate::db::agent_dispatch::get(conn, "j-silent")?.unwrap();
            assert_eq!(
                job.status,
                crate::db::agent_dispatch::DispatchStatus::Failed
            );
            let run = crate::db::workflows::get_run(conn, "batch-silent")?.unwrap();
            assert_eq!(run.status, crate::models::RunStatus::Failed);
            assert_eq!(run.batch_completed, 0);
            assert_eq!(run.batch_failed, 1);
            assert_eq!(run.batch_no_response, 1);
            Ok(())
        })
        .await
        .unwrap();
    }
}

#[cfg(test)]
mod refused_tool_call_notice_tests {
    use super::{persist_dispatch_settlement, refused_tool_calls_of_worker};
    use std::sync::Arc;

    const REFUSAL: &str = "Claude requested permissions to edit /wt/backend/.cargo/audit.toml which is a sensitive file.";

    async fn worker_with(
        messages: Vec<(&'static str, &'static str, &'static str)>,
    ) -> crate::db::Database {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.with_conn(move |conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('d-worker', 'Worker', ?1, ?1)",
                [&now],
            )?;
            for (order, (id, role, content)) in messages.iter().enumerate() {
                conn.execute(
                    "INSERT INTO messages
                     (id, discussion_id, role, content, timestamp, sort_order, received_at)
                     VALUES (?1, 'd-worker', ?2, ?3, ?4, ?5, ?4)",
                    rusqlite::params![id, role, content, now, order as i64],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
        db
    }

    /// KT-1541 — the reasons the worker's CLI refused are read back from its own
    /// transcript, most recent first.
    #[tokio::test]
    async fn the_refused_tool_calls_are_read_back_from_the_worker_transcript() {
        let db = worker_with(vec![
            ("u1", "User", "go"),
            (
                "a1",
                "Agent",
                "Editing the audit config.\n> ⚠ Tool call refused: Claude requested permissions to edit /wt/backend/.cargo/audit.toml which is a sensitive file.\nI cannot continue.",
            ),
        ])
        .await;
        let reasons = db
            .with_conn(|conn| refused_tool_calls_of_worker(conn, "d-worker"))
            .await
            .unwrap();
        assert_eq!(reasons, vec![REFUSAL.to_string()]);
    }

    /// A worker that writes or quotes the words (this ticket's own gotcha doc
    /// does) is not a refusal: only the line the transcript writer emits counts.
    #[tokio::test]
    async fn prose_that_quotes_the_refusal_marker_is_not_a_refusal() {
        let db = worker_with(vec![
            ("u1", "User", "go"),
            (
                "a1",
                "Agent",
                "The doc says Tool call refused: is how a refusal reads.\n  > ⚠ Tool call refused: indented, so quoted\n> Tool call refused: no warning sign\nSee `> ⚠ Tool call refused: inline` too.",
            ),
        ])
        .await;
        let reasons = db
            .with_conn(|conn| refused_tool_calls_of_worker(conn, "d-worker"))
            .await
            .unwrap();
        assert!(reasons.is_empty(), "{reasons:?}");
    }

    /// The worker's room outlives its review rounds: a refusal met before the
    /// reviewer's findings belongs to an earlier attempt, not the current one.
    #[tokio::test]
    async fn a_refusal_from_an_earlier_attempt_is_not_reported() {
        let db = worker_with(vec![
            ("u1", "User", "go"),
            (
                "a1",
                "Agent",
                "> ⚠ Tool call refused: an old refusal, since worked around",
            ),
            ("u2", "User", "Changes requested: fix the lint"),
            (
                "a2",
                "Agent",
                "Fixed.\n> ⚠ Tool call refused: the current one\n",
            ),
        ])
        .await;
        let reasons = db
            .with_conn(|conn| refused_tool_calls_of_worker(conn, "d-worker"))
            .await
            .unwrap();
        assert_eq!(reasons, vec!["the current one".to_string()]);
    }

    #[tokio::test]
    async fn nothing_is_reported_when_the_worker_was_refused_nothing() {
        let db = worker_with(vec![
            ("u1", "User", "Tool call refused: pasted by the user"),
            ("a1", "Agent", "Done, no tool was refused."),
        ])
        .await;
        let reasons = db
            .with_conn(|conn| refused_tool_calls_of_worker(conn, "d-worker"))
            .await
            .unwrap();
        assert!(reasons.is_empty(), "{reasons:?}");
    }

    /// KT-1541 — through the real settlement: a worker that ends its turn on a
    /// refused edit without delivering leaves the principal a notice that names
    /// the refusal, not just "no delivery".
    #[tokio::test]
    async fn the_ended_without_delivery_notice_includes_the_refused_tool_calls() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let execution_id = db
            .with_conn(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                for (id, title) in [("d-parent", "Parent"), ("d-child", "Child")] {
                    conn.execute(
                        "INSERT INTO discussions (id, title, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?3)",
                        rusqlite::params![id, title, now],
                    )?;
                }
                conn.execute(
                    "INSERT INTO planning_tasks
                     (id, task_number, title, status, created_at, updated_at)
                     VALUES ('t-refused', 9902, 'Refused', 'todo', ?1, ?1)",
                    [&now],
                )?;
                let actor = crate::models::PlanningActor {
                    kind: crate::models::PlanningActorKind::Backend,
                    id: Some("orchestrator".into()),
                    session_id: None,
                    source_message_id: None,
                };
                let execution = crate::db::orchestration::launch_single_task(
                    conn,
                    &crate::models::LaunchSingleTaskInput::new("t-refused", "d-parent"),
                    &actor,
                )?
                .execution;
                for status in [
                    crate::models::TaskExecutionStatus::Provisioning,
                    crate::models::TaskExecutionStatus::Working,
                ] {
                    crate::db::orchestration::transition_execution(
                        conn,
                        &execution.id,
                        status,
                        &actor,
                        serde_json::json!({}),
                    )?;
                }
                conn.execute(
                    "INSERT INTO messages
                     (id, discussion_id, role, content, timestamp, sort_order, received_at)
                     VALUES ('u-refused', 'd-child', 'User', 'go', ?1, 1, ?1)",
                    [&now],
                )?;
                conn.execute(
                    "INSERT INTO messages
                     (id, discussion_id, role, content, timestamp, sort_order, received_at)
                     VALUES ('a-refused', 'd-child', 'Agent', ?2, ?1, 2, ?1)",
                    rusqlite::params![now, format!("Editing.\n> ⚠ Tool call refused: {REFUSAL}\n")],
                )?;
                crate::db::agent_dispatch::enqueue_for_latest_user(
                    conn,
                    crate::db::agent_dispatch::NewLatestUserDispatch {
                        id: "j-refused",
                        discussion_id: "d-child",
                        dedupe_key: "message:u-refused",
                        agent_override: Some(&crate::models::AgentType::Ollama),
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: None,
                        group_concurrency_limit: None,
                    },
                )?;
                crate::db::agent_dispatch::claim(conn, "j-refused")?;
                crate::db::orchestration::attach_execution_dispatch(
                    conn,
                    &execution.id,
                    "j-refused",
                )?;
                Ok(execution.id)
            })
            .await
            .unwrap();

        let settled = db
            .with_conn(|conn| {
                persist_dispatch_settlement(
                    conn,
                    "j-refused",
                    "d-child",
                    None,
                    crate::db::workflows::BatchChildOutcome::Success,
                    None,
                    None,
                )
            })
            .await
            .unwrap();
        assert!(settled.changed);

        let notice = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT content FROM messages WHERE id = ?1 AND discussion_id = 'd-parent'",
                    [format!("orch-worker-undelivered:{execution_id}")],
                    |row| row.get::<_, String>(0),
                )?)
            })
            .await
            .unwrap();
        assert!(notice.contains("sans livraison durable"), "{notice}");
        assert!(notice.contains(REFUSAL), "{notice}");
    }
}
