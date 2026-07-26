// HTTP-facing endpoints that drive an agent: send_message (user
// types something → agent runs), run_agent (re-fire on existing
// thread), dismiss_partial (wipe a dangling boot-recovered partial),
// stop_agent (cancel a running agent via the cancel registry).
//
// All four either delegate to `super::streaming::make_agent_stream`
// or touch `state.cancel_registry` — they're the thin glue between
// the route layer and the streaming/runtime modules.

use std::convert::Infallible;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

use super::streaming::make_agent_stream;
use super::{SseStream, MAX_CONTENT_LEN};

fn sse_events(events: Vec<Event>) -> Sse<SseStream> {
    let stream: SseStream = Box::pin(futures::stream::iter(
        events.into_iter().map(Ok::<_, Infallible>),
    ));
    Sse::new(stream)
}

fn accepted_event(message_id: &str, sort_order: i64, duplicate: bool) -> Event {
    Event::default().event("accepted").data(
        serde_json::json!({
            "message_id": message_id,
            "sort_order": sort_order,
            "duplicate": duplicate,
        })
        .to_string(),
    )
}

async fn local_agent_policy(
    state: &AppState,
    discussion_id: &str,
    target_agent: Option<&AgentType>,
) -> (bool, i64) {
    let no_agent_id = discussion_id.to_string();
    let no_agent = state
        .db
        .with_conn(move |conn| crate::db::discussions::disc_is_no_agent(conn, &no_agent_id))
        .await
        .unwrap_or(false);
    if no_agent {
        return (true, 0);
    }

    let live_check_id = discussion_id.to_string();
    let live_target = target_agent.map(|agent| format!("{agent:?}"));
    let live_agents = match state
        .db
        .with_conn(move |conn| match live_target {
            Some(agent) => crate::db::discussion_sessions::count_live_participants_for_agent(
                conn,
                &live_check_id,
                &agent,
            ),
            None => crate::db::discussion_sessions::count_live_participants(conn, &live_check_id),
        })
        .await
    {
        Ok(count) => count,
        Err(error) => {
            tracing::warn!(
                "send_message: count_live_participants failed for disc {discussion_id}, \
                 falling back to local runner: {error}"
            );
            0
        }
    };
    (false, live_agents)
}

/// GET /api/discussions/running — disc ids with an in-flight agent run RIGHT
/// NOW, server-side. Source of truth is the cancel registry: every agent run
/// registers a `CancelGuard` there for its entire lifetime (removed on ANY
/// exit — completion, error, timeout, cancel), so its keys are exactly the
/// currently-running discussions, foreground OR background/batch. Page-
/// independent: the frontend polls this so a run still working after you
/// navigate away keeps showing as running, instead of looking dead and
/// tempting a needless re-launch (2026-06-24).
pub async fn running_discussions(State(state): State<AppState>) -> Json<ApiResponse<Vec<String>>> {
    let ids: Vec<String> = match state.cancel_registry.lock() {
        Ok(reg) => reg.keys().cloned().collect(),
        Err(poisoned) => poisoned.into_inner().keys().cloned().collect(),
    };
    Json(ApiResponse::ok(ids))
}

/// POST /api/discussions/:id/messages
pub async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SendMessageRequest>,
) -> Sse<SseStream> {
    // Input validation
    if req.content.len() > MAX_CONTENT_LEN {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(
                Event::default()
                    .event("error")
                    .data(serde_json::json!({ "error": "Message too long" }).to_string()),
            )
        }));
        return Sse::new(stream);
    }

    let message_id = match req.client_message_id.as_deref() {
        Some(raw) => match Uuid::parse_str(raw) {
            Ok(id) => id.to_string(),
            Err(_) => {
                return sse_events(vec![Event::default().event("error").data(
                    serde_json::json!({ "error": "Invalid client_message_id" }).to_string(),
                )]);
            }
        },
        None => Uuid::new_v4().to_string(),
    };

    let target = req.target_agent.clone();
    // Resolve responder ownership before the acceptance transaction so human-
    // only/shared rooms never expose a briefly-runnable local dispatch job.
    let (no_agent, live_agents) = local_agent_policy(&state, &id, target.as_ref()).await;
    let needs_local_dispatch = !no_agent && live_agents == 0;

    // Read user identity from config for message attribution
    let (author_pseudo, author_avatar_email) = {
        let config = state.config.read().await;
        (
            config.server.pseudo.clone(),
            config.server.avatar_email.clone(),
        )
    };

    // Add user message to DB
    let user_msg = DiscussionMessage {
        model: None,
        lint_report: None,
        id: message_id.clone(),
        role: MessageRole::User,
        content: req.content.clone(),
        agent_type: None,
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo,
        author_avatar_email,
        source_msg_id: None,
        duration_ms: None,
    };
    let disc_id = id.clone();
    let msg = user_msg;
    let target_clone = target.clone();
    let insert_job_id = Uuid::new_v4().to_string();

    let insert_outcome = match state
        .db
        .with_conn(move |conn| {
            let outcome = if needs_local_dispatch {
                crate::db::discussions::insert_user_message_with_dispatch(
                    conn,
                    &disc_id,
                    &msg,
                    &insert_job_id,
                    target_clone.as_ref(),
                )?
            } else {
                crate::db::discussions::insert_user_message_with_agent_handoff(
                    conn, &disc_id, &msg,
                )?
            };
            if matches!(
                &outcome,
                crate::db::discussions::InsertUserMessageOutcome::Inserted { .. }
            ) {
                // Pin any files the user staged in the composer to THIS message (0.8.8),
                // so they render in its bubble and clear from the input. Non-fatal: a
                // link failure must not drop the message the user just sent.
                if let Err(e) = crate::db::discussions::link_pending_context_files_to_message(
                    conn, &disc_id, &msg.id,
                ) {
                    tracing::warn!(
                        "Failed to link pending context files to message {}: {e}",
                        msg.id
                    );
                }
                // Track new participant
                if let Some(ref t) = target_clone {
                    let disc = crate::db::discussions::get_discussion(conn, &disc_id)?;
                    if let Some(d) = disc {
                        if !d.participants.contains(t) {
                            let mut participants = d.participants;
                            participants.push(t.clone());
                            crate::db::discussions::update_discussion_participants(
                                conn,
                                &disc_id,
                                &participants,
                            )?;
                        }
                    }
                }
            }
            Ok(outcome)
        })
        .await
    {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::error!("Failed to save user message: {e}");
            let stream: SseStream = Box::pin(futures::stream::once(async move {
                Ok::<_, Infallible>(
                    Event::default()
                        .event("error")
                        .data(serde_json::json!({ "error": "Failed to save message" }).to_string()),
                )
            }));
            return Sse::new(stream);
        }
    };

    let (stored_user_msg, sort_order, claimed_dispatch) = match insert_outcome {
        crate::db::discussions::InsertUserMessageOutcome::Inserted {
            message,
            sort_order,
            dispatch_job,
        } => (*message, sort_order, dispatch_job.map(|job| *job)),
        crate::db::discussions::InsertUserMessageOutcome::Duplicate { sort_order } => {
            return sse_events(vec![accepted_event(&message_id, sort_order, true)]);
        }
        crate::db::discussions::InsertUserMessageOutcome::PartialPending => {
            return sse_events(vec![Event::default().event("error").data(
                serde_json::json!({
                    "error": "partial_pending",
                    "message": "Une réponse d'agent précédente est en cours de récupération. Patientez ou fermez la notification de récupération avant de renvoyer."
                }).to_string(),
            )]);
        }
    };
    let accepted = accepted_event(&message_id, sort_order, false);

    // F9 — human-only disc: never spawn an agent. Persist + federate the human
    // message (done above) and stop. Guarantees true human↔human chat even on
    // an instance that has an agent installed.
    if no_agent {
        crate::api::federation::federate_message(&state, &id, &stored_user_msg).await;
        let payload = serde_json::json!({ "skipped": true, "reason": "no_agent" }).to_string();
        return sse_events(vec![
            accepted,
            Event::default().event("skipped_no_agent").data(payload),
        ]);
    }

    // Double-responder guard (2026-06-04, flagged by Romuald; made
    // presence-sticky 2026-06-08) — for an untargeted message, any active MCP
    // agent answers; for `@agent`, only an active session of that concrete
    // target suppresses the local runner. This lets an absent mentioned agent
    // join via the normal durable dispatch even while another peer is present.
    // Spawning the local runner too made BOTH reply to the same message
    // (reproduced on disc ca495847: Kronn's native reply + the CLI peer's
    // MCP reply to one user turn). The user message is already persisted
    // + broadcast above, so the connected agent picks it up — we simply
    // don't spawn. Emit one informational SSE event and let the stream end:
    // `parseSSEStream` fires onDone on stream-close, so the frontend's
    // "sending" state clears with no empty agent bubble (the peer's reply
    // arrives separately via the disc message list / WS).
    //
    // PRESENCE-STICKY: `count_live_participants` counts any 'active' session
    // regardless of how long ago it last heartbeated — a turn-based CLI peer
    // idles minutes between human turns and must NOT be judged dead (the old
    // 300s window was the double-responder bug). Crashed-peer escape hatch:
    // `run_agent` (/run) is unguarded, so the user forces a Kronn reply with
    // one click; and abandoned sessions (idle > 24h) are reaped at boot.
    // `paused` agents are NOT counted (they won't reply → Kronn answers).
    if live_agents > 0 {
        crate::api::federation::federate_message(&state, &id, &stored_user_msg).await;
        tracing::info!(
            "send_message: {live_agents} live MCP agent(s) on disc {id} — skipping local runner (connected agents respond)"
        );
        let payload = serde_json::json!({
            "skipped": true,
            "reason": "live_mcp_agents",
            "live_agents": live_agents,
        })
        .to_string();
        return sse_events(vec![
            accepted,
            Event::default().event("skipped_live_agents").data(payload),
        ]);
    }

    let stream = match claimed_dispatch {
        Some(job) => {
            super::runtime::stream_claimed_dispatch_job(state.clone(), job, Some(accepted)).await
        }
        None => None,
    };
    // Start/track the local durable run before awaiting federation I/O: once
    // the DB transaction returned a Running claim, no slow peer can strand it
    // between claim and execution.
    crate::api::federation::federate_message(&state, &id, &stored_user_msg).await;
    state.agent_dispatch_notify.notify_one();
    stream.unwrap_or_else(|| sse_events(vec![accepted_event(&message_id, sort_order, false)]))
}

/// POST /api/discussions/:id/messages/revise
///
/// Atomic edit/resend: CAS-edit the target User message, archive the trailing
/// reply projection, emit a fresh cursor-visible revision event and create
/// (or intentionally omit) the durable dispatch obligation in one transaction.
pub async fn revise_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ReviseMessageRequest>,
) -> Response {
    if request.content.trim().is_empty() || request.content.len() > MAX_CONTENT_LEN {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err("Invalid revised message content")),
        )
            .into_response();
    }
    if Uuid::parse_str(&request.idempotency_key).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err("Invalid idempotency_key")),
        )
            .into_response();
    }

    let (no_agent, live_agents) =
        local_agent_policy(&state, &id, request.target_agent.as_ref()).await;
    let needs_local_dispatch = !no_agent && live_agents == 0;
    let dispatch_job_id = Uuid::new_v4().to_string();
    let db_request = request.clone();
    let db_discussion_id = id.clone();
    let outcome = state
        .db
        .with_conn(move |connection| {
            Ok(crate::db::discussions::revise_message_with_dispatch(
                connection,
                crate::db::discussions::ReviseMessageParams {
                    discussion_id: &db_discussion_id,
                    message_id: &db_request.message_id,
                    content: db_request.content.trim(),
                    expected_revision: &db_request.expected_revision,
                    idempotency_key: &db_request.idempotency_key,
                    target_agent: db_request.target_agent.as_ref(),
                    needs_local_dispatch,
                    dispatch_job_id: &dispatch_job_id,
                },
            ))
        })
        .await;

    let outcome = match outcome {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(crate::db::discussions::ReviseMessageError::Conflict { current_revision })) => {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::<serde_json::Value>::err_coded(
                    ApiErrorCode::Conflict,
                    serde_json::json!({
                        "error": "revision_conflict",
                        "current_revision": current_revision,
                    })
                    .to_string(),
                )),
            )
                .into_response();
        }
        Ok(Err(crate::db::discussions::ReviseMessageError::IdempotencyConflict)) => {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::<()>::err_coded(
                    ApiErrorCode::Conflict,
                    "Idempotency key already belongs to a different revision",
                )),
            )
                .into_response();
        }
        Ok(Err(crate::db::discussions::ReviseMessageError::DispatchInProgress)) => {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::<()>::err_coded(
                    ApiErrorCode::Conflict,
                    "An agent response is already in progress",
                )),
            )
                .into_response();
        }
        Ok(Err(crate::db::discussions::ReviseMessageError::NotFound)) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<()>::err_coded(
                    ApiErrorCode::NotFound,
                    "Editable user message not found",
                )),
            )
                .into_response();
        }
        Ok(Err(error)) => {
            tracing::error!("Atomic message revision failed for {id}: {error}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err("Failed to revise message")),
            )
                .into_response();
        }
        Err(error) => {
            tracing::error!("Atomic message revision DB task failed for {id}: {error}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err("Failed to revise message")),
            )
                .into_response();
        }
    };

    let revision_event = Event::default()
        .event("message_revised")
        .data(serde_json::to_string(&outcome.receipt).unwrap_or_else(|_| "{}".into()));
    if !outcome.receipt.duplicate {
        crate::api::federation::federate_message_revision(&state, &outcome.event).await;
    }

    if no_agent || live_agents > 0 || outcome.receipt.duplicate {
        return sse_events(vec![revision_event]).into_response();
    }

    let stream = match outcome.claimed_dispatch {
        Some(job) => {
            super::runtime::stream_claimed_dispatch_job(state.clone(), job, Some(revision_event))
                .await
        }
        None => None,
    };
    state.agent_dispatch_notify.notify_one();
    stream
        .unwrap_or_else(|| sse_events(Vec::new()))
        .into_response()
}

/// POST /api/discussions/:id/run
pub async fn run_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Option<Json<RunAgentRequest>>,
) -> Sse<SseStream> {
    let job_id = Uuid::new_v4().to_string();
    let enqueue_id = job_id.clone();
    let enqueue_discussion_id = id.clone();
    let requested_key = request
        .and_then(|Json(request)| request.idempotency_key)
        .filter(|key| !key.trim().is_empty());
    if requested_key
        .as_ref()
        .is_some_and(|key| Uuid::parse_str(key).is_err())
    {
        return sse_events(vec![Event::default().event("error").data(
            serde_json::json!({ "error": "Invalid idempotency_key" }).to_string(),
        )]);
    }
    let dedupe_key = format!(
        "force:{}",
        requested_key.as_deref().unwrap_or(job_id.as_str())
    );
    let job = state
        .db
        .with_conn(move |conn| {
            if let Some(active) =
                crate::db::agent_dispatch::find_active_for_discussion(conn, &enqueue_discussion_id)?
            {
                return Ok(Some(active));
            }
            let has_user_message: bool = conn.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM messages
                    WHERE discussion_id = ?1 AND role = 'User'
                )",
                [&enqueue_discussion_id],
                |row| row.get(0),
            )?;
            if !has_user_message {
                return Ok(None);
            }
            let transaction = conn.unchecked_transaction()?;
            let job = crate::db::agent_dispatch::enqueue_for_latest_user(
                &transaction,
                crate::db::agent_dispatch::NewLatestUserDispatch {
                    id: &enqueue_id,
                    discussion_id: &enqueue_discussion_id,
                    dedupe_key: &dedupe_key,
                    agent_override: None,
                    chain_prompt_ids: &[],
                    batch_item: None,
                    group_id: None,
                    group_concurrency_limit: None,
                },
            )?;
            crate::db::discussions::set_awaiting_agent(&transaction, &enqueue_discussion_id, true)?;
            transaction.commit()?;
            Ok(Some(job))
        })
        .await;
    let job = match job {
        Ok(Some(job)) => job,
        // Legacy/empty discussions have no User message to anchor a durable
        // obligation. Preserve the force-run behaviour for those rare cases.
        Ok(None) => return make_agent_stream(state, id, None).await,
        Err(error) => {
            tracing::error!("Force-run durable enqueue failed for {id}: {error}");
            return sse_events(vec![Event::default().event("error").data(
                serde_json::json!({ "error": "Failed to queue agent run" }).to_string(),
            )]);
        }
    };
    state.agent_dispatch_notify.notify_one();
    super::runtime::stream_dispatch_job(state, job.id, None)
        .await
        .unwrap_or_else(|| sse_events(Vec::new()))
}

/// POST /api/discussions/:id/dismiss-partial
///
/// Force-recover a pending partial_response on demand. Used by the
/// "Dismiss" button the frontend shows next to the PartialResponseRecovered
/// toast and as a fallback when the WS event missed: calls the same
/// recovery path used at boot, scoped to this one disc.
///
/// Returns `{ recovered: true }` if there was a partial to recover,
/// `{ recovered: false }` if the disc was clean (no-op, idempotent).
pub async fn dismiss_partial(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<serde_json::Value>> {
    let ids = match state
        .db
        .with_conn(move |conn| {
            // Reuses the boot recovery — process-wide (handles every disc with
            // a non-null partial), so a "dismiss" click incidentally cleans up
            // any other dangling partials too. Cheap (one indexed scan).
            crate::db::discussions::recover_partial_responses(conn)
        })
        .await
    {
        Ok(list) => list,
        Err(e) => return Json(ApiResponse::err(format!("Recovery failed: {}", e))),
    };
    let recovered_this = ids.iter().any(|d| d == &id);
    if !ids.is_empty() {
        let _ = state
            .ws_broadcast
            .send(WsMessage::PartialResponseRecovered {
                discussion_ids: ids,
            });
    }
    Json(ApiResponse::ok(
        serde_json::json!({ "recovered": recovered_this }),
    ))
}

/// POST /api/discussions/:id/stop
///
/// Abort the currently-running agent for this discussion. Triggers the
/// disc's cancellation token if one is registered in `state.cancel_registry`.
/// The agent task's `select!` picks up the cancellation, kills the spawned
/// child process, saves a partial response with an "⏹️ Interrompu" footer,
/// and broadcasts `batch_run_progress` if the disc was part of a batch.
///
/// Returns `{ cancelled: true }` if a token was registered and triggered,
/// `{ cancelled: false }` if nothing was running (agent already finished,
/// disc never started, race with backend restart, etc.) — which lets the
/// frontend show a "Rien à arrêter" toast rather than fake-confirming.
pub async fn stop_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<serde_json::Value>> {
    let cancelled_process = {
        let mut map = match state.cancel_registry.lock() {
            Ok(m) => m,
            Err(_) => return Json(ApiResponse::err("Cancel registry poisoned")),
        };
        if let Some(token) = map.remove(&id) {
            token.cancel();
            true
        } else {
            false
        }
    };
    let cancel_id = id.clone();
    let cancelled_job = state
        .db
        .with_conn(move |conn| {
            let transaction = conn.unchecked_transaction()?;
            let active =
                crate::db::agent_dispatch::find_active_for_discussion(&transaction, &cancel_id)?;
            let count = crate::db::agent_dispatch::cancel_for_discussion(&transaction, &cancel_id)?;
            let mut batch_run = None;
            if count > 0 {
                let still_awaiting =
                    crate::db::agent_dispatch::has_active_for_discussion(&transaction, &cancel_id)?;
                crate::db::discussions::set_awaiting_agent(
                    &transaction,
                    &cancel_id,
                    still_awaiting,
                )?;
                if let Some(run_id) = active.as_ref().and_then(|job| job.group_id.as_deref()) {
                    batch_run = crate::db::workflows::increment_batch_progress(
                        &transaction,
                        run_id,
                        false,
                    )?;
                }
            }
            transaction.commit()?;
            Ok((count, batch_run))
        })
        .await;
    let (cancelled_jobs, batch_run) = cancelled_job.unwrap_or_else(|error| {
        tracing::error!("Failed to cancel durable dispatch for {id}: {error}");
        (0, None)
    });
    if let Some(updated_run) = batch_run {
        super::streaming::broadcast_batch_progress(&state, &id, &updated_run);
    }
    let cancelled = cancelled_process || cancelled_jobs > 0;
    state.agent_dispatch_notify.notify_waiters();
    Json(ApiResponse::ok(
        serde_json::json!({ "cancelled": cancelled }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::default_config;
    use crate::db::Database;
    use crate::DEFAULT_MAX_CONCURRENT_AGENTS;
    use axum::response::IntoResponse;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    /// State with one project + one disc, mirroring the disc_invite test
    /// harness. `send_message` is a free function over extractors, so we
    /// drive it directly without spinning up axum.
    async fn make_state_with_disc(disc_id: &str) -> AppState {
        let db = Arc::new(Database::open_in_memory().expect("in-memory DB"));
        let disc_id = disc_id.to_string();
        db.with_conn(move |conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p-test', 'Test', '/tmp', ?1, ?1)",
                rusqlite::params![now],
            )?;
            conn.execute(
                "INSERT INTO discussions (id, project_id, title, created_at, updated_at)
                 VALUES (?1, 'p-test', 'Test disc', ?2, ?2)",
                rusqlite::params![disc_id, now],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let cfg = Arc::new(RwLock::new(default_config()));
        AppState::new_defaults(cfg, db, DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    async fn sse_body_to_string(resp: Sse<SseStream>) -> String {
        let response = resp.into_response();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect SSE body");
        String::from_utf8_lossy(&bytes).to_string()
    }

    async fn response_body_to_string(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect response body");
        String::from_utf8_lossy(&bytes).to_string()
    }

    /// The double-responder fix (2026-06-04): with a LIVE MCP agent on the
    /// disc, send_message must persist the human message + broadcast, then
    /// SKIP the local runner (emit `skipped_live_agents`) so the connected
    /// agent is the sole responder. We assert: (a) the skip event is on the
    /// wire, (b) the User message is persisted, (c) NO Agent reply was added
    /// (the runner never ran).
    #[tokio::test]
    async fn send_message_skips_local_runner_when_live_agent_connected() {
        let disc = "d-live-1";
        let state = make_state_with_disc(disc).await;
        // A live MCP agent is connected (status='active', fresh last_seen
        // from create_session).
        state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::create_session(
                    conn,
                    disc,
                    "Codex",
                    Some("sess-x"),
                    "peer",
                )
            })
            .await
            .unwrap();

        let resp = send_message(
            State(state.clone()),
            Path(disc.to_string()),
            Json(SendMessageRequest {
                content: "hello peers".into(),
                target_agent: None,
                client_message_id: Some("5fa2fc3c-4b92-4472-9729-faba80bf0525".into()),
            }),
        )
        .await;
        let body = sse_body_to_string(resp).await;
        assert!(
            body.contains("skipped_live_agents"),
            "expected skip event, got: {body}"
        );
        assert!(body.contains("live_mcp_agents"), "skip reason present");
        let accepted_pos = body.find("event: accepted").expect("accepted event");
        let skipped_pos = body.find("event: skipped_live_agents").expect("skip event");
        assert!(
            accepted_pos < skipped_pos,
            "accepted receipt must be the first event: {body}"
        );

        // User message persisted, and NO Agent message (runner never ran).
        let msgs = state
            .db
            .with_conn(move |conn| crate::db::discussions::list_messages(conn, disc))
            .await
            .unwrap();
        assert_eq!(msgs.len(), 1, "only the human message is persisted");
        assert_eq!(msgs[0].role, MessageRole::User);
        assert!(
            !msgs.iter().any(|m| m.role == MessageRole::Agent),
            "no agent reply — the connected agent answers, not Kronn's runner"
        );
    }

    #[tokio::test]
    async fn retrying_client_message_id_returns_duplicate_receipt_without_rerun() {
        let disc = "d-idempotent";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::create_session(
                    conn,
                    disc,
                    "Codex",
                    Some("sess-idempotent"),
                    "peer",
                )
            })
            .await
            .unwrap();

        let client_message_id = "e8618b06-f4ce-42c8-9cb2-cbc0062bc995";
        let first = send_message(
            State(state.clone()),
            Path(disc.to_string()),
            Json(SendMessageRequest {
                content: "only once".into(),
                target_agent: None,
                client_message_id: Some(client_message_id.into()),
            }),
        )
        .await;
        let first_body = sse_body_to_string(first).await;
        assert!(first_body.contains(r#""duplicate":false"#));
        assert!(first_body.contains("skipped_live_agents"));

        let retry = send_message(
            State(state.clone()),
            Path(disc.to_string()),
            Json(SendMessageRequest {
                content: "only once".into(),
                target_agent: None,
                client_message_id: Some(client_message_id.into()),
            }),
        )
        .await;
        let retry_body = sse_body_to_string(retry).await;
        assert!(retry_body.contains("event: accepted"));
        assert!(retry_body.contains(r#""duplicate":true"#));
        assert!(
            !retry_body.contains("skipped_live_agents"),
            "duplicate request must stop after the receipt: {retry_body}"
        );

        let messages = state
            .db
            .with_conn(move |conn| crate::db::discussions::list_messages(conn, disc))
            .await
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, client_message_id);
    }

    #[tokio::test]
    async fn revise_message_is_idempotent_and_rejects_divergent_key_reuse() {
        let disc = "d-revise-handler";
        let state = make_state_with_disc(disc).await;
        let message_id = "2878216c-8d4e-4479-96af-5c801b98b1f1";
        state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::create_session(
                    conn,
                    disc,
                    "ClaudeCode",
                    Some("live-reviser"),
                    "peer",
                )?;
                crate::db::discussions::insert_message(
                    conn,
                    disc,
                    &DiscussionMessage {
                        id: message_id.into(),
                        role: MessageRole::User,
                        content: "before".into(),
                        timestamp: Utc::now(),
                        model: None,
                        lint_report: None,
                        agent_type: None,
                        tokens_used: 0,
                        auth_mode: None,
                        model_tier: None,
                        cost_usd: None,
                        author_pseudo: None,
                        author_avatar_email: None,
                        source_msg_id: None,
                        duration_ms: None,
                    },
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let expected_revision = state
            .db
            .with_conn(move |conn| {
                let messages = crate::db::discussions::list_messages(conn, disc)?;
                Ok(messages[0].timestamp.to_rfc3339())
            })
            .await
            .unwrap();
        let idempotency_key = "de60829d-250d-41b1-bb45-632c22c59f7c";
        let request = ReviseMessageRequest {
            message_id: message_id.into(),
            content: "after".into(),
            expected_revision,
            idempotency_key: idempotency_key.into(),
            target_agent: None,
        };

        let first = revise_message(
            State(state.clone()),
            Path(disc.into()),
            Json(request.clone()),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = response_body_to_string(first).await;
        assert!(first_body.contains("event: message_revised"));
        assert!(first_body.contains("\"duplicate\":false"));

        let retry = revise_message(
            State(state.clone()),
            Path(disc.into()),
            Json(request.clone()),
        )
        .await;
        assert_eq!(retry.status(), StatusCode::OK);
        let retry_body = response_body_to_string(retry).await;
        assert!(retry_body.contains("\"duplicate\":true"));

        let conflict = revise_message(
            State(state),
            Path(disc.into()),
            Json(ReviseMessageRequest {
                content: "different payload".into(),
                ..request
            }),
        )
        .await;
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn no_agent_receives_accepted_before_skip() {
        let disc = "d-no-agent";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                conn.execute("UPDATE discussions SET no_agent = 1 WHERE id = ?1", [disc])?;
                Ok(())
            })
            .await
            .unwrap();

        let response = send_message(
            State(state),
            Path(disc.to_string()),
            Json(SendMessageRequest {
                content: "human only".into(),
                target_agent: None,
                client_message_id: Some("6661b620-162d-4a8a-9552-33f0896c6835".into()),
            }),
        )
        .await;
        let body = sse_body_to_string(response).await;
        let accepted_pos = body.find("event: accepted").expect("accepted event");
        let skipped_pos = body
            .find("event: skipped_no_agent")
            .expect("no-agent skip event");
        assert!(
            accepted_pos < skipped_pos,
            "accepted receipt must precede no-agent skip: {body}"
        );
    }

    #[tokio::test]
    async fn accepted_receipt_precedes_agent_preflight_error() {
        let state = make_state_with_disc("existing-disc").await;
        let (response, _completion) =
            super::super::streaming::make_agent_stream_tracked_with_initial_event(
                state,
                "missing-disc".into(),
                None,
                "test-dispatch".into(),
                accepted_event("3d966785-b7b4-446b-8303-ac28f02a5427", 9, false),
            )
            .await;
        let body = sse_body_to_string(response).await;
        let accepted_pos = body.find("event: accepted").expect("accepted event");
        let error_pos = body.find("event: error").expect("preflight error event");
        assert!(
            accepted_pos < error_pos,
            "accepted receipt must precede runner preflight errors: {body}"
        );
    }

    #[tokio::test]
    async fn invalid_client_message_id_is_rejected_without_insert() {
        let disc = "d-invalid-client-id";
        let state = make_state_with_disc(disc).await;
        let response = send_message(
            State(state.clone()),
            Path(disc.to_string()),
            Json(SendMessageRequest {
                content: "must not persist".into(),
                target_agent: None,
                client_message_id: Some("not-a-uuid".into()),
            }),
        )
        .await;
        let body = sse_body_to_string(response).await;
        assert!(body.contains("Invalid client_message_id"));
        assert!(!body.contains("event: accepted"));

        let messages = state
            .db
            .with_conn(move |conn| crate::db::discussions::list_messages(conn, disc))
            .await
            .unwrap();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn stop_cancels_only_the_current_durable_turn() {
        let disc = "d-stop-durable";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                let first = DiscussionMessage {
                    id: "d9158714-19d4-4dbf-9b7d-0839c93458b7".into(),
                    role: MessageRole::User,
                    content: "first".into(),
                    timestamp: Utc::now(),
                    model: None,
                    lint_report: None,
                    agent_type: None,
                    tokens_used: 0,
                    auth_mode: None,
                    model_tier: None,
                    cost_usd: None,
                    author_pseudo: None,
                    author_avatar_email: None,
                    source_msg_id: None,
                    duration_ms: None,
                };
                let second = DiscussionMessage {
                    id: "812ee54a-62d6-4426-bf19-b970416958d7".into(),
                    content: "second".into(),
                    ..first.clone()
                };
                crate::db::discussions::insert_user_message_with_dispatch(
                    conn,
                    disc,
                    &first,
                    "j-running",
                    None,
                )?;
                crate::db::discussions::insert_user_message_with_dispatch(
                    conn,
                    disc,
                    &second,
                    "j-pending",
                    None,
                )?;
                crate::db::workflows::ensure_batch_placeholder_workflow(
                    conn,
                    "qp-stop",
                    "Stop batch",
                    None,
                )?;
                crate::db::workflows::insert_run(
                    conn,
                    &crate::models::WorkflowRun {
                        id: "batch-stop".into(),
                        workflow_id: "qp:qp-stop".into(),
                        status: crate::models::RunStatus::Running,
                        trigger_context: None,
                        step_results: vec![],
                        tokens_used: 0,
                        workspace_path: None,
                        started_at: Utc::now(),
                        finished_at: None,
                        run_type: "batch".into(),
                        batch_total: 2,
                        batch_completed: 0,
                        batch_failed: 0,
                        batch_name: Some("Stop batch".into()),
                        parent_run_id: None,
                        state: std::collections::HashMap::new(),
                        produced_branches: vec![],
                        parent_workflow_id: None,
                        parent_workflow_name: None,
                        parent_run_started_at: None,
                    },
                )?;
                conn.execute(
                    "UPDATE agent_dispatch_jobs
                     SET group_id = 'batch-stop'
                     WHERE id IN ('j-running', 'j-pending')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        state
            .db
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER reject_stop_batch_progress
                     BEFORE UPDATE ON workflow_runs
                     BEGIN
                       SELECT RAISE(ABORT, 'forced cancellation progress failure');
                     END;",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let refused = stop_agent(State(state.clone()), Path(disc.into())).await;
        assert_eq!(
            refused.0.data.unwrap()["cancelled"],
            serde_json::Value::Bool(false),
            "a failed batch update must roll the cancellation back"
        );
        state
            .db
            .with_conn(|conn| {
                let job = crate::db::agent_dispatch::get(conn, "j-running")?.unwrap();
                assert!(
                    matches!(
                        job.status,
                        crate::db::agent_dispatch::DispatchStatus::Pending
                            | crate::db::agent_dispatch::DispatchStatus::Running
                    ),
                    "the job must remain retryable after rollback"
                );
                let batch = crate::db::workflows::get_run(conn, "batch-stop")?.unwrap();
                assert_eq!(batch.batch_failed, 0);
                conn.execute_batch("DROP TRIGGER reject_stop_batch_progress;")?;
                Ok(())
            })
            .await
            .unwrap();

        let response = stop_agent(State(state.clone()), Path(disc.into())).await;
        assert_eq!(
            response.0.data.unwrap()["cancelled"],
            serde_json::Value::Bool(true)
        );
        let (running, pending, awaiting, batch) = state
            .db
            .with_conn(move |conn| {
                let running = crate::db::agent_dispatch::get(conn, "j-running")?.unwrap();
                let pending = crate::db::agent_dispatch::get(conn, "j-pending")?.unwrap();
                let awaiting = conn.query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = ?1",
                    [disc],
                    |row| row.get::<_, bool>(0),
                )?;
                let batch = crate::db::workflows::get_run(conn, "batch-stop")?.unwrap();
                Ok((running, pending, awaiting, batch))
            })
            .await
            .unwrap();
        assert_eq!(
            running.status,
            crate::db::agent_dispatch::DispatchStatus::Cancelled
        );
        assert_eq!(
            pending.status,
            crate::db::agent_dispatch::DispatchStatus::Pending
        );
        assert!(awaiting, "the queued follow-up is still owed");
        assert_eq!(batch.batch_failed, 1);
        assert_eq!(batch.status, crate::models::RunStatus::Running);
    }

    /// A `paused` session is NOT a live responder, so send_message must
    /// NOT skip. We can't drive make_agent_stream (it launches a real CLI)
    /// in a unit test, so we assert the decision input directly: with only
    /// a paused session, count_live_participants is 0 → the guard is not
    /// taken. (The skip-path behaviour itself is covered above.)
    #[tokio::test]
    async fn send_message_does_not_skip_when_only_paused_agent() {
        let disc = "d-paused-1";
        let state = make_state_with_disc(disc).await;
        let pk = state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::create_session(
                    conn,
                    disc,
                    "Codex",
                    Some("sess-p"),
                    "peer",
                )
            })
            .await
            .unwrap();
        state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::set_session_status(conn, pk, "paused")
            })
            .await
            .unwrap();

        let live = state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::count_live_participants(conn, disc)
            })
            .await
            .unwrap();
        assert_eq!(
            live, 0,
            "paused agent is not a live responder → Kronn would still answer"
        );
    }

    #[tokio::test]
    async fn targeted_policy_ignores_other_live_agent_types() {
        let disc = "d-targeted-policy";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::create_session(
                    conn,
                    disc,
                    "ClaudeCode",
                    Some("claude-live"),
                    "owner",
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let (_, untargeted_live) = local_agent_policy(&state, disc, None).await;
        let (_, codex_live) = local_agent_policy(&state, disc, Some(&AgentType::Codex)).await;
        let (_, claude_live) = local_agent_policy(&state, disc, Some(&AgentType::ClaudeCode)).await;

        assert_eq!(untargeted_live, 1);
        assert_eq!(
            codex_live, 0,
            "a live Claude session must not swallow a targeted Codex run"
        );
        assert_eq!(
            claude_live, 1,
            "an already-live target must still prevent a duplicate runner"
        );
    }

    /// A run that dies in make_agent_stream's
    /// preflight (here: Isolated disc whose worktree re-lock fails because
    /// the project path isn't a git repo) must NOT leave awaiting_agent=1,
    /// or the next boot reconcile appends a bogus interruption notice.
    /// The marker is set after every preflight early-return, so this path
    /// never touches it.
    #[tokio::test]
    async fn run_agent_preflight_failure_leaves_no_awaiting_marker() {
        let disc = "d-relock-fail";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "UPDATE discussions SET workspace_mode = 'Isolated',
                            workspace_path = NULL, worktree_branch = 'kronn/test-relock'
                     WHERE id = ?1",
                    rusqlite::params![disc],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let resp = run_agent(State(state.clone()), Path(disc.to_string()), None).await;
        let body = sse_body_to_string(resp).await;
        assert!(
            body.contains("error"),
            "re-lock preflight must fail, got: {body}"
        );

        let awaiting: i64 = state
            .db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = ?1",
                    rusqlite::params![disc],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(
            awaiting, 0,
            "failed preflight must not leave the disc marked as owed a run"
        );
    }

    /// A BATCH child is pre-marked awaiting_agent=1 at enqueue
    /// (create_batch_run) and its SSE stream has no consumer. A preflight
    /// failure must behave like the agent-start-failed arm: persist the
    /// error in the thread, clear the marker (no bogus boot notice) and
    /// bump the batch counters (no run stuck at n-1/N).
    #[tokio::test]
    async fn batch_child_preflight_failure_persists_error_and_settles_the_batch() {
        let disc = "d-batch-relock";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
                     VALUES ('wf-r12', 'r12', '{}', '[]', ?1, ?1)",
                    rusqlite::params![now],
                )?;
                conn.execute(
                    "INSERT INTO workflow_runs (id, workflow_id, run_type, status, started_at, batch_total)
                     VALUES ('run-r12', 'wf-r12', 'batch', 'Running', ?1, 1)",
                    rusqlite::params![now],
                )?;
                conn.execute(
                    "UPDATE discussions SET workflow_run_id = 'run-r12', awaiting_agent = 1,
                            workspace_mode = 'Isolated', workspace_path = NULL,
                            worktree_branch = 'kronn/test-relock'
                     WHERE id = ?1",
                    rusqlite::params![disc],
                )?;
                crate::db::discussions::insert_message(
                    conn,
                    disc,
                    &DiscussionMessage {
                        id: "user-batch-relock".into(),
                        role: MessageRole::User,
                        content: "run batch child".into(),
                        timestamp: Utc::now(),
                        model: None,
                        lint_report: None,
                        agent_type: None,
                        tokens_used: 0,
                        auth_mode: None,
                        model_tier: None,
                        cost_usd: None,
                        author_pseudo: None,
                        author_avatar_email: None,
                        source_msg_id: None,
                        duration_ms: None,
                    },
                )?;
                crate::db::agent_dispatch::enqueue_for_latest_user(
                    conn,
                    crate::db::agent_dispatch::NewLatestUserDispatch {
                        id: "job-batch-relock",
                        discussion_id: disc,
                        dedupe_key: "message:user-batch-relock",
                        agent_override: None,
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: Some("run-r12"),
                        group_concurrency_limit: None,
                    },
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let resp = crate::api::discussions::runtime::stream_dispatch_job(
            state.clone(),
            "job-batch-relock".to_string(),
            None,
        )
        .await
        .expect("durable batch child stream");
        let body = sse_body_to_string(resp).await;
        assert!(
            body.contains("error"),
            "re-lock preflight must fail, got: {body}"
        );

        let settled = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let failed = state
                    .db
                    .with_conn(|conn| {
                        Ok(conn.query_row(
                            "SELECT batch_failed FROM workflow_runs WHERE id = 'run-r12'",
                            [],
                            |row| row.get::<_, i64>(0),
                        )?)
                    })
                    .await
                    .unwrap();
                if failed == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(settled.is_ok(), "durable preflight settlement timed out");

        let (awaiting, batch_failed): (i64, i64) = state
            .db
            .with_conn(move |conn| {
                let awaiting = conn.query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = ?1",
                    rusqlite::params![disc],
                    |r| r.get(0),
                )?;
                let failed = conn.query_row(
                    "SELECT batch_failed FROM workflow_runs WHERE id = 'run-r12'",
                    [],
                    |r| r.get(0),
                )?;
                Ok((awaiting, failed))
            })
            .await
            .unwrap();
        assert_eq!(
            awaiting, 0,
            "enqueue-time marker must be cleared on preflight failure"
        );
        assert_eq!(
            batch_failed, 1,
            "the child must count as failed so the batch can finish"
        );

        let msgs = state
            .db
            .with_conn(move |conn| crate::db::discussions::list_messages(conn, disc))
            .await
            .unwrap();
        assert!(
            msgs.iter()
                .any(|m| m.role == MessageRole::System && m.content.starts_with("Erreur:")),
            "the preflight error must be persisted in the thread (fire-and-forget child)"
        );
    }
}
