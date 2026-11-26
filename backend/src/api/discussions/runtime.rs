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

pub use crate::api::orchestration::RUNTIME_UNAVAILABLE_MAX_DEFERRALS;

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

pub struct DispatchSettlement {
    changed: bool,
    batch_run: Option<crate::models::WorkflowRun>,
}

use crate::api::delivery_publication::refused_tool_calls_of_worker;

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

/// How a worker's dispatch ended without a delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UndeliveredCause {
    Completed,
    Failed,
    /// KT-1782 — cancelled before it ended: a Stop, or any other cancel.
    Stopped,
    /// KT-2037 — stopped by Kronn at the run time limit. Callers pass `Failed`:
    /// the journal of the dispatch tells the two apart.
    TimedOut,
}

/// The execution a terminal dispatch leaves without a delivery is parked as
/// `Interrupted` and the principal is told once per dispatch
/// (`orch-worker-undelivered:<exec>:<dispatch>`, `orch-worker-stopped:…` for a
/// stop, `orch-worker-timed-out:…` for a run stopped at its time limit).
/// A no-op when the dispatch belongs to no `Working` execution.
///
/// KT-1758 — kept apart from the settlement so a link that takes the work over
/// can hold it back: telling the principal "reassign or cancel" about an
/// execution Kronn is itself reassigning is contradictory.
pub(super) fn interrupt_undelivered_with_notice(
    transaction: &rusqlite::Connection,
    job_id: &str,
    discussion_id: &str,
    cause: UndeliveredCause,
) -> anyhow::Result<()> {
    // KT-2037 — a failure that follows a limit stop of this very run is Kronn's
    // stop, not the worker's failure. An unreadable journal keeps the failure.
    let limit_stop = match cause {
        UndeliveredCause::Failed | UndeliveredCause::TimedOut => {
            crate::api::worker_time_limit::limit_stop_of_dispatch(transaction, job_id)
                .unwrap_or_else(|error| {
                    tracing::warn!(dispatch_job_id = %job_id, "time-limit stop unreadable: {error:#}");
                    None
                })
        }
        UndeliveredCause::Completed | UndeliveredCause::Stopped => None,
    };
    let cause = match (cause, &limit_stop) {
        (UndeliveredCause::Failed | UndeliveredCause::TimedOut, Some(_)) => {
            UndeliveredCause::TimedOut
        }
        (UndeliveredCause::TimedOut, None) => UndeliveredCause::Failed,
        (cause, _) => cause,
    };
    let interruption_reason = match cause {
        UndeliveredCause::Completed => "worker_completed_without_delivery",
        UndeliveredCause::Failed => "worker_failed_without_delivery",
        UndeliveredCause::Stopped => "worker_stopped_without_delivery",
        UndeliveredCause::TimedOut => crate::db::orchestration::WORKER_TIMED_OUT_REASON,
    };
    let actor = crate::models::PlanningActor {
        kind: crate::models::PlanningActorKind::Backend,
        id: Some("orchestrator".into()),
        session_id: None,
        source_message_id: None,
        token_declared: false,
    };
    if let Some(interrupted) =
        crate::db::orchestration::interrupt_undelivered_execution_for_dispatch(
            transaction,
            job_id,
            interruption_reason,
            &actor,
        )?
    {
        let parent =
            crate::db::discussions::get_discussion(transaction, &interrupted.parent_discussion_id)?
                .context("parent discussion vanished before undelivered-worker notice")?;
        let target = crate::db::orchestration::principal_notice_target(
            transaction,
            &interrupted.execution_id,
            parent.agent,
        )?;
        // KT-2108 — every cause is told per dispatch: a resumed or reassigned
        // worker can end without delivering again, and the principal must be woken.
        let notice_kind = match cause {
            UndeliveredCause::Stopped => "orch-worker-stopped",
            UndeliveredCause::TimedOut => "orch-worker-timed-out",
            UndeliveredCause::Completed | UndeliveredCause::Failed => "orch-worker-undelivered",
        };
        let message_id = format!("{notice_kind}:{}:{job_id}", interrupted.execution_id);
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
            let refused = refused_tool_calls_sentence(transaction, discussion_id)?;
            let refusal = interrupted
                .delivery_refusal
                .as_deref()
                .map(|detail| format!(" Cause : la livraison du worker a été refusée — {detail}"))
                .unwrap_or_default();
            let (task_id, execution_id) = (&interrupted.task_id, &interrupted.execution_id);
            let content = match (cause, &limit_stop) {
                (UndeliveredCause::TimedOut, Some(stop)) => format!(
                    "**Worker arrêté par la limite de temps, sans livraison**\n\nKronn a arrêté le dispatch `{job_id}` de la tâche `{task_id}` après {elapsed} min : il a atteint la durée maximale d'une exécution agent ({limit} min, Config > Serveur). Le worker n'a déclaré aucun échec et n'avait pas encore livré. L'exécution `{execution_id}` est maintenant `Interrupted`. {kept}{refused}{refusal} Reprends-la (`task_exec_resume`) ou réaffecte-la (`task_exec_reassign`, dont le champ `reason` est lu par le worker) : le worker repart avec la limite de son agent ({limit} min s'il reste le même), donc indique-lui quelle commande longue ne pas relancer. Sinon annule-la (`task_exec_cancel`).",
                    elapsed = stop.elapsed.as_secs() / 60,
                    limit = stop.limit.as_secs() / 60,
                    kept = stop.kept(true),
                ),
                _ => {
                    // KT-2021 — given up beside a previous session: another
                    // worker would start beside the same one.
                    let beside_a_previous_session =
                        crate::db::agent_dispatch::get(transaction, job_id)?
                            .and_then(|job| job.last_error)
                            .is_some_and(|error| error.contains(PREVIOUS_WORKER_SESSION_ALIVE));
                    let (title, ended, instruction) = match cause {
                        UndeliveredCause::Failed | UndeliveredCause::TimedOut
                            if beside_a_previous_session =>
                        {
                            (
                                "Worker non démarré, sans livraison durable",
                                "n'a pas démarré et n'a donc fait aucun appel à",
                                "Une session précédente de ce worker n'a pas pu être arrêtée : l'avis « Worker non démarré » de cette discussion dit laquelle. Arrêtez ses processus, puis reprenez l'exécution (`task_exec_resume`) ; la réaffecter ne change rien.",
                            )
                        }
                        UndeliveredCause::Failed | UndeliveredCause::TimedOut => (
                            "Worker échoué sans livraison durable",
                            "s'est terminé sans appel valide à",
                            "Examine l'erreur du dispatch dans la sous-discussion, puis réaffecte ou annule explicitement l'exécution.",
                        ),
                        UndeliveredCause::Completed => (
                            "Worker terminé sans livraison durable",
                            "s'est terminé sans appel valide à",
                            "Relis le bilan du worker dans la sous-discussion, puis reprends l'exécution une fois (`task_exec_resume`, le worktree est conservé), ou réaffecte / annule-la explicitement.",
                        ),
                        UndeliveredCause::Stopped => (
                            "Worker arrêté sans livraison durable",
                            "a été arrêté avant tout appel valide à",
                            "Reprends l'exécution une fois (`task_exec_resume`, le worktree est conservé), réaffecte-la (`task_exec_reassign`) ou annule-la (`task_exec_cancel`).",
                        ),
                    };
                    format!(
                        "**{title}**\n\nLe dispatch `{job_id}` de la tâche `{task_id}` {ended} `task_exec_deliver`. L'exécution `{execution_id}` est maintenant `Interrupted` au lieu de rester faussement `Working`.{refused}{refusal} {instruction}"
                    )
                }
            };
            let message = crate::api::orchestration::orchestrator_message(message_id, content);
            crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
                transaction,
                &interrupted.parent_discussion_id,
                &message,
                &[target],
                &[],
                None,
            )?;
        }
    }
    Ok(())
}

/// KT-1782 — the parking that goes with a Stop, inside the stop's transaction.
/// The stop is the owner's action and commits whatever happens here: a failure
/// undoes the parking alone and the end of the stream parks it then.
pub(super) fn park_stopped_worker(conn: &rusqlite::Connection, job_id: &str, discussion_id: &str) {
    let parked = conn
        .execute_batch("SAVEPOINT park_stopped_worker")
        .map_err(anyhow::Error::from)
        .and_then(|()| {
            let parked = interrupt_undelivered_with_notice(
                conn,
                job_id,
                discussion_id,
                UndeliveredCause::Stopped,
            )
            .and_then(|()| {
                conn.execute_batch("RELEASE park_stopped_worker")
                    .map_err(anyhow::Error::from)
            });
            if parked.is_err() {
                let _ = conn
                    .execute_batch("ROLLBACK TO park_stopped_worker; RELEASE park_stopped_worker");
            }
            parked
        });
    if let Err(error) = parked {
        tracing::warn!(
            dispatch_job_id = %job_id,
            "parking the execution of a stopped worker failed, left to the end of its stream: {error:#}"
        );
    }
}

/// KT-1782 — the room Stop names no dispatch: parks the room's `Working`
/// execution when its dispatch is a cancelled one. A `Failed` dispatch is being
/// handed to a fallback (KT-1758) and is left alone.
pub(super) fn park_stopped_worker_of_room(conn: &rusqlite::Connection, discussion_id: &str) {
    let stopped = crate::db::orchestration::working_worker_dispatch_of_room(conn, discussion_id)
        .and_then(|dispatch_id| {
            let Some(dispatch_id) = dispatch_id else {
                return Ok(None);
            };
            let cancelled =
                crate::db::agent_dispatch::get(conn, &dispatch_id)?.is_some_and(|job| {
                    job.status == crate::db::agent_dispatch::DispatchStatus::Cancelled
                });
            Ok(cancelled.then_some(dispatch_id))
        });
    match stopped {
        Ok(Some(dispatch_id)) => park_stopped_worker(conn, &dispatch_id, discussion_id),
        Ok(None) => {}
        Err(error) => tracing::warn!(
            discussion_id = %discussion_id,
            "finding the worker a room stop left without a dispatch failed: {error:#}"
        ),
    }
}

/// KT-1758 — the interruption a settlement held back (see
/// [`persist_dispatch_settlement_handing_over`]), once no link could take the
/// work over: the execution is `Interrupted` and the principal is told.
///
/// The settlement left the execution `Working` with a terminal dispatch, a state
/// nothing else is going to resolve soon, so this does not give up: the
/// interruption and the notice are tried twice, and if the notice is what keeps
/// failing the execution is still parked, without it (said in the log).
pub fn interrupt_undelivered_after_failure(
    conn: &rusqlite::Connection,
    job_id: &str,
    discussion_id: &str,
) -> anyhow::Result<()> {
    let attempt = || -> anyhow::Result<()> {
        let transaction = conn.unchecked_transaction()?;
        interrupt_undelivered_with_notice(
            &transaction,
            job_id,
            discussion_id,
            UndeliveredCause::Failed,
        )?;
        transaction.commit()?;
        Ok(())
    };
    let Err(first) = attempt() else {
        return Ok(());
    };
    tracing::warn!(dispatch_job_id = %job_id, "interrupting an undelivered worker failed, retrying once: {first}");
    let Err(second) = attempt() else {
        return Ok(());
    };
    tracing::error!(
        dispatch_job_id = %job_id,
        "the undelivered-worker notice could not be posted ({second}); parking the execution without it"
    );
    let actor = crate::models::PlanningActor {
        kind: crate::models::PlanningActorKind::Backend,
        id: Some("orchestrator".into()),
        session_id: None,
        source_message_id: None,
        token_declared: false,
    };
    crate::db::orchestration::interrupt_undelivered_execution_for_dispatch(
        conn,
        job_id,
        "worker_failed_without_delivery",
        &actor,
    )
    .map_err(|error| anyhow::anyhow!("the execution could not be parked either: {error}"))?;
    Ok(())
}

/// Persist a dispatch terminal state and its parent batch progress as one
/// atomic unit. If either write fails, the job remains runnable and recovery
/// can retry without leaving a batch permanently stuck at n-1/N.
pub fn persist_dispatch_settlement(
    conn: &rusqlite::Connection,
    job_id: &str,
    discussion_id: &str,
    group_id: Option<&str>,
    child_outcome: crate::db::workflows::BatchChildOutcome,
    error: Option<&str>,
    failure_kind: Option<&str>,
) -> anyhow::Result<DispatchSettlement> {
    persist_dispatch_settlement_with(
        conn,
        job_id,
        discussion_id,
        group_id,
        child_outcome,
        error,
        failure_kind,
        true,
    )
}

/// KT-1758 — [`persist_dispatch_settlement`] for a failure a fallback is about
/// to take over: the execution stays `Working` (the reassignment moves it from
/// there, as a quota fallback does) and the principal is not told to reassign
/// what Kronn is reassigning. The caller owes
/// [`interrupt_undelivered_after_failure`] when no link takes the work over.
pub fn persist_dispatch_settlement_handing_over(
    conn: &rusqlite::Connection,
    job_id: &str,
    discussion_id: &str,
    group_id: Option<&str>,
    child_outcome: crate::db::workflows::BatchChildOutcome,
    error: Option<&str>,
    failure_kind: Option<&str>,
) -> anyhow::Result<DispatchSettlement> {
    persist_dispatch_settlement_with(
        conn,
        job_id,
        discussion_id,
        group_id,
        child_outcome,
        error,
        failure_kind,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn persist_dispatch_settlement_with(
    conn: &rusqlite::Connection,
    job_id: &str,
    discussion_id: &str,
    group_id: Option<&str>,
    child_outcome: crate::db::workflows::BatchChildOutcome,
    error: Option<&str>,
    failure_kind: Option<&str>,
    interrupt_undelivered: bool,
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
    if changed && failure_kind != Some("quota_exhausted") && interrupt_undelivered {
        let cause = if error.is_some() {
            UndeliveredCause::Failed
        } else {
            UndeliveredCause::Completed
        };
        interrupt_undelivered_with_notice(&transaction, job_id, discussion_id, cause)?;
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
pub async fn stream_dispatch_job(
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

pub async fn stream_claimed_dispatch_job(
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
    match hold_worker_launch_beside_a_previous_session(&state, &job).await {
        None => {}
        Some(true) => {
            handoff_guard.disarm();
            return None;
        }
        // The hold could not be written: the armed guard hands the claim back
        // to the queue. Never a launch beside the session just found alive.
        Some(false) => return None,
    }
    // KT-1518 — an agent on cooldown is skipped: the turn starts on its fallback.
    let job = super::agent_fallback_start::start_on_fallback(&state, job).await;

    let power_lease = crate::core::power_guard::acquire();
    let target_tier = {
        let trigger_message_id = job.trigger_message_id.clone();
        let agent_override = job.agent_override.clone();
        let connection_id = job.connection_id.clone();
        let job_id = job.id.clone();
        state
            .db
            .with_read_conn(move |conn| {
                // KT-1518 — a turn that moved to a fallback keeps its tier.
                if let Some(pin) = crate::core::agent_fallback::get_pin(conn, &job_id)? {
                    return Ok(Some(pin.tier));
                }
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
        crate::api::orchestration::stop_worker_dispatch_processes(&state.db, &job.id).await;
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
            settle_cancelled_worker(&state, &job).await;
            return;
        }
        let reported_success = match outcome {
            AgentExecutionOutcome::Finished { success } => success,
            AgentExecutionOutcome::RuntimeUnavailable { reason } => {
                defer_or_stop_runtime_unavailable(&state, &job, &reason).await;
                return;
            }
            AgentExecutionOutcome::PreflightFailed { diagnostic } => {
                settle_preflight_failure(&state, &job, &diagnostic, None).await;
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

/// The end of a stream whose dispatch was cancelled. Whoever cancelled it
/// settled the job and its parent batch already, so nothing is counted here
/// (it would double-count the child); waiters are only woken.
///
/// KT-1782 — a cancel that did not come through a stop handler leaves its
/// execution `Working` with no worker: it is parked here. After a stop, a
/// reassignment or a cancel of the execution this finds nothing to park.
async fn settle_cancelled_worker(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
) {
    let (job_id, discussion_id) = (job.id.clone(), job.discussion_id.clone());
    let parked = state
        .db
        .with_conn(move |conn| {
            let transaction = conn.unchecked_transaction()?;
            interrupt_undelivered_with_notice(
                &transaction,
                &job_id,
                &discussion_id,
                UndeliveredCause::Stopped,
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await;
    if let Err(error) = parked {
        tracing::warn!(
            dispatch_job_id = %job.id,
            "parking the execution of a cancelled worker dispatch failed: {error}"
        );
    }
    state.agent_dispatch_notify.notify_waiters();
}

/// A launch the engine refused before anything ran (`PreflightFailed`).
///
/// KT-1758 — a fallback worker refused for good as a task worker moves on to the
/// next link instead of failing silently; any other refusal is the dispatch's
/// failure, made visible where it is read. `available`: the installed agents,
/// detected when `None`.
pub async fn settle_preflight_failure(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    diagnostic: &str,
    available: Option<Vec<crate::models::AgentType>>,
) {
    if is_permanent_task_worker_refusal(diagnostic)
        && settle_worker_launch_refusal_among(state, job, diagnostic, available).await
    {
        return;
    }
    fail_dispatch_job(state, job, diagnostic).await;
    ensure_failure_visible(state, job, diagnostic).await;
}

pub(super) fn dispatch_target_tier(
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
pub async fn try_quota_provider_fallback_among(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    announced_reset: Option<chrono::DateTime<chrono::Utc>>,
    available_agents: Vec<crate::models::AgentType>,
) -> anyhow::Result<Option<(crate::models::AgentType, String)>> {
    provider_fallback_among(state, job, announced_reset, available_agents, false).await
}

/// The campaign's provider fallback. `launch_refused`: the provider did not
/// refuse the quota, it was refused as this execution's worker (KT-1758): it
/// is excluded from this execution's chain without being written down as a
/// quota refusal for everyone.
async fn provider_fallback_among(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    announced_reset: Option<chrono::DateTime<chrono::Utc>>,
    available_agents: Vec<crate::models::AgentType>,
    launch_refused: bool,
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
            if launch_refused {
                // Nothing announced and nothing to wait for: this execution
                // never offers that provider to itself again.
                crate::db::orchestration::record_execution_quota_refusal(
                    conn,
                    &execution.id,
                    &crate::db::orchestration::QuotaRefusal {
                        provider: crate::db::orchestration::agent_type_to_db(&current),
                        reset_at: None,
                    },
                )?;
            } else {
                crate::db::orchestration::note_provider_quota_refusal(
                    conn,
                    &crate::db::orchestration::agent_type_to_db(&current),
                    announced_reset,
                )?;
            }
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
            // KT-2000 — the chain's shell rule (KT-1991), from the same
            // capability: a generic worker builds, tests and commits, so it is
            // never moved to an agent without a shell, named or merely allowed.
            let needs_shell = execution.worker_scope.is_none();
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
                // KT-1749 — never an alternative the launch would refuse
                // (native ACP route, typed-delivery capability).
                if crate::core::agent_fallback::task_worker_launch_refusal(candidate).is_some() {
                    continue;
                }
                if needs_shell && !crate::core::agent_fallback::capabilities(candidate).shell {
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
    // An HTTP alternative is a `discussion_agent`, a host CLI an `agent`.
    let mut target = crate::core::agent_fallback::task_worker_target(&alternative);
    target.tier = tier;
    let selection = crate::models::CampaignWorkerSelection {
        target,
        model: None,
        profile_id,
    };
    // KT-1758 — only while the execution still runs the dispatch that failed:
    // the quota return may have moved it since, and the move is then its own.
    let view = crate::api::orchestration::reassign_native_execution_for_dispatch(
        state,
        &execution_id,
        selection,
        if launch_refused {
            "automatic fallback: the previous worker agent was refused at launch and the campaign opted into a provider fallback"
        } else {
            "automatic quota fallback: the campaign opted into a provider fallback on quota exhaustion"
        },
        crate::api::orchestration::ReassignCause::QuotaFallback {
            reset_at: announced_reset,
        },
        Some(&job.id),
    )
    .await?;
    state.agent_dispatch_notify.notify_one();
    Ok(Some((alternative, view.execution.parent_discussion_id)))
}

/// The provider's own words in a failed turn's reply (KT-1517).
///
/// The reply leads with Kronn's hint and folds the raw output into a
/// `kronn:context` block, and a Codex failure can echo the whole prompt before the
/// refusal. So the line that carries the quota signal is the message worth
/// quoting; without one, the start of the raw output.
pub fn provider_quota_message(response: &str) -> String {
    const CONTEXT_CLOSE: &str = "<!-- /kronn:context -->";
    let raw = response
        .find("<!-- kronn:context")
        .and_then(|open| {
            let body = &response[open..];
            let start = body.find("-->")? + "-->".len();
            let end = body.find(CONTEXT_CLOSE).unwrap_or(body.len());
            body.get(start..end.max(start))
        })
        .unwrap_or(response);
    let line = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && super::is_hard_quota_exhausted(line));
    line.unwrap_or_else(|| raw.trim())
        .trim_start_matches("[Agent provider error]")
        .trim()
        .chars()
        .take(800)
        .collect()
}

async fn finish_dispatch_turn(
    state: &AppState,
    job: crate::db::agent_dispatch::AgentDispatchJob,
    response: String,
    execution_succeeded: bool,
) {
    // KT-2037 — a run Kronn stopped at its time limit is settled as that, before
    // its prose can be read as a provider refusal and moved to a fallback.
    if !execution_succeeded {
        let dispatch_id = job.id.clone();
        let limit_stop = state
            .db
            .with_read_conn(move |conn| {
                crate::api::worker_time_limit::limit_stop_of_dispatch(conn, &dispatch_id)
            })
            .await
            .ok()
            .flatten();
        if let Some(stop) = limit_stop {
            fail_dispatch_job(
                state,
                &job,
                &crate::api::worker_time_limit::stop_diagnostic(&stop),
            )
            .await;
            return;
        }
    }
    // KT-1518 — a provider that could not serve the turn (rate limit, quota,
    // outage past its retries, not authenticated) hands it to the agent's
    // fallback chain; a failure of the work itself never does.
    if !execution_succeeded
        && super::agent_fallback::intercept_failed_turn(state, &job, &response).await
    {
        return;
    }
    if !execution_succeeded && super::is_hard_quota_exhausted(&response) {
        let detail = provider_quota_message(&response);
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

    // KT-1743 — a provider edge/auth error that is the network's doing (a
    // Cloudflare challenge, a 403 from the API edge) is waited out on the same
    // agent, within TRANSIENT_UPSTREAM_BACKOFF_SECONDS. A task worker would
    // otherwise end Interrupted on a blip that clears in minutes.
    if !execution_succeeded && super::is_transient_upstream_auth_error(&response) {
        if let Some(delay_seconds) = transient_upstream_delay(job.turn_attempts) {
            if retry_after_transient_upstream(state, &job, &response, delay_seconds).await {
                return;
            }
        }
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

/// KT-1743 — waits between the automatic redispatches of a turn that ended on a
/// transient upstream error: 1, 3 then 6 minutes, ten in all. After the third
/// retry the failure is real as far as Kronn can tell and settles as before.
pub const TRANSIENT_UPSTREAM_BACKOFF_SECONDS: [i64; 3] = [60, 180, 360];

/// The wait before the next redispatch, given how many times the dispatch has
/// been claimed (`turn_attempts`, 1 on the first run). `None`: retries spent.
pub fn transient_upstream_delay(turn_attempts: u32) -> Option<i64> {
    let retries_done = usize::try_from(turn_attempts.saturating_sub(1)).ok()?;
    TRANSIENT_UPSTREAM_BACKOFF_SECONDS
        .get(retries_done)
        .copied()
}

/// Re-queues the dispatch after `delay_seconds`, drops the error reply and
/// tells the room (the worker room, for a task worker) what happens next.
/// `true`: the turn is queued again and the caller has nothing more to settle.
async fn retry_after_transient_upstream(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    response: &str,
    delay_seconds: i64,
) -> bool {
    let attempt = job.turn_attempts;
    let total = TRANSIENT_UPSTREAM_BACKOFF_SECONDS.len();
    let cause: String = response.trim().chars().take(300).collect();
    tracing::warn!(
        "Discussion {} hit a transient upstream error — retry {attempt}/{total} in {delay_seconds}s",
        job.discussion_id
    );
    let retry_id = job.id.clone();
    let discussion_id = job.discussion_id.clone();
    let agent_override = job.agent_override.clone();
    let trigger_message_id = job.trigger_message_id.clone();
    let retried = state
        .db
        .with_conn(move |conn| {
            let transaction = conn.unchecked_transaction()?;
            let queued = crate::db::agent_dispatch::retry_after(
                &transaction,
                &retry_id,
                delay_seconds,
                "transient_upstream_error",
            )?;
            anyhow::ensure!(queued, "upstream-error dispatch is no longer running");
            crate::db::discussions::delete_dispatch_reply_messages(
                &transaction,
                &discussion_id,
                &retry_id,
            )?;
            if let Some(discussion) = crate::db::discussions::get_discussion(&transaction, &discussion_id)? {
                let agent = agent_override.unwrap_or_else(|| discussion.agent.clone());
                let notice = super::agent_fallback::system_notice(
                    format!(
                        "**Transient provider error — automatic retry {attempt}/{total}.** \
                         The provider refused the turn with a network/edge error rather than a lost \
                         login: `{cause}`. Kronn redispatches it in {delay_seconds}s on the same \
                         agent; if it is still refused after {total} retries the turn ends as failed \
                         with this cause."
                    ),
                    &agent,
                    discussion.tier,
                    &trigger_message_id,
                );
                crate::db::discussions::insert_message(&transaction, &discussion_id, &notice)?;
            }
            crate::db::discussions::set_awaiting_agent(&transaction, &discussion_id, true)?;
            transaction.commit()?;
            Ok(())
        })
        .await;
    match retried {
        Ok(()) => {
            state.agent_dispatch_notify.notify_one();
            true
        }
        Err(error) => {
            tracing::warn!(dispatch_job_id = %job.id, "transient upstream retry failed: {error}");
            false
        }
    }
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
    let transient_upstream = super::is_transient_upstream_auth_error(response);
    let failure_kind = if response.trim().is_empty() {
        "no_visible_output"
    } else if transient_upstream {
        // KT-1743 — reached only once the automatic retries are spent.
        "provider_upstream_error"
    } else {
        "worker_reported_failure"
    };
    if is_task_worker {
        let cause = if transient_upstream {
            format!(
                " Cause: {} (still refused after {} automatic retries over ten minutes).",
                response.trim().chars().take(300).collect::<String>(),
                TRANSIENT_UPSTREAM_BACKOFF_SECONDS.len()
            )
        } else {
            String::new()
        };
        return format!(
            "task-worker startup/completion failed: phase=worker_completion; \
             failure_kind={failure_kind}; exit=unsuccessful.{cause} The worker did not submit a durable \
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
                // KT-1518 — no campaign opt-in applied: the agent's own chain.
                match super::agent_fallback::worker_after_quota(state, job, error).await {
                    Ok(true) => {
                        state.agent_dispatch_notify.notify_waiters();
                        return;
                    }
                    Ok(false) => {}
                    Err(chain_error) => {
                        tracing::warn!(dispatch_job_id = %job.id, "agent fallback chain failed: {chain_error}");
                    }
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
                // KT-1517 — the room that ran the turn gets the System card: the
                // provider's message and what to do next. Without it the agent just
                // stops publishing and the room looks idle.
                record_quota_card(state, job, error, announced_reset).await;
                // A task worker's parent room also gets the orchestrator's escalation.
                if target_discussion != job.discussion_id {
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
                            crate::db::discussions::insert_message(
                                conn,
                                &target_discussion,
                                &message,
                            )?;
                            Ok(())
                        })
                        .await
                    {
                        tracing::warn!(dispatch_job_id = %job.id, "quota notice could not be persisted: {db_error}");
                    }
                }
            }
        }
    }
    state.agent_dispatch_notify.notify_waiters();
}

/// KT-1517 — the System card a quota-refused turn leaves in its own room: the
/// provider's message, when it resets if it said, and the way out. It is linked to
/// the dispatch, so the room's Retry runs the same turn again.
async fn record_quota_card(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    error: &str,
    announced_reset: Option<chrono::DateTime<chrono::Utc>>,
) {
    let lookup = job.clone();
    let loaded = state
        .db
        .with_read_conn(move |conn| {
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
        Err(db_error) => {
            tracing::warn!(dispatch_job_id = %job.id, "unable to read the room for the quota card: {db_error}");
            return;
        }
    };
    let agent = job.agent_override.clone().unwrap_or(agent);
    let message = error.strip_prefix("quota_exhausted: ").unwrap_or(error);
    let content = super::start_refusal::quota_exhausted_card(
        &language,
        &format!("{agent:?}"),
        message,
        announced_reset,
        tier,
        Some(&job.id),
    );
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

pub use crate::api::orchestration::{previous_session_of_hold, PREVIOUS_WORKER_SESSION_ALIVE};

/// KT-2021 — a worker never starts beside a previous session of its execution
/// (a resume at boot, `task_exec_resume`, a reassignment, a fallback, a watchdog
/// requeue). A previous launch still streaming is cancelled; one without a
/// stream is stopped here. While either may still run the launch is deferred,
/// then given up with the reason in both rooms (KT-1758).
///
/// `None`: nothing holds the launch. `Some(written)`: a previous session may
/// still run, and whether the deferral (or the give-up) was written.
async fn hold_worker_launch_beside_a_previous_session(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
) -> Option<bool> {
    hold_worker_launch_beside_a_previous_session_with(
        state,
        job,
        crate::api::orchestration::kill_recorded_worker_group,
        crate::core::dispatch_reaper::reap_dispatch,
    )
    .await
}

/// [`hold_worker_launch_beside_a_previous_session`] with the stop injected.
async fn hold_worker_launch_beside_a_previous_session_with<K, R>(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    kill: K,
    reap: R,
) -> Option<bool>
where
    K: Fn(i64, Option<&str>) -> crate::core::agent_orphans::OrphanOutcome + Clone + Send + 'static,
    R: Fn(&str) -> crate::core::dispatch_reaper::ReapReport + Clone + Send + 'static,
{
    let dispatch =
        previous_worker_session_alive_with(state, &job.id, &job.discussion_id, kill, reap).await?;
    Some(
        defer_or_stop_runtime_unavailable(state, job, &previous_session_alive_reason(&dispatch))
            .await,
    )
}

/// The reason of a launch held beside the session of `dispatch`. It names no
/// pid: identical reasons are what the deferral counts.
pub(super) fn previous_session_alive_reason(dispatch: &str) -> String {
    format!("{PREVIOUS_WORKER_SESSION_ALIVE}: dispatch {dispatch}")
}

/// KT-2021 — the dispatch of a previous session that may still run beside the
/// worker `dispatch_job_id` is about to launch in `room`. Each one is ended
/// first: a live stream is cancelled, anything else is stopped.
pub(super) async fn previous_worker_session_alive(
    state: &AppState,
    dispatch_job_id: &str,
    room: &str,
) -> Option<String> {
    previous_worker_session_alive_with(
        state,
        dispatch_job_id,
        room,
        crate::api::orchestration::kill_recorded_worker_group,
        crate::core::dispatch_reaper::reap_dispatch,
    )
    .await
}

/// [`previous_worker_session_alive`] with the stop injected.
async fn previous_worker_session_alive_with<K, R>(
    state: &AppState,
    dispatch_job_id: &str,
    room: &str,
    kill: K,
    reap: R,
) -> Option<String>
where
    K: Fn(i64, Option<&str>) -> crate::core::agent_orphans::OrphanOutcome + Clone + Send + 'static,
    R: Fn(&str) -> crate::core::dispatch_reaper::ReapReport + Clone + Send + 'static,
{
    let (lookup, worker_room) = (dispatch_job_id.to_string(), room.to_string());
    let previous = state
        .db
        .with_read_conn(move |conn| {
            let Some(execution) =
                crate::db::orchestration::get_execution_for_dispatch(conn, &lookup)?
            else {
                return Ok(Vec::new());
            };
            // Only the worker's own launch: an execution can point at a dispatch
            // of another room, which starts no worker and must cancel none.
            if execution.sub_discussion_id.as_deref() != Some(worker_room.as_str()) {
                return Ok(Vec::new());
            }
            let instance = crate::core::dispatch_reaper::instance_id();
            Ok(
                crate::db::worker_processes::unsettled_for_execution(conn, &execution.id)?
                    .into_iter()
                    .filter(|row| row.instance_id == instance)
                    .map(|row| row.dispatch_job_id)
                    .collect::<Vec<String>>(),
            )
        })
        .await;
    let previous = match previous {
        Ok(previous) => previous,
        Err(error) => {
            tracing::warn!(
                dispatch_job_id,
                "previous worker sessions unreadable: {error}"
            );
            return None;
        }
    };
    let mut alive: Option<String> = None;
    for dispatch in previous {
        // A token under the launching dispatch's own id is an earlier run of it
        // closing, and the key its new run registers under: never cancelled here.
        let streaming = if dispatch == dispatch_job_id {
            None
        } else {
            state
                .cancel_registry
                .lock()
                .ok()
                .and_then(|registry| registry.get(&dispatch).cloned())
        };
        let may_still_run = match streaming {
            // Its own stream ends on the cancel, then stops what it started.
            Some(token) => {
                token.cancel();
                true
            }
            None => crate::api::orchestration::stop_worker_dispatch_processes_with(
                &state.db,
                &dispatch,
                kill.clone(),
                reap.clone(),
            )
            .await
            .may_still_run(),
        };
        if may_still_run && alive.is_none() {
            alive = Some(dispatch);
        }
    }
    alive
}

/// KT-1758 — a task-worker refusal that waiting cannot change: the launch path
/// refuses the agent as this kind of worker (a native ACP agent carries neither
/// the delivery context nor the permission scope).
pub fn is_permanent_task_worker_refusal(reason: &str) -> bool {
    reason.contains(crate::agents::runner::TASK_WORKER_REFUSED_PREFIX)
}

/// A binary that is not there. For an ordinary room it can appear between two
/// tries (the established deferral); for a fallback worker it is a link that
/// cannot run, and the next one is worth more than waiting.
fn is_missing_binary(reason: &str) -> bool {
    reason.starts_with("Binary '") && reason.ends_with("not found")
}

/// The task worker a dispatch belongs to, when it is the execution's current one.
struct WorkerLaunch {
    execution_id: String,
    parent_room: String,
    /// The agent the execution runs on, in the `agent_type_to_db` form.
    agent: String,
    /// A quota or chain fallback moved it here: it has somewhere to go back to.
    is_fallback: bool,
}

fn worker_launch_context(
    conn: &rusqlite::Connection,
    dispatch_id: &str,
) -> anyhow::Result<Option<WorkerLaunch>> {
    let Some(execution) = crate::db::orchestration::get_execution_for_dispatch(conn, dispatch_id)?
    else {
        return Ok(None);
    };
    if execution.status.is_terminal() {
        return Ok(None);
    }
    let agent = execution.worker_agent_type.clone().unwrap_or_default();
    // "Fallback" is about the CURRENT worker: the one a fallback chose. The
    // origins outlive the move (the chain's is never cleared by a return), so
    // an execution back on the worker it started on is not one — a transient
    // failure there must not make it leave for good.
    let is_fallback = crate::db::orchestration::quota_origin(conn, &execution.id)?
        .is_some_and(|origin| origin.provider != agent)
        || crate::core::agent_fallback::get_execution_origin(conn, &execution.id)?
            .is_some_and(|origin| crate::db::orchestration::agent_type_to_db(&origin) != agent);
    Ok(Some(WorkerLaunch {
        agent,
        parent_room: execution.parent_discussion_id,
        execution_id: execution.id,
        is_fallback,
    }))
}

/// KT-1758 — a worker agent that cannot start, said once and settled. The
/// dispatch ends `Failed` with the reason; a fallback then asks for the next
/// link (the campaign's own opt-in first, then the agent's chain); and when
/// nothing can take the work, both rooms are told why, in their language, and
/// nothing is retried behind anyone's back.
///
/// Only an execution a fallback moved is handled here: the caller keeps its own
/// settlement for a worker that was never one. Returns whether the dispatch was
/// settled here. `available`: the installed agents, detected when `None`.
pub async fn settle_worker_launch_refusal_among(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    reason: &str,
    available: Option<Vec<crate::models::AgentType>>,
) -> bool {
    settle_worker_launch_refusal(state, job, reason, available, true, LaunchStop::Refused).await
}

/// Why a worker's launch is given up on, which is what its notices say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchStop {
    /// The launch path refuses this agent as this kind of worker (or its binary
    /// is not there for a fallback): waiting does not change that.
    Refused,
    /// A cause that can change was met `times` times in a row: it is the
    /// retries that ran out, not a refusal.
    Unavailable { times: u32 },
}

impl LaunchStop {
    fn failure_kind(self) -> &'static str {
        match self {
            Self::Refused => "launch_refused",
            Self::Unavailable { .. } => "launch_unavailable",
        }
    }

    /// The durable `last_error` of the failed dispatch.
    fn error(self, reason: &str) -> String {
        match self {
            Self::Refused => format!("launch_refused: {reason}"),
            Self::Unavailable { times } => format!(
                "launch_unavailable: unavailable {times} times in a row ({reason}), {RUNTIME_UNAVAILABLE_RETRY_DELAY_SECONDS} s apart"
            ),
        }
    }

    /// The reason as the rooms quote it.
    fn detail(self, reason: &str) -> String {
        match self {
            Self::Refused => reason.to_string(),
            Self::Unavailable { times } => {
                format!("unavailable {times} times in a row ({reason})")
            }
        }
    }
}

/// The worker kinds worth suggesting for a reassignment, never the agent that
/// just failed (`agent` in the `agent_type_to_db` form).
fn compatible_workers_hint(agent: &str, french: bool) -> String {
    [
        ("ClaudeCode", "Claude Code", "Claude Code"),
        ("Codex", "Codex", "Codex"),
        ("", "un modèle HTTP", "an HTTP model"),
        ("", "une session CLI jointe", "a joined CLI session"),
    ]
    .iter()
    .filter(|(key, _, _)| key.is_empty() || *key != agent)
    .map(|(_, fr, en)| if french { *fr } else { *en })
    .collect::<Vec<_>>()
    .join(", ")
}

/// KT-2021 — the pids the latest stop of `previous` left alive, read from the
/// execution's journal: the counted reason never carries them.
async fn previous_session_survivors(
    state: &AppState,
    execution_id: &str,
    previous: &str,
) -> Vec<u64> {
    let (execution_id, previous) = (execution_id.to_string(), previous.to_string());
    let survivors = state
        .db
        .with_read_conn(move |conn| {
            Ok(
                crate::db::orchestration::list_execution_events(conn, &execution_id)?
                    .into_iter()
                    .rev()
                    .find(|event| {
                        event.action == crate::api::orchestration::WORKER_PROCESSES_STOPPED_ACTION
                            && event.changes["dispatch_job_id"].as_str() == Some(previous.as_str())
                    })
                    .and_then(|event| event.changes["survivors"].as_array().cloned())
                    .map(|pids| {
                        pids.iter()
                            .filter_map(serde_json::Value::as_u64)
                            .collect::<Vec<u64>>()
                    })
                    .unwrap_or_default(),
            )
        })
        .await;
    survivors.unwrap_or_else(|error| {
        tracing::warn!("the survivors of a previous worker session are unreadable: {error}");
        Vec::new()
    })
}

/// KT-2021 — what both rooms are told (French, English) when a launch is given
/// up beside a previous session: which one, what is left of it, what to do.
fn previous_session_alive_notice(
    dispatch: &str,
    execution: &str,
    previous: &str,
    agent: &str,
    tries: Option<u32>,
    survivors: &[u64],
) -> (String, String) {
    let (fr_tries, en_tries) = match tries {
        Some(tries) => (
            format!(" après {tries} essais"),
            format!(" after {tries} tries"),
        ),
        None => (String::new(), String::new()),
    };
    let (fr_left, en_left) = if survivors.is_empty() {
        (
            "Aucun identifiant de processus n'a été relevé pour cette session.".to_string(),
            "No process id was recorded for this session.".to_string(),
        )
    } else {
        let pids = survivors
            .iter()
            .map(|pid| format!("`{pid}`"))
            .collect::<Vec<_>>()
            .join(", ");
        (
            format!("Processus encore en vie : {pids}."),
            format!("Processes still alive: {pids}."),
        )
    };
    (
        format!(
            "**Worker non démarré — une session précédente tourne encore**\n\nLe dispatch `{dispatch}` de l'exécution `{execution}` n'a pas démarré : une session précédente de ce worker, celle du dispatch `{previous}`, n'a pas pu être arrêtée{fr_tries}. {fr_left}\n\nL'agent `{agent}` n'est pas en cause : Kronn n'a confié l'exécution à aucun autre worker, et la réaffecter ne change rien. Arrêtez les processus de cette session, puis reprenez l'exécution avec `task_exec_resume`."
        ),
        format!(
            "**Worker not started — a previous session is still running**\n\nDispatch `{dispatch}` of execution `{execution}` did not start: a previous session of this worker, the one of dispatch `{previous}`, could not be stopped{en_tries}. {en_left}\n\nThe agent `{agent}` is not the cause: Kronn handed the execution to no other worker, and reassigning it changes nothing. Stop the processes of this session, then resume the execution with `task_exec_resume`."
        ),
    )
}

/// `fallback_only`: leave an execution that no fallback moved to the caller.
async fn settle_worker_launch_refusal(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    reason: &str,
    available: Option<Vec<crate::models::AgentType>>,
    fallback_only: bool,
    stop: LaunchStop,
) -> bool {
    let lookup = job.id.clone();
    let worker = match state
        .db
        .with_conn(move |conn| worker_launch_context(conn, &lookup))
        .await
    {
        Ok(Some(worker)) if worker.is_fallback || !fallback_only => worker,
        Ok(_) => return false,
        Err(error) => {
            tracing::warn!(dispatch_job_id = %job.id, "unable to read the worker of a refused launch: {error}");
            return false;
        }
    };
    let (fail_id, fail_discussion, fail_group) = (
        job.id.clone(),
        job.discussion_id.clone(),
        job.group_id.clone(),
    );
    let fail_error = stop.error(reason);
    let fail_kind = stop.failure_kind();
    // A fallback worker has links to move on to: the execution is not parked and
    // the principal is not told to reassign it until none of them takes it. A
    // worker that is not one has nowhere to go: it is settled as any failure.
    // KT-2021 — nor has a launch held beside a previous session: the agent is
    // not the cause, and another one would start beside the same session.
    let previous_session = previous_session_of_hold(reason).map(str::to_string);
    let hands_over = worker.is_fallback && previous_session.is_none();
    let settled = state
        .db
        .with_conn(move |conn| {
            let settle = if hands_over {
                persist_dispatch_settlement_handing_over
            } else {
                persist_dispatch_settlement
            };
            settle(
                conn,
                &fail_id,
                &fail_discussion,
                fail_group.as_deref(),
                crate::db::workflows::BatchChildOutcome::Failed,
                Some(&fail_error),
                Some(fail_kind),
            )
        })
        .await;
    match settled {
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
            return true;
        }
        Err(error) => {
            tracing::error!(
                "Unable to settle the refused launch of dispatch {}: {error}",
                job.id
            );
            return false;
        }
    }
    tracing::warn!(
        dispatch_job_id = %job.id,
        execution_id = %worker.execution_id,
        "task worker given up at launch on {}: {}",
        worker.agent,
        stop.detail(reason)
    );
    let detail: String = reason
        .chars()
        .take(super::agent_fallback::LAUNCH_REFUSAL_DETAIL_CHARS)
        .collect();
    let detail = stop.detail(&detail);
    let mut moved_to: Option<String> = None;
    // KT-2000 — the chain's own reason when it took nothing (« … it has no
    // shell … »): the stop notice carries it.
    let mut chain_stop: Option<String> = None;
    if hands_over {
        let available = match available {
            Some(available) => available,
            None => crate::api::orchestration::available_agent_types_for_state(state).await,
        };
        match provider_fallback_among(state, job, None, available.clone(), true).await {
            Ok(Some((alternative, _room))) => moved_to = Some(format!("{alternative:?}")),
            Ok(None) => {}
            Err(error) if crate::api::orchestration::is_execution_moved_on(&error) => {
                return execution_moved_on_since_refusal(state, job);
            }
            Err(error) => {
                tracing::warn!(dispatch_job_id = %job.id, "provider fallback after a refused launch failed: {error}")
            }
        }
        if moved_to.is_none() {
            // The chain posts its own line in the parent room when it moves on.
            let trigger = match stop {
                LaunchStop::Refused => crate::core::agent_fallback::Trigger::LaunchRefused,
                LaunchStop::Unavailable { .. } => {
                    crate::core::agent_fallback::Trigger::LaunchUnavailable
                }
            };
            match super::agent_fallback::worker_after_launch_refusal_among(
                state,
                job,
                trigger,
                &detail,
                Some(available),
            )
            .await
            {
                Ok(super::agent_fallback::LaunchChain::Moved) => {
                    state.agent_dispatch_notify.notify_waiters();
                    return true;
                }
                Ok(super::agent_fallback::LaunchChain::NoLink { why }) => chain_stop = why,
                Err(error) if crate::api::orchestration::is_execution_moved_on(&error) => {
                    return execution_moved_on_since_refusal(state, job);
                }
                Err(error) => {
                    tracing::warn!(dispatch_job_id = %job.id, "agent fallback chain after a refused launch failed: {error}")
                }
            }
        }
        // No link was found because the execution is no longer this dispatch's
        // (the quota return moved it between the settlement and here): it is
        // running on its original worker, and "no usable link is left" would be
        // said about it.
        if moved_to.is_none() && !execution_still_runs_dispatch(state, job).await {
            return execution_moved_on_since_refusal(state, job);
        }
    }
    if hands_over && moved_to.is_none() {
        // Nothing took the work over: what the settlement held back is due now.
        let (undelivered_id, undelivered_discussion) = (job.id.clone(), job.discussion_id.clone());
        if let Err(error) = state
            .db
            .with_conn(move |conn| {
                interrupt_undelivered_after_failure(conn, &undelivered_id, &undelivered_discussion)
            })
            .await
        {
            tracing::error!(
                dispatch_job_id = %job.id,
                "unable to park the execution of a refused launch no link could take: {error}"
            );
        }
    }
    let (agent, execution, dispatch) = (&worker.agent, &worker.execution_id, &job.id);
    let refused = matches!(stop, LaunchStop::Refused);
    match moved_to {
        Some(alternative) => {
            let (fr_title, en_title, fr_why, en_why) = if refused {
                (
                    "worker refusé au lancement",
                    "worker refused at launch",
                    "Cette cause ne change pas en attendant",
                    "Waiting does not change that",
                )
            } else {
                (
                    "worker indisponible au lancement",
                    "worker unavailable at launch",
                    "Les essais n'ont rien changé",
                    "Retrying changed nothing",
                )
            };
            post_room_notice(
                state,
                worker.parent_room.clone(),
                format!("worker-launch-fallback:{}", job.id),
                format!(
                    "**Bascule automatique — {fr_title}**\n\nLe dispatch `{dispatch}` de l'exécution `{execution}` n'a pas pu démarrer sur `{agent}` : {detail}\n\n{fr_why} : Kronn a réassigné l'exécution à `{alternative}`, dans sa sous-discussion et son worktree existants."
                ),
                format!(
                    "**Automatic fallback — {en_title}**\n\nDispatch `{dispatch}` of execution `{execution}` could not start on `{agent}`: {detail}\n\n{en_why}: Kronn reassigned the execution to `{alternative}`, in its existing child discussion and worktree."
                ),
            )
            .await;
        }
        None => {
            let (fr_chain, en_chain) = match chain_stop.as_deref() {
                Some(why) => (
                    format!("\n\nChaîne de repli : {why}"),
                    format!("\n\nFallback chain: {why}"),
                ),
                None => (String::new(), String::new()),
            };
            let (fr, en) = if let Some(previous) = previous_session.as_deref() {
                let survivors = previous_session_survivors(state, execution, previous).await;
                let tries = match stop {
                    LaunchStop::Unavailable { times } => Some(times),
                    LaunchStop::Refused => None,
                };
                previous_session_alive_notice(
                    dispatch, execution, previous, agent, tries, &survivors,
                )
            } else if refused {
                (
                    format!(
                        "**Worker non démarré — `{agent}` refusé au lancement**\n\nLe dispatch `{dispatch}` de l'exécution `{execution}` n'a pas pu démarrer sur `{agent}` : {detail}{fr_chain}\n\nCette cause ne change pas en attendant et aucun lien de repli utilisable ne reste : Kronn n'a pas relancé le dispatch et ne le fera pas seul. Réaffecte l'exécution à un worker compatible ({hint}) ou annule-la.",
                        hint = compatible_workers_hint(agent, true)
                    ),
                    format!(
                        "**Worker not started — `{agent}` refused at launch**\n\nDispatch `{dispatch}` of execution `{execution}` could not start on `{agent}`: {detail}{en_chain}\n\nWaiting does not change that and no usable fallback link is left: Kronn did not retry the dispatch and will not do it on its own. Reassign the execution to a compatible worker ({hint}) or cancel it.",
                        hint = compatible_workers_hint(agent, false)
                    ),
                )
            } else {
                (
                    format!(
                        "**Worker non démarré — `{agent}` indisponible**\n\nLe dispatch `{dispatch}` de l'exécution `{execution}` n'a pas pu démarrer sur `{agent}` : {detail}{fr_chain}\n\nAucun lien de repli utilisable ne reste : Kronn a cessé de relancer le dispatch et ne le fera pas seul. Réaffecte l'exécution à un autre worker ou annule-la."
                    ),
                    format!(
                        "**Worker not started — `{agent}` unavailable**\n\nDispatch `{dispatch}` of execution `{execution}` could not start on `{agent}`: {detail}{en_chain}\n\nNo usable fallback link is left: Kronn stopped retrying the dispatch and will not do it on its own. Reassign the execution to another worker or cancel it."
                    ),
                )
            };
            post_room_notice(
                state,
                worker.parent_room.clone(),
                format!("worker-launch-refused:{}", job.id),
                fr.clone(),
                en.clone(),
            )
            .await;
            post_room_notice(
                state,
                job.discussion_id.clone(),
                format!("worker-launch-refused-child:{}", job.id),
                fr,
                en,
            )
            .await;
        }
    }
    state.agent_dispatch_notify.notify_waiters();
    true
}

/// KT-1758 — whether the execution of `job` still points at it. `true` when
/// that cannot be read: the caller then keeps its own behaviour.
async fn execution_still_runs_dispatch(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
) -> bool {
    let id = job.id.clone();
    state
        .db
        .with_conn(move |conn| {
            Ok(crate::db::orchestration::get_execution_for_dispatch(conn, &id)?.is_some())
        })
        .await
        .unwrap_or(true)
}

/// KT-1758 — the execution moved on while a refused launch was being settled:
/// the failed dispatch has nothing left to hand over, and says nothing.
fn execution_moved_on_since_refusal(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
) -> bool {
    tracing::info!(
        dispatch_job_id = %job.id,
        "a refused launch's execution was moved meanwhile: nothing to hand over, nothing said"
    );
    state.agent_dispatch_notify.notify_waiters();
    true
}

/// One orchestrator line in `room`, in the room's language, written once per `id`.
async fn post_room_notice(state: &AppState, room: String, id: String, fr: String, en: String) {
    let posted = state
        .db
        .with_conn(move |conn| {
            let already: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1)",
                [&id],
                |row| row.get(0),
            )?;
            if already {
                return Ok(());
            }
            let lang = crate::db::discussions::room_lang(conn, &room);
            let message = crate::api::orchestration::orchestrator_message(
                id,
                lang.pick(&fr, &en).to_string(),
            );
            crate::db::discussions::insert_message(conn, &room, &message)?;
            Ok(())
        })
        .await;
    if let Err(error) = posted {
        tracing::warn!("worker launch notice could not be persisted: {error}");
    }
}

/// KT-1758 — an unavailable runtime for a task worker's dispatch: retried while
/// the cause can change, never forever.
///
/// The cause is permanent for a fallback whose binary is missing: it moves on.
/// Otherwise the deferral is counted (in the dispatch's own `last_error`, which
/// the claim keeps for it); the same reason
/// [`RUNTIME_UNAVAILABLE_MAX_DEFERRALS`] times in a row ends the retries and
/// says why, in both rooms, with the reason as the runtime gave it. A dispatch
/// that is not a task worker's keeps the durable deferral it always had.
///
/// `false` when neither the deferral nor the stop could be written: the
/// dispatch is still claimed.
pub async fn defer_or_stop_runtime_unavailable(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    reason: &str,
) -> bool {
    defer_or_stop_runtime_unavailable_among(state, job, reason, None).await
}

/// [`defer_or_stop_runtime_unavailable`] with the installed agents given, so the
/// move to the next link is testable without the CLIs of the host.
pub async fn defer_or_stop_runtime_unavailable_among(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    reason: &str,
    available: Option<Vec<crate::models::AgentType>>,
) -> bool {
    let lookup = job.id.clone();
    let context = state
        .db
        .with_conn(move |conn| {
            Ok((
                crate::db::agent_dispatch::get(conn, &lookup)?
                    .and_then(|current| current.last_error),
                worker_launch_context(conn, &lookup)?,
            ))
        })
        .await;
    let (previous, worker) = match context {
        Ok(context) => context,
        Err(error) => {
            tracing::warn!(dispatch_job_id = %job.id, "unable to read the deferred dispatch: {error}");
            (None, None)
        }
    };
    let Some(worker) = worker else {
        return defer_runtime_unavailable(state, job, reason, 1).await;
    };
    if worker.is_fallback
        && is_missing_binary(reason)
        && settle_worker_launch_refusal(
            state,
            job,
            reason,
            available.clone(),
            true,
            LaunchStop::Refused,
        )
        .await
    {
        return true;
    }
    let deferrals = previous
        .as_deref()
        .and_then(crate::db::agent_dispatch::parse_runtime_unavailable)
        .filter(|(_, earlier)| *earlier == reason)
        .map_or(0, |(count, _)| count)
        + 1;
    if deferrals > RUNTIME_UNAVAILABLE_MAX_DEFERRALS {
        // Not a refusal: a cause that can change, which did not in all this
        // time. Said as that, and never as a reason to go to the failing agent.
        let stop = LaunchStop::Unavailable { times: deferrals };
        if settle_worker_launch_refusal(state, job, reason, available, false, stop).await {
            return true;
        }
    }
    defer_runtime_unavailable(state, job, reason, deferrals).await
}

/// `false` when the deferral could not be written.
async fn defer_runtime_unavailable(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    reason: &str,
    deferrals: u32,
) -> bool {
    let job_id = job.id.clone();
    let persisted_reason = crate::db::agent_dispatch::runtime_unavailable_marker(deferrals, reason);
    let written = match state
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
        Ok(true) => {
            tracing::warn!(
                "Deferred dispatch {} for {}s because its runtime is unavailable: {}",
                job.id,
                RUNTIME_UNAVAILABLE_RETRY_DELAY_SECONDS,
                reason
            );
            true
        }
        // No longer running (cancelled meanwhile): nothing is left to start.
        Ok(false) => true,
        Err(error) => {
            tracing::error!(
                "Unable to defer dispatch {} after unavailable runtime: {}",
                job.id,
                error
            );
            false
        }
    };
    state.agent_dispatch_notify.notify_waiters();
    written
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
pub fn render_chain_qp_prompt(
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
        render_chain_qp_prompt, select_quota_alternative, settle_cancelled_worker,
        unsuccessful_completion_diagnostic, DispatchHandoffGuard,
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
        assert!(
            cards[0].contains("j-vis"),
            "retryable from the room: {cards:?}"
        );

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
                    shadow_tokens_used: 0,
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
                    token_declared: false,
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
            let notice_id = format!("orch-worker-undelivered:{execution_id}:j-undelivered");
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
                    token_declared: false,
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
            let notice_id = format!("orch-worker-undelivered:{execution_id}:j-failed-undelivered");
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

    /// KT-1782 — a cancel that went through no stop handler: the end of the
    /// worker's stream parks its execution, and a second pass adds nothing.
    #[tokio::test]
    async fn a_cancelled_task_worker_dispatch_interrupts_its_execution_when_its_stream_ends() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let (execution_id, job) = db
            .with_conn(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                for (id, title) in [("d-cancel-parent", "Parent"), ("d-cancel-child", "Child")] {
                    conn.execute(
                        "INSERT INTO discussions (id, title, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?3)",
                        rusqlite::params![id, title, now],
                    )?;
                }
                conn.execute(
                    "INSERT INTO planning_tasks
                     (id, task_number, title, status, created_at, updated_at)
                     VALUES ('t-cancelled', 9903, 'Cancelled', 'todo', ?1, ?1)",
                    [&now],
                )?;
                let actor = crate::models::PlanningActor {
                    kind: crate::models::PlanningActorKind::Backend,
                    id: Some("orchestrator".into()),
                    session_id: None,
                    source_message_id: None,
                    token_declared: false,
                };
                let execution = crate::db::orchestration::launch_single_task(
                    conn,
                    &crate::models::LaunchSingleTaskInput::new("t-cancelled", "d-cancel-parent"),
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
                     VALUES ('u-cancelled', 'd-cancel-child', 'User', 'go', ?1, 1, ?1)",
                    [&now],
                )?;
                crate::db::agent_dispatch::enqueue_for_latest_user(
                    conn,
                    crate::db::agent_dispatch::NewLatestUserDispatch {
                        id: "j-cancelled",
                        discussion_id: "d-cancel-child",
                        dedupe_key: "message:u-cancelled",
                        agent_override: Some(&crate::models::AgentType::Ollama),
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: None,
                        group_concurrency_limit: None,
                    },
                )?;
                crate::db::agent_dispatch::claim(conn, "j-cancelled")?;
                crate::db::orchestration::attach_execution_dispatch(
                    conn,
                    &execution.id,
                    "j-cancelled",
                )?;
                assert!(crate::db::agent_dispatch::cancel_for_discussion_by_id(
                    conn,
                    "d-cancel-child",
                    "j-cancelled"
                )?);
                let job = crate::db::agent_dispatch::get(conn, "j-cancelled")?
                    .expect("the cancelled dispatch stays on record");
                Ok((execution.id, job))
            })
            .await
            .unwrap();
        let state = crate::AppState::new_defaults(
            Arc::new(RwLock::new(crate::core::config::default_config())),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );

        settle_cancelled_worker(&state, &job).await;
        settle_cancelled_worker(&state, &job).await;

        db.with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                .expect("execution must remain queryable");
            assert_eq!(
                execution.status,
                crate::models::TaskExecutionStatus::Interrupted
            );
            let recovery = crate::db::orchestration::get_execution_recovery(conn, &execution_id)?
                .expect("a stopped worker is parked for a decision");
            assert_eq!(recovery.recovery_reason, "worker_stopped_without_delivery");
            assert_eq!(
                recovery.recovery_action,
                crate::models::ExecutionRecoveryAction::ResumeWorker
            );
            assert!(recovery.pending);
            let mut statement = conn.prepare(
                "SELECT id, content FROM messages
                 WHERE discussion_id = 'd-cancel-parent'
                   AND id LIKE 'orch-worker-stopped:%'",
            )?;
            let notices = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(notices.len(), 1, "one notice for two passes: {notices:?}");
            let (notice_id, notice) = &notices[0];
            assert_eq!(
                notice_id,
                &format!("orch-worker-stopped:{execution_id}:j-cancelled")
            );
            assert!(
                notice.contains("Worker arrêté sans livraison durable"),
                "{notice}"
            );
            assert!(notice.contains("task_exec_resume"), "{notice}");
            assert_eq!(
                crate::db::discussions::list_message_targets(conn, notice_id)?.len(),
                1,
                "the notice is aimed at the principal"
            );
            Ok(())
        })
        .await
        .unwrap();
    }

    fn backend_actor() -> crate::models::PlanningActor {
        crate::models::PlanningActor {
            kind: crate::models::PlanningActorKind::Backend,
            id: Some("orchestrator".into()),
            session_id: None,
            source_message_id: None,
            token_declared: false,
        }
    }

    /// A claimed dispatch `job` of the room `d-limit-child`, attached to
    /// `execution_id`: the worker's first run, or the run after a resume.
    fn attach_claimed_dispatch(
        conn: &rusqlite::Connection,
        execution_id: &str,
        job: &str,
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO messages
             (id, discussion_id, role, content, timestamp, sort_order, received_at)
             VALUES (?1, 'd-limit-child', 'User', 'go', ?2,
                     (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM messages
                       WHERE discussion_id = 'd-limit-child'), ?2)",
            rusqlite::params![format!("u-{job}"), now],
        )?;
        // The room's own counter follows, so a notice posted later gets a free order.
        conn.execute(
            "UPDATE discussions SET next_message_seq =
                 (SELECT MAX(sort_order) + 1 FROM messages WHERE discussion_id = 'd-limit-child')
             WHERE id = 'd-limit-child'",
            [],
        )?;
        crate::db::agent_dispatch::enqueue_for_latest_user(
            conn,
            crate::db::agent_dispatch::NewLatestUserDispatch {
                id: job,
                discussion_id: "d-limit-child",
                dedupe_key: &format!("message:u-{job}"),
                agent_override: Some(&crate::models::AgentType::Ollama),
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )?;
        crate::db::agent_dispatch::claim(conn, job)?;
        crate::db::orchestration::attach_execution_dispatch(conn, execution_id, job)?;
        Ok(())
    }

    /// A `Working` execution launched from `d-limit-parent`, whose worker runs
    /// the claimed dispatch `job` in `d-limit-child`.
    fn working_execution(conn: &rusqlite::Connection, job: &str) -> anyhow::Result<String> {
        let now = chrono::Utc::now().to_rfc3339();
        for (id, title) in [("d-limit-parent", "Parent"), ("d-limit-child", "Child")] {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)",
                rusqlite::params![id, title, now],
            )?;
        }
        conn.execute(
            "INSERT INTO planning_tasks
             (id, task_number, title, status, created_at, updated_at)
             VALUES ('t-limit', 9904, 'Limit', 'todo', ?1, ?1)",
            [&now],
        )?;
        let execution = crate::db::orchestration::launch_single_task(
            conn,
            &crate::models::LaunchSingleTaskInput::new("t-limit", "d-limit-parent"),
            &backend_actor(),
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
                &backend_actor(),
                serde_json::json!({}),
            )?;
        }
        attach_claimed_dispatch(conn, &execution.id, job)?;
        Ok(execution.id)
    }

    /// The worker of `execution_id` is resumed on a new claimed dispatch `job`.
    fn resume_on(conn: &rusqlite::Connection, execution_id: &str, job: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            crate::db::orchestration::transition_execution(
                conn,
                execution_id,
                crate::models::TaskExecutionStatus::Working,
                &backend_actor(),
                serde_json::json!({}),
            )?,
            "the parked execution goes back to Working"
        );
        attach_claimed_dispatch(conn, execution_id, job)
    }

    /// The limit stops the run of `job` after 240 of 240 minutes, then its
    /// dispatch settles as failed, the way the end of the stream does.
    fn stop_at_the_limit_and_settle(
        conn: &rusqlite::Connection,
        execution_id: &str,
        job: &str,
    ) -> anyhow::Result<()> {
        let limit = std::time::Duration::from_secs(240 * 60);
        crate::api::worker_time_limit::record_stop_event(
            conn,
            execution_id,
            job,
            limit,
            limit,
            None,
        )?;
        let settled = persist_dispatch_settlement(
            conn,
            job,
            "d-limit-child",
            None,
            crate::db::workflows::BatchChildOutcome::Failed,
            Some("task worker stopped by Kronn"),
            None,
        )?;
        anyhow::ensure!(settled.changed, "the dispatch settles");
        Ok(())
    }

    /// The notices of `d-limit-parent` whose id starts with `prefix`, oldest first.
    fn parent_notices(
        conn: &rusqlite::Connection,
        prefix: &str,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let mut statement = conn.prepare(
            "SELECT id, content FROM messages
             WHERE discussion_id = 'd-limit-parent' AND id LIKE ?1 || '%'
             ORDER BY sort_order",
        )?;
        let notices = statement
            .query_map([prefix], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(notices)
    }

    /// KT-2037 — a worker Kronn stopped at the run time limit is parked with
    /// that cause, and the principal is told the limit, not "worker failed".
    #[tokio::test]
    async fn a_worker_stopped_by_the_time_limit_is_parked_with_that_cause() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            let execution_id = working_execution(conn, "j-limit")?;
            stop_at_the_limit_and_settle(conn, &execution_id, "j-limit")?;

            let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                .expect("execution must remain queryable");
            assert_eq!(
                execution.status,
                crate::models::TaskExecutionStatus::Interrupted
            );
            let recovery = crate::db::orchestration::get_execution_recovery(conn, &execution_id)?
                .expect("a limit stop leaves a decision to the principal");
            assert_eq!(
                recovery.recovery_reason,
                "worker_timed_out_without_delivery"
            );
            assert!(recovery.pending);

            let notices = parent_notices(conn, "orch-worker-")?;
            assert_eq!(notices.len(), 1, "{notices:?}");
            let (notice_id, notice) = &notices[0];
            assert_eq!(
                notice_id,
                &format!("orch-worker-timed-out:{execution_id}:j-limit")
            );
            assert!(notice.contains("limite de temps"), "{notice}");
            assert!(notice.contains("après 240 min"), "{notice}");
            assert!(notice.contains("240 min s'il reste le même"), "{notice}");
            assert!(notice.contains("le worktree était propre"), "{notice}");
            assert!(notice.contains("task_exec_resume"), "{notice}");
            assert!(!notice.contains("Worker échoué"), "{notice}");
            assert_eq!(
                crate::db::discussions::list_message_targets(conn, notice_id)?.len(),
                1,
                "the notice is aimed at the principal"
            );
            Ok(())
        })
        .await
        .unwrap();
    }

    /// KT-2037 — a failure with no limit stop of its run keeps the failed
    /// notice: only the journal of the dispatch turns it into a limit stop.
    #[tokio::test]
    async fn a_failed_worker_without_a_limit_stop_keeps_the_failed_notice() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            let execution_id = working_execution(conn, "j-failed")?;
            // A stop of another dispatch of the same execution is not this run's.
            let limit = std::time::Duration::from_secs(240 * 60);
            crate::api::worker_time_limit::record_stop_event(
                conn,
                &execution_id,
                "j-earlier",
                limit,
                limit,
                None,
            )?;
            let settled = persist_dispatch_settlement(
                conn,
                "j-failed",
                "d-limit-child",
                None,
                crate::db::workflows::BatchChildOutcome::Failed,
                Some("provider crashed"),
                None,
            )?;
            assert!(settled.changed);

            let recovery = crate::db::orchestration::get_execution_recovery(conn, &execution_id)?
                .expect("failure must leave an explicit human recovery");
            assert_eq!(recovery.recovery_reason, "worker_failed_without_delivery");
            let notices = parent_notices(conn, "orch-worker-")?;
            assert_eq!(notices.len(), 1, "{notices:?}");
            assert_eq!(
                notices[0].0,
                format!("orch-worker-undelivered:{execution_id}:j-failed")
            );
            assert!(
                notices[0]
                    .1
                    .contains("Worker échoué sans livraison durable"),
                "{notices:?}"
            );
            assert!(!notices[0].1.contains("limite de temps"), "{notices:?}");
            Ok(())
        })
        .await
        .unwrap();
    }

    /// KT-2037 — a resumed worker the limit stops again is announced again:
    /// the notice is per dispatch, not per execution.
    #[tokio::test]
    async fn a_second_time_limit_stop_is_announced_again() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            let execution_id = working_execution(conn, "j-limit-1")?;
            stop_at_the_limit_and_settle(conn, &execution_id, "j-limit-1")?;
            resume_on(conn, &execution_id, "j-limit-2")?;
            stop_at_the_limit_and_settle(conn, &execution_id, "j-limit-2")?;

            let notices = parent_notices(conn, "orch-worker-timed-out:")?;
            let ids: Vec<&str> = notices.iter().map(|(id, _)| id.as_str()).collect();
            assert_eq!(
                ids,
                [
                    format!("orch-worker-timed-out:{execution_id}:j-limit-1"),
                    format!("orch-worker-timed-out:{execution_id}:j-limit-2"),
                ]
            );
            for (notice_id, _) in &notices {
                assert_eq!(
                    crate::db::discussions::list_message_targets(conn, notice_id)?.len(),
                    1,
                    "each notice is aimed at the principal"
                );
            }
            let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                .expect("execution must remain queryable");
            assert_eq!(
                execution.status,
                crate::models::TaskExecutionStatus::Interrupted
            );
            Ok(())
        })
        .await
        .unwrap();
    }

    /// KT-2108 — each dispatch of one execution that ends without delivering
    /// posts its own notice, so the second failure wakes the principal too.
    #[tokio::test]
    async fn a_second_dispatch_that_ends_without_delivery_gets_its_own_notice() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            let execution_id = working_execution(conn, "j-first")?;
            let settle = |job: &str, error: Option<&str>| {
                persist_dispatch_settlement(
                    conn,
                    job,
                    "d-limit-child",
                    None,
                    if error.is_some() {
                        crate::db::workflows::BatchChildOutcome::Failed
                    } else {
                        crate::db::workflows::BatchChildOutcome::Success
                    },
                    error,
                    None,
                )
            };
            // The first run ends its turn without delivering; the resumed one fails.
            assert!(settle("j-first", None)?.changed);
            resume_on(conn, &execution_id, "j-second")?;
            assert!(settle("j-second", Some("provider crashed"))?.changed);
            // A replayed settlement of the same dispatch adds nothing (KT-402).
            assert!(!settle("j-second", Some("provider crashed"))?.changed);

            let notices = parent_notices(conn, "orch-worker-undelivered:")?;
            let ids: Vec<&str> = notices.iter().map(|(id, _)| id.as_str()).collect();
            assert_eq!(
                ids,
                [
                    format!("orch-worker-undelivered:{execution_id}:j-first"),
                    format!("orch-worker-undelivered:{execution_id}:j-second"),
                ]
            );
            assert!(
                notices[0]
                    .1
                    .contains("Worker terminé sans livraison durable"),
                "{notices:?}"
            );
            assert!(
                notices[1]
                    .1
                    .contains("Worker échoué sans livraison durable"),
                "{notices:?}"
            );
            assert!(notices[1].1.contains("`j-second`"), "{notices:?}");
            for (notice_id, _) in &notices {
                assert_eq!(
                    crate::db::discussions::list_message_targets(conn, notice_id)?.len(),
                    1,
                    "each notice is aimed at the principal"
                );
            }
            let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                .expect("execution must remain queryable");
            assert_eq!(
                execution.status,
                crate::models::TaskExecutionStatus::Interrupted
            );
            Ok(())
        })
        .await
        .unwrap();
    }

    /// A `sleep` leading its own process group, as a worker's agent does. It
    /// is killed on drop unless the test saw it gone: a group that ended is
    /// never signalled, its id may be another's.
    #[cfg(unix)]
    struct SleepingGroup {
        child: std::process::Child,
        gone: bool,
    }

    #[cfg(unix)]
    impl SleepingGroup {
        fn start() -> Self {
            let mut command = std::process::Command::new("sleep");
            command.arg("600");
            std::os::unix::process::CommandExt::process_group(&mut command, 0);
            Self {
                child: command.spawn().unwrap(),
                gone: false,
            }
        }

        /// The recorded form of this group: its id and its leader's start stamp.
        fn recorded(&self) -> (i64, Option<String>) {
            let stamp = crate::core::agent_orphans::start_stamp(self.child.id());
            assert!(
                stamp.is_some(),
                "the platform must be able to stamp a process"
            );
            (i64::from(self.child.id()), stamp)
        }

        /// Whether the leader ended and nothing is left in its group, within `bound`.
        fn gone_within(&mut self, bound: std::time::Duration) -> bool {
            let deadline = std::time::Instant::now() + bound;
            loop {
                let ended = matches!(self.child.try_wait(), Ok(Some(_)));
                // SAFETY: signal 0 only checks that the group exists.
                if ended && unsafe { libc::killpg(self.child.id() as i32, 0) } != 0 {
                    self.gone = true;
                    return true;
                }
                if std::time::Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    #[cfg(unix)]
    impl Drop for SleepingGroup {
        fn drop(&mut self) {
            if !self.gone {
                // SAFETY: the group this test started, not yet seen gone.
                unsafe { libc::killpg(self.child.id() as i32, libc::SIGKILL) };
            }
            let _ = self.child.wait();
        }
    }

    fn previous_session_agent_gone(
        _pgid: i64,
        _marker: Option<&str>,
    ) -> crate::core::agent_orphans::OrphanOutcome {
        crate::core::agent_orphans::OrphanOutcome::Gone
    }

    fn previous_session_reap_nothing(_tag: &str) -> crate::core::dispatch_reaper::ReapReport {
        crate::core::dispatch_reaper::ReapReport::default()
    }

    fn previous_session_reap_a_survivor(_tag: &str) -> crate::core::dispatch_reaper::ReapReport {
        crate::core::dispatch_reaper::ReapReport {
            signalled: 1,
            survivors: vec![4242],
            unverified: false,
        }
    }

    /// A `Working` execution resumed on the claimed dispatch `j-ghost-new`,
    /// after its dispatch `j-ghost-old` was cancelled. The old one is a
    /// recorded worker launch of this instance, with `agent` as its group.
    fn resumed_beside_a_recorded_session(
        conn: &rusqlite::Connection,
        agent: Option<(i64, Option<String>)>,
    ) -> anyhow::Result<(String, crate::db::agent_dispatch::AgentDispatchJob)> {
        let execution_id = working_execution(conn, "j-ghost-old")?;
        crate::db::orchestration::set_execution_sub_discussion(
            conn,
            &execution_id,
            "d-limit-child",
        )?;
        anyhow::ensure!(
            crate::db::agent_dispatch::cancel_for_discussion_by_id(
                conn,
                "d-limit-child",
                "j-ghost-old"
            )?,
            "the previous dispatch is cancelled"
        );
        attach_claimed_dispatch(conn, &execution_id, "j-ghost-new")?;
        crate::db::worker_processes::record_launch(
            conn,
            "j-ghost-old",
            &execution_id,
            crate::core::dispatch_reaper::instance_id(),
        )?;
        if let Some((pgid, marker)) = agent {
            crate::db::worker_processes::set_agent(conn, "j-ghost-old", pgid, marker.as_deref())?;
        }
        let job = crate::db::agent_dispatch::get(conn, "j-ghost-new")?
            .ok_or_else(|| anyhow::anyhow!("the resumed dispatch is on record"))?;
        anyhow::ensure!(
            job.status == crate::db::agent_dispatch::DispatchStatus::Running,
            "the resumed dispatch is claimed"
        );
        Ok((execution_id, job))
    }

    /// The `changes` of the worker-process stops journaled on `execution_id`.
    fn worker_stop_events(
        conn: &rusqlite::Connection,
        execution_id: &str,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        Ok(
            crate::db::orchestration::list_execution_events(conn, execution_id)?
                .into_iter()
                .filter(|event| {
                    event.action == crate::api::orchestration::WORKER_PROCESSES_STOPPED_ACTION
                })
                .map(|event| event.changes)
                .collect(),
        )
    }

    /// KT-2021 — a previous session of the execution still alive when its
    /// worker is resumed is killed first: the launch is not held.
    #[cfg(unix)] // kills a `sleep` process group through libc
    #[tokio::test]
    async fn a_previous_session_still_running_is_stopped_before_the_worker_is_resumed() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let mut previous = SleepingGroup::start();
        let recorded = previous.recorded();
        let (execution_id, job) = db
            .with_conn(move |conn| resumed_beside_a_recorded_session(conn, Some(recorded)))
            .await
            .unwrap();
        let state = crate::AppState::new_defaults(
            Arc::new(RwLock::new(crate::core::config::default_config())),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );

        let held = super::hold_worker_launch_beside_a_previous_session_with(
            &state,
            &job,
            crate::api::orchestration::kill_recorded_worker_group,
            previous_session_reap_nothing,
        )
        .await
        .is_some();

        assert!(!held, "nothing is left of the previous session");
        assert!(
            previous.gone_within(std::time::Duration::from_secs(30)),
            "the previous session's agent must be gone before the worker starts"
        );
        db.with_conn(move |conn| {
            let old = crate::db::worker_processes::get(conn, "j-ghost-old")?
                .expect("the recorded launch");
            assert!(old.stopped_at.is_some(), "the previous launch is settled");
            let resumed = crate::db::agent_dispatch::get(conn, "j-ghost-new")?.unwrap();
            assert_eq!(
                resumed.status,
                crate::db::agent_dispatch::DispatchStatus::Running,
                "the resumed dispatch goes on to its launch"
            );
            let events = worker_stop_events(conn, &execution_id)?;
            assert_eq!(events.len(), 1, "{events:?}");
            assert_eq!(events[0]["dispatch_job_id"], "j-ghost-old");
            assert_eq!(events[0]["agent"], "killed");
            Ok(())
        })
        .await
        .unwrap();
    }

    /// KT-2021 — a previous session that cannot be stopped holds the launch:
    /// the dispatch is deferred with the stable reason, and once the deferrals
    /// run out the execution is parked and both rooms are told why.
    #[tokio::test]
    async fn a_previous_session_that_cannot_be_stopped_holds_the_launch_and_says_why() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let (execution_id, mut job) = db
            .with_conn(|conn| resumed_beside_a_recorded_session(conn, None))
            .await
            .unwrap();
        let state = crate::AppState::new_defaults(
            Arc::new(RwLock::new(crate::core::config::default_config())),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let reason = format!(
            "{}: dispatch j-ghost-old",
            super::PREVIOUS_WORKER_SESSION_ALIVE
        );
        let bound = super::RUNTIME_UNAVAILABLE_MAX_DEFERRALS;

        for attempt in 1..=bound {
            let held = super::hold_worker_launch_beside_a_previous_session_with(
                &state,
                &job,
                previous_session_agent_gone,
                previous_session_reap_a_survivor,
            )
            .await
            .is_some();
            assert!(held, "try {attempt}: the launch is held");
            let expected = crate::db::agent_dispatch::runtime_unavailable_marker(attempt, &reason);
            job = db
                .with_conn(move |conn| {
                    let deferred = crate::db::agent_dispatch::get(conn, "j-ghost-new")?.unwrap();
                    assert_eq!(
                        deferred.status,
                        crate::db::agent_dispatch::DispatchStatus::Pending,
                        "still retried"
                    );
                    assert_eq!(deferred.last_error.as_deref(), Some(expected.as_str()));
                    // The next scanner pass: the deferred dispatch is due again.
                    conn.execute(
                        "UPDATE agent_dispatch_jobs SET available_at = ?1 WHERE id = 'j-ghost-new'",
                        [chrono::Utc::now().to_rfc3339()],
                    )?;
                    crate::db::agent_dispatch::claim(conn, "j-ghost-new")?
                        .ok_or_else(|| anyhow::anyhow!("the deferred dispatch is claimable"))
                })
                .await
                .unwrap();
        }
        let held = super::hold_worker_launch_beside_a_previous_session_with(
            &state,
            &job,
            previous_session_agent_gone,
            previous_session_reap_a_survivor,
        )
        .await
        .is_some();
        assert!(held, "the launch that is given up is not started either");

        db.with_conn(move |conn| {
            let ended = crate::db::agent_dispatch::get(conn, "j-ghost-new")?.unwrap();
            assert_eq!(
                ended.status,
                crate::db::agent_dispatch::DispatchStatus::Failed,
                "the bound ends the retries"
            );
            let error = ended.last_error.clone().unwrap_or_default();
            assert!(error.contains(&reason), "{error}");
            assert_eq!(ended.failure_kind.as_deref(), Some("launch_unavailable"));
            let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                .expect("execution must remain queryable");
            assert_eq!(
                execution.status,
                crate::models::TaskExecutionStatus::Interrupted
            );
            for (room, notice_id) in [
                ("d-limit-parent", "worker-launch-refused:j-ghost-new"),
                ("d-limit-child", "worker-launch-refused-child:j-ghost-new"),
            ] {
                let notice: String = conn.query_row(
                    "SELECT content FROM messages WHERE id = ?1 AND discussion_id = ?2",
                    rusqlite::params![notice_id, room],
                    |row| row.get(0),
                )?;
                // Which session, what is left of it, and the remedy that works.
                for said in ["`j-ghost-old`", "`4242`", "`task_exec_resume`"] {
                    assert!(notice.contains(said), "{room}: {said} missing in {notice}");
                }
                for absent in [
                    "Réaffecte l'exécution",
                    "Reassign the execution",
                    super::PREVIOUS_WORKER_SESSION_ALIVE,
                ] {
                    assert!(!notice.contains(absent), "{room}: {absent:?} in {notice}");
                }
            }
            // The notice that wakes the principal does not send it to a reassignment.
            let woken = parent_notices(conn, "orch-worker-undelivered:")?;
            assert_eq!(woken.len(), 1, "{woken:?}");
            assert!(woken[0].1.contains("`task_exec_resume`"), "{}", woken[0].1);
            assert!(
                !woken[0].1.contains("réaffecte ou annule"),
                "{}",
                woken[0].1
            );
            let old = crate::db::worker_processes::get(conn, "j-ghost-old")?
                .expect("the recorded launch");
            assert!(old.stopped_at.is_none(), "a survivor leaves it unsettled");
            let events = worker_stop_events(conn, &execution_id)?;
            assert_eq!(
                events.len(),
                1,
                "the same stop is journaled once: {events:?}"
            );
            assert_eq!(events[0]["survivors"], serde_json::json!([4242]));
            Ok(())
        })
        .await
        .unwrap();
    }

    /// A claimed dispatch `job` of `room` for `agent`, triggered by a user turn
    /// of its own.
    fn claimed_dispatch_in(
        conn: &rusqlite::Connection,
        room: &str,
        job: &str,
        agent: &crate::models::AgentType,
    ) -> anyhow::Result<crate::db::agent_dispatch::AgentDispatchJob> {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO messages
             (id, discussion_id, role, content, timestamp, sort_order, received_at)
             VALUES (?1, ?2, 'User', 'go', ?3,
                     (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM messages
                       WHERE discussion_id = ?2), ?3)",
            rusqlite::params![format!("u-{job}"), room, now],
        )?;
        conn.execute(
            "UPDATE discussions SET next_message_seq =
                 (SELECT MAX(sort_order) + 1 FROM messages WHERE discussion_id = ?1)
             WHERE id = ?1",
            [room],
        )?;
        crate::db::agent_dispatch::enqueue_for_latest_user(
            conn,
            crate::db::agent_dispatch::NewLatestUserDispatch {
                id: job,
                discussion_id: room,
                dedupe_key: &format!("message:u-{job}"),
                agent_override: Some(agent),
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )?;
        crate::db::agent_dispatch::claim(conn, job)?
            .ok_or_else(|| anyhow::anyhow!("dispatch {job} is claimable"))
    }

    /// A live stream of `dispatch`: its cancel token, registered as a run's is.
    fn live_stream(state: &crate::AppState, dispatch: &str) -> tokio_util::sync::CancellationToken {
        let token = tokio_util::sync::CancellationToken::new();
        state
            .cancel_registry
            .lock()
            .unwrap()
            .insert(dispatch.to_string(), token.clone());
        token
    }

    /// KT-2021 — the hold acts only for a worker launch. An execution can point
    /// at a dispatch of its parent room: that dispatch starts no worker, so it
    /// is not held and the worker's live stream is not cancelled.
    #[tokio::test]
    async fn a_dispatch_of_the_parent_room_holds_nothing_and_cancels_no_worker() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let (execution_id, job) = db
            .with_conn(|conn| {
                let execution_id = working_execution(conn, "j-ghost-worker")?;
                crate::db::orchestration::set_execution_sub_discussion(
                    conn,
                    &execution_id,
                    "d-limit-child",
                )?;
                crate::db::worker_processes::record_launch(
                    conn,
                    "j-ghost-worker",
                    &execution_id,
                    crate::core::dispatch_reaper::instance_id(),
                )?;
                // A turn of the parent room the execution now points at.
                let job = claimed_dispatch_in(
                    conn,
                    "d-limit-parent",
                    "j-ghost-parent",
                    &crate::models::AgentType::Ollama,
                )?;
                crate::db::orchestration::attach_execution_dispatch(
                    conn,
                    &execution_id,
                    "j-ghost-parent",
                )?;
                Ok((execution_id, job))
            })
            .await
            .unwrap();
        let state = crate::AppState::new_defaults(
            Arc::new(RwLock::new(crate::core::config::default_config())),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let worker_stream = live_stream(&state, "j-ghost-worker");

        let held = super::hold_worker_launch_beside_a_previous_session_with(
            &state,
            &job,
            previous_session_agent_gone,
            previous_session_reap_a_survivor,
        )
        .await
        .is_some();

        assert!(!held, "a turn of the parent room is not a worker launch");
        assert!(
            !worker_stream.is_cancelled(),
            "the worker that is running must not be cancelled by a turn of the parent room"
        );
        db.with_conn(move |conn| {
            let parent_turn = crate::db::agent_dispatch::get(conn, "j-ghost-parent")?.unwrap();
            assert_eq!(
                parent_turn.status,
                crate::db::agent_dispatch::DispatchStatus::Running,
                "the turn of the parent room goes on"
            );
            assert_eq!(parent_turn.last_error, None);
            let worker = crate::db::worker_processes::get(conn, "j-ghost-worker")?
                .expect("the recorded launch");
            assert!(worker.stopped_at.is_none(), "the worker was not stopped");
            assert!(worker_stop_events(conn, &execution_id)?.is_empty());
            Ok(())
        })
        .await
        .unwrap();
    }

    /// KT-2021 — a dispatch launched again (a watchdog requeue, a retry) is held
    /// while the session of its own previous run may still be alive, and is
    /// launched, its record settled, once nothing is left. A token under its
    /// own id is never cancelled: its new run registers under that key.
    #[tokio::test]
    async fn a_requeued_dispatch_is_held_beside_its_own_previous_session() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let (execution_id, job) = db
            .with_conn(|conn| {
                let execution_id = working_execution(conn, "j-ghost-own")?;
                crate::db::orchestration::set_execution_sub_discussion(
                    conn,
                    &execution_id,
                    "d-limit-child",
                )?;
                // What the previous run of this very dispatch recorded.
                crate::db::worker_processes::record_launch(
                    conn,
                    "j-ghost-own",
                    &execution_id,
                    crate::core::dispatch_reaper::instance_id(),
                )?;
                let job = crate::db::agent_dispatch::get(conn, "j-ghost-own")?
                    .ok_or_else(|| anyhow::anyhow!("the requeued dispatch is on record"))?;
                Ok((execution_id, job))
            })
            .await
            .unwrap();
        let state = crate::AppState::new_defaults(
            Arc::new(RwLock::new(crate::core::config::default_config())),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let own_key = live_stream(&state, "j-ghost-own");

        let held = super::hold_worker_launch_beside_a_previous_session_with(
            &state,
            &job,
            previous_session_agent_gone,
            previous_session_reap_a_survivor,
        )
        .await
        .is_some();

        assert!(held, "a survivor of its own previous run holds the launch");
        assert!(
            !own_key.is_cancelled(),
            "a token under the dispatch's own id is never cancelled"
        );
        let reason = super::previous_session_alive_reason("j-ghost-own");
        let expected = crate::db::agent_dispatch::runtime_unavailable_marker(1, &reason);
        let job = db
            .with_conn(move |conn| {
                let deferred = crate::db::agent_dispatch::get(conn, "j-ghost-own")?.unwrap();
                assert_eq!(
                    deferred.status,
                    crate::db::agent_dispatch::DispatchStatus::Pending,
                    "held, to be retried"
                );
                assert_eq!(deferred.last_error.as_deref(), Some(expected.as_str()));
                let own = crate::db::worker_processes::get(conn, "j-ghost-own")?
                    .expect("the recorded launch");
                assert!(own.stopped_at.is_none(), "a survivor leaves it unsettled");
                // The next scanner pass: the deferred dispatch is due again.
                conn.execute(
                    "UPDATE agent_dispatch_jobs SET available_at = ?1 WHERE id = 'j-ghost-own'",
                    [chrono::Utc::now().to_rfc3339()],
                )?;
                crate::db::agent_dispatch::claim(conn, "j-ghost-own")?
                    .ok_or_else(|| anyhow::anyhow!("the deferred dispatch is claimable"))
            })
            .await
            .unwrap();

        let held = super::hold_worker_launch_beside_a_previous_session_with(
            &state,
            &job,
            previous_session_agent_gone,
            previous_session_reap_nothing,
        )
        .await
        .is_some();

        assert!(!held, "nothing is left of the previous run");
        assert!(!own_key.is_cancelled());
        db.with_conn(move |conn| {
            let launched = crate::db::agent_dispatch::get(conn, "j-ghost-own")?.unwrap();
            assert_eq!(
                launched.status,
                crate::db::agent_dispatch::DispatchStatus::Running,
                "the dispatch goes on to its launch"
            );
            let own = crate::db::worker_processes::get(conn, "j-ghost-own")?
                .expect("the recorded launch");
            assert!(own.stopped_at.is_some(), "the previous run is settled");
            let events = worker_stop_events(conn, &execution_id)?;
            assert_eq!(events.len(), 2, "{events:?}");
            assert_eq!(events[0]["survivors"], serde_json::json!([4242]));
            assert_eq!(events[1]["survivors"], serde_json::json!([]));
            Ok(())
        })
        .await
        .unwrap();
    }

    /// KT-2021 — a worker woken in its room adopts its interrupted execution
    /// after the launch hold ran. While a previous session of that execution is
    /// still alive the turn is not started beside it: it ends as unavailable
    /// with the hold's own reason, which the dispatcher retries and counts.
    #[tokio::test]
    async fn a_woken_worker_is_not_started_beside_a_previous_session_of_its_execution() {
        use crate::api::discussions::streaming::AgentExecutionOutcome;
        use axum::response::IntoResponse;
        // No working directory can be prepared for this id: a turn that is not
        // held stops at that preflight, before any agent is started.
        const CHILD: &str = "invalid/woken-child";
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let execution_id = db
            .with_conn(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                for (id, title) in [("d-woken-parent", "Parent"), (CHILD, "Child")] {
                    conn.execute(
                        "INSERT INTO discussions
                         (id, title, agent, language, created_at, updated_at, awaiting_agent)
                         VALUES (?1, ?2, 'Codex', 'en', ?3, ?3, 1)",
                        rusqlite::params![id, title, now],
                    )?;
                }
                conn.execute(
                    "INSERT INTO planning_tasks
                     (id, task_number, title, status, created_at, updated_at)
                     VALUES ('t-woken', 9905, 'Woken', 'todo', ?1, ?1)",
                    [&now],
                )?;
                let mut launch =
                    crate::models::LaunchSingleTaskInput::new("t-woken", "d-woken-parent");
                launch.worker_target_kind = Some(crate::models::MessageTargetKind::Agent);
                launch.worker_agent_type = Some("Codex".into());
                let execution =
                    crate::db::orchestration::launch_single_task(conn, &launch, &backend_actor())?
                        .execution;
                for status in [
                    crate::models::TaskExecutionStatus::Provisioning,
                    crate::models::TaskExecutionStatus::Working,
                ] {
                    crate::db::orchestration::transition_execution(
                        conn,
                        &execution.id,
                        status,
                        &backend_actor(),
                        serde_json::json!({}),
                    )?;
                }
                crate::db::orchestration::set_execution_sub_discussion(conn, &execution.id, CHILD)?;
                claimed_dispatch_in(conn, CHILD, "j-woken-old", &crate::models::AgentType::Codex)?;
                crate::db::orchestration::attach_execution_dispatch(
                    conn,
                    &execution.id,
                    "j-woken-old",
                )?;
                crate::db::worker_processes::record_launch(
                    conn,
                    "j-woken-old",
                    &execution.id,
                    crate::core::dispatch_reaper::instance_id(),
                )?;
                // Its turn ended without a delivery: the execution is parked
                // for a resume, which waking the worker in its room is.
                let settled = persist_dispatch_settlement(
                    conn,
                    "j-woken-old",
                    CHILD,
                    None,
                    crate::db::workflows::BatchChildOutcome::Failed,
                    Some("the agent ended without a delivery"),
                    None,
                )?;
                anyhow::ensure!(settled.changed, "the previous dispatch settles");
                claimed_dispatch_in(conn, CHILD, "j-woken-new", &crate::models::AgentType::Codex)?;
                Ok(execution.id)
            })
            .await
            .unwrap();
        let state = crate::AppState::new_defaults(
            Arc::new(RwLock::new(crate::core::config::default_config())),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        // The previous session still streams: nothing has to be signalled here.
        let previous_stream = live_stream(&state, "j-woken-old");

        let (tx, rx) = tokio::sync::oneshot::channel();
        let response = crate::api::discussions::streaming::make_agent_stream_inner(
            state.clone(),
            CHILD.into(),
            None,
            None,
            Some("j-woken-new".into()),
            None,
            Some(tx),
        )
        .await
        .into_response();
        let _ = axum::body::to_bytes(response.into_body(), 64 * 1024).await;

        assert_eq!(
            rx.await.unwrap(),
            AgentExecutionOutcome::RuntimeUnavailable {
                reason: super::previous_session_alive_reason("j-woken-old"),
            },
            "the woken turn must not start beside the previous session"
        );
        assert!(
            previous_stream.is_cancelled(),
            "the previous session is told to end"
        );
        db.with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                .expect("execution must remain queryable");
            assert_eq!(
                execution.status,
                crate::models::TaskExecutionStatus::Working,
                "the woken dispatch adopted the execution"
            );
            assert_eq!(execution.dispatch_job_id.as_deref(), Some("j-woken-new"));
            Ok(())
        })
        .await
        .unwrap();
    }

    /// KT-2021 — the notice of a launch given up beside a previous session
    /// names that session, what is left of it and the remedy, in both
    /// languages, and never tells anyone to reassign.
    #[test]
    fn the_previous_session_notice_says_which_session_and_what_to_do() {
        let (fr, en) = super::previous_session_alive_notice(
            "j-new",
            "exec-1",
            "j-old",
            "Codex",
            Some(11),
            &[4242, 4243],
        );
        for notice in [&fr, &en] {
            for said in [
                "`j-new`",
                "`exec-1`",
                "`j-old`",
                "`4242`, `4243`",
                "`task_exec_resume`",
                "11",
            ] {
                assert!(notice.contains(said), "{said} missing in {notice}");
            }
        }
        assert!(fr.contains("Arrêtez les processus"), "{fr}");
        assert!(fr.contains("reprenez l'exécution"), "{fr}");
        assert!(fr.contains("la réaffecter ne change rien"), "{fr}");
        assert!(en.contains("Stop the processes"), "{en}");
        assert!(en.contains("reassigning it changes nothing"), "{en}");

        // A session that could not be looked at left no pid to name.
        let (fr, en) =
            super::previous_session_alive_notice("j-new", "exec-1", "j-old", "Codex", None, &[]);
        assert!(fr.contains("Aucun identifiant de processus"), "{fr}");
        assert!(en.contains("No process id was recorded"), "{en}");
        assert!(!fr.contains("essais") && !en.contains("tries"));

        assert_eq!(
            super::previous_session_of_hold(&super::previous_session_alive_reason("j-old")),
            Some("j-old")
        );
        assert_eq!(
            super::previous_session_of_hold("Binary 'codex' not found"),
            None
        );
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
                    token_declared: false,
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

    fn backend_actor() -> crate::models::PlanningActor {
        crate::models::PlanningActor {
            kind: crate::models::PlanningActorKind::Backend,
            id: Some("orchestrator".into()),
            session_id: None,
            source_message_id: None,
            token_declared: false,
        }
    }

    /// A `Working` execution of a new task, in rooms `d-parent` / `d-child`,
    /// whose worker ran the claimed dispatch `job` and answered `reply`.
    fn working_worker(
        conn: &rusqlite::Connection,
        task_id: &str,
        job: &str,
        reply: &str,
    ) -> anyhow::Result<String> {
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
             VALUES (?1, 9902, 'Refused', 'todo', ?2, ?2)",
            rusqlite::params![task_id, now],
        )?;
        let actor = backend_actor();
        let execution = crate::db::orchestration::launch_single_task(
            conn,
            &crate::models::LaunchSingleTaskInput::new(task_id, "d-parent"),
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
        let asked = format!("u-{job}");
        conn.execute(
            "INSERT INTO messages
             (id, discussion_id, role, content, timestamp, sort_order, received_at)
             VALUES (?1, 'd-child', 'User', 'go', ?2, 1, ?2)",
            rusqlite::params![asked, now],
        )?;
        conn.execute(
            "INSERT INTO messages
             (id, discussion_id, role, content, timestamp, sort_order, received_at)
             VALUES (?1, 'd-child', 'Agent', ?2, ?3, 2, ?3)",
            rusqlite::params![format!("a-{job}"), reply, now],
        )?;
        crate::db::agent_dispatch::enqueue_for_latest_user(
            conn,
            crate::db::agent_dispatch::NewLatestUserDispatch {
                id: job,
                discussion_id: "d-child",
                dedupe_key: &format!("message:{asked}"),
                agent_override: Some(&crate::models::AgentType::Ollama),
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )?;
        crate::db::agent_dispatch::claim(conn, job)?;
        crate::db::orchestration::attach_execution_dispatch(conn, &execution.id, job)?;
        Ok(execution.id)
    }

    /// KT-1992 — a worker whose empty hand-back was refused and that then ends
    /// its turn is parked, and the principal reads the worker's reason in the room.
    #[tokio::test]
    async fn a_handed_back_worker_is_parked_with_its_reason_in_the_room() {
        const HAND_BACK: &str = "nothing to review: «the build tools are missing here» (the worker's reason). HEAD is still the execution's base `c0af606630` and none of the 2 Definition of Done items is met: recorded as a hand-back, not a delivery. The worker must not call `task_exec_deliver` again before it has committed work or met a DoD item; it ends its turn with one paragraph saying what blocks it or what it found, then Kronn parks the execution and tells the principal, who can resume or reassign it.";
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let execution_id = db
            .with_conn(|conn| {
                let execution_id = working_worker(
                    conn,
                    "t-handed-back",
                    "j-handed-back",
                    "I cannot do this task here.",
                )?;
                crate::db::orchestration::record_delivery_refusal(
                    conn,
                    &execution_id,
                    0,
                    HAND_BACK,
                    &backend_actor(),
                )?;
                Ok(execution_id)
            })
            .await
            .unwrap();

        let settled = db
            .with_conn(|conn| {
                persist_dispatch_settlement(
                    conn,
                    "j-handed-back",
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

        let (status, notice) = db
            .with_conn(move |conn| {
                let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                    .expect("execution must remain queryable");
                let notice = conn.query_row(
                    "SELECT content FROM messages WHERE id = ?1 AND discussion_id = 'd-parent'",
                    [format!(
                        "orch-worker-undelivered:{execution_id}:j-handed-back"
                    )],
                    |row| row.get::<_, String>(0),
                )?;
                Ok((execution.status, notice))
            })
            .await
            .unwrap();
        assert_eq!(status, crate::models::TaskExecutionStatus::Interrupted);
        assert!(notice.contains("nothing to review"), "{notice}");
        assert!(
            notice.contains("«the build tools are missing here»"),
            "{notice}"
        );
        assert!(notice.contains("task_exec_resume"), "{notice}");
    }

    /// KT-1541 — through the real settlement: a worker that ends its turn on a
    /// refused edit without delivering leaves the principal a notice that names
    /// the refusal, not just "no delivery".
    #[tokio::test]
    async fn the_ended_without_delivery_notice_includes_the_refused_tool_calls() {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let execution_id = db
            .with_conn(|conn| {
                working_worker(
                    conn,
                    "t-refused",
                    "j-refused",
                    &format!("Editing.\n> ⚠ Tool call refused: {REFUSAL}\n"),
                )
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
                    [format!("orch-worker-undelivered:{execution_id}:j-refused")],
                    |row| row.get::<_, String>(0),
                )?)
            })
            .await
            .unwrap();
        assert!(notice.contains("sans livraison durable"), "{notice}");
        assert!(notice.contains(REFUSAL), "{notice}");
    }
}

#[cfg(test)]
#[path = "runtime_quota_tests.rs"]
mod quota_turn_tests;

#[cfg(test)]
#[path = "runtime_transient_upstream_tests.rs"]
mod transient_upstream_tests;

#[cfg(test)]
#[path = "runtime_time_limit_tests.rs"]
mod time_limit_tests;
