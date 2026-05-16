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
    response::sse::{Event, Sse},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

use super::streaming::make_agent_stream;
use super::{SseStream, MAX_CONTENT_LEN};

/// POST /api/discussions/:id/messages
pub async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SendMessageRequest>,
) -> Sse<SseStream> {
    // Input validation
    if req.content.len() > MAX_CONTENT_LEN {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(Event::default().event("error").data(
                serde_json::json!({ "error": "Message too long" }).to_string()
            ))
        }));
        return Sse::new(stream);
    }

    // Guard against the 2026-04-13 double-response bug: if a previous agent
    // run on this disc is still in recovery (partial_response checkpoint
    // dangling from a backend crash), refuse the new send instead of
    // stacking a fresh run on top of what will soon become a recovered
    // Agent message. The frontend can either wait for the PartialResponseRecovered
    // WS event or explicitly dismiss the partial (same endpoint below).
    let pending_check_id = id.clone();
    let has_partial = state.db.with_conn(move |conn| {
        crate::db::discussions::has_pending_partial(conn, &pending_check_id)
    }).await.unwrap_or(false);
    if has_partial {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(Event::default().event("error").data(
                serde_json::json!({
                    "error": "partial_pending",
                    "message": "Une réponse d'agent précédente est en cours de récupération. Patientez ou fermez la notification de récupération avant de renvoyer."
                }).to_string()
            ))
        }));
        return Sse::new(stream);
    }

    let target = req.target_agent.clone();

    // Read user identity from config for message attribution
    let (author_pseudo, author_avatar_email) = {
        let config = state.config.read().await;
        (config.server.pseudo.clone(), config.server.avatar_email.clone())
    };

    // Add user message to DB
    let user_msg = DiscussionMessage {
        id: Uuid::new_v4().to_string(),
        role: MessageRole::User,
        content: req.content.clone(),
        agent_type: None,
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: None, cost_usd: None, author_pseudo, author_avatar_email,
        source_msg_id: None, duration_ms: None,
    };
    let disc_id = id.clone();
    let msg = user_msg.clone();
    let target_clone = target.clone();
    let shared_id_for_ws = {
        let disc_id_check = id.clone();
        state.db.with_conn(move |conn| {
            crate::db::discussions::get_discussion(conn, &disc_id_check)
                .map(|d| d.and_then(|d| d.shared_id))
        }).await.ok().flatten()
    };

    if let Err(e) = state.db.with_conn(move |conn| {
        crate::db::discussions::insert_message(conn, &disc_id, &msg)?;
        // Track new participant
        if let Some(ref t) = target_clone {
            let disc = crate::db::discussions::get_discussion(conn, &disc_id)?;
            if let Some(d) = disc {
                if !d.participants.contains(t) {
                    let mut participants = d.participants;
                    participants.push(t.clone());
                    crate::db::discussions::update_discussion_participants(conn, &disc_id, &participants)?;
                }
            }
        }
        Ok(())
    }).await {
        tracing::error!("Failed to save user message: {e}");
    }

    // Broadcast to peers if this is a shared discussion
    if let Some(shared_id) = shared_id_for_ws {
        let config = state.config.read().await;
        let pseudo = config.server.pseudo.clone().unwrap_or_default();
        let avatar = config.server.avatar_email.clone();
        let host = crate::api::contacts::advertised_host_async(&config.server).await;
        let port = config.server.port;
        drop(config);
        let invite_code = format!("kronn:{}@{}:{}", pseudo, host, port);

        let _ = state.ws_broadcast.send(WsMessage::ChatMessage {
            shared_discussion_id: shared_id,
            message_id: user_msg.id.clone(),
            from_pseudo: pseudo,
            from_avatar_email: avatar,
            from_invite_code: invite_code,
            content: req.content.clone(),
            timestamp: user_msg.timestamp.timestamp_millis(),
        });
    }

    make_agent_stream(state, id, target).await
}

/// POST /api/discussions/:id/run
pub async fn run_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Sse<SseStream> {
    make_agent_stream(state, id, None).await
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
    let ids = match state.db.with_conn(move |conn| {
        // Reuses the boot recovery — process-wide (handles every disc with
        // a non-null partial), so a "dismiss" click incidentally cleans up
        // any other dangling partials too. Cheap (one indexed scan).
        crate::db::discussions::recover_partial_responses(conn)
    }).await {
        Ok(list) => list,
        Err(e) => return Json(ApiResponse::err(format!("Recovery failed: {}", e))),
    };
    let recovered_this = ids.iter().any(|d| d == &id);
    if !ids.is_empty() {
        let _ = state.ws_broadcast.send(WsMessage::PartialResponseRecovered {
            discussion_ids: ids,
        });
    }
    Json(ApiResponse::ok(serde_json::json!({ "recovered": recovered_this })))
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
    let cancelled = {
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
    Json(ApiResponse::ok(serde_json::json!({ "cancelled": cancelled })))
}
