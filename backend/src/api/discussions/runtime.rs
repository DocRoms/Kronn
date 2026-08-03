// Durable agent-dispatch runtime. Every detached run is represented by an
// `agent_dispatch_jobs` row before execution, claimed atomically, and only
// completed after the agent's background stream has actually terminated.

use crate::AppState;

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

struct DispatchSettlement {
    changed: bool,
    batch_run: Option<crate::models::WorkflowRun>,
}

/// Persist a dispatch terminal state and its parent batch progress as one
/// atomic unit. If either write fails, the job remains runnable and recovery
/// can retry without leaving a batch permanently stuck at n-1/N.
fn persist_dispatch_settlement(
    conn: &rusqlite::Connection,
    job_id: &str,
    discussion_id: &str,
    group_id: Option<&str>,
    child_succeeded: bool,
    error: Option<&str>,
) -> anyhow::Result<DispatchSettlement> {
    let transaction = conn.unchecked_transaction()?;
    let changed = if let Some(error) = error {
        crate::db::agent_dispatch::mark_failed(&transaction, job_id, error)?
    } else {
        crate::db::agent_dispatch::mark_completed(&transaction, job_id)?
    };
    let mut batch_run = None;
    if changed {
        let still_awaiting =
            crate::db::agent_dispatch::has_active_for_discussion(&transaction, discussion_id)?;
        crate::db::discussions::set_awaiting_agent(&transaction, discussion_id, still_awaiting)?;
        if let Some(run_id) = group_id {
            batch_run = crate::db::workflows::increment_batch_progress(
                &transaction,
                run_id,
                child_succeeded,
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
    let job = match state
        .db
        .with_conn(move |conn| crate::db::agent_dispatch::claim(conn, &claim_id))
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
    if let Some(ref run_id) = job.group_id {
        let _ = state
            .ws_broadcast
            .send(crate::models::WsMessage::BatchRunChildStarted {
                run_id: run_id.clone(),
                discussion_id: job.discussion_id.clone(),
            });
    }

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

    let power_lease = crate::core::power_guard::acquire();
    let (stream, completion) = match initial_event {
        Some(event) => {
            make_agent_stream_tracked_with_initial_event(
                state.clone(),
                job.discussion_id.clone(),
                job.agent_override.clone(),
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
            AgentExecutionOutcome::PreflightFailed => {
                fail_dispatch_job(&state, &job, "agent execution preflight failed").await;
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
                fail_dispatch_job(&state, &job, "agent finished without a durable reply").await
            }
            Err(error) => {
                fail_dispatch_job(&state, &job, &format!("reply lookup failed: {error}")).await
            }
        }
    });
    handoff_guard.disarm();
    Some(stream)
}

async fn finish_dispatch_turn(
    state: &AppState,
    job: crate::db::agent_dispatch::AgentDispatchJob,
    response: String,
    execution_succeeded: bool,
) {
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
                crate::db::discussions::delete_last_agent_messages(
                    &transaction,
                    &retry_discussion_id,
                )?;
                crate::db::agent_dispatch::retry_after(
                    &transaction,
                    &retry_id,
                    5,
                    "silent_agent_crash",
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

    if !execution_succeeded {
        fail_dispatch_job(state, &job, "agent reported an unsuccessful completion").await;
        return;
    }

    if let Some(qp_id) = job.chain_prompt_ids.get(job.next_chain_index) {
        let lookup_id = qp_id.clone();
        let qp = state
            .db
            .with_conn(move |conn| crate::db::quick_prompts::get_quick_prompt(conn, &lookup_id))
            .await;
        let qp = match qp {
            Ok(Some(qp)) => qp,
            Ok(None) => {
                fail_dispatch_job(state, &job, &format!("chain QP '{qp_id}' not found")).await;
                return;
            }
            Err(error) => {
                fail_dispatch_job(state, &job, &format!("chain QP lookup failed: {error}")).await;
                return;
            }
        };
        let message = crate::models::DiscussionMessage {
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
                true,
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

async fn fail_dispatch_job(
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    error: &str,
) {
    tracing::error!("Agent dispatch {} failed: {error}", job.id);
    let fail_id = job.id.clone();
    let fail_discussion_id = job.discussion_id.clone();
    let fail_group_id = job.group_id.clone();
    let fail_error = error.to_string();
    let failed = state
        .db
        .with_conn(move |conn| {
            persist_dispatch_settlement(
                conn,
                &fail_id,
                &fail_discussion_id,
                fail_group_id.as_deref(),
                false,
                Some(&fail_error),
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
    state.agent_dispatch_notify.notify_waiters();
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

#[cfg(test)]
mod chain_render_tests {
    use super::{persist_dispatch_settlement, render_chain_qp_prompt, DispatchHandoffGuard};
    use std::sync::Arc;
    use tokio::sync::RwLock;

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
                    batch_name: Some("Atomic batch".into()),
                    parent_run_id: None,
                    state: std::collections::HashMap::new(),
                    produced_branches: vec![],
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
                    true,
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
                    true,
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
}
