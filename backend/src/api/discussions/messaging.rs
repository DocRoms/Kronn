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
        lint_report: None,
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

    // Double-responder guard (2026-06-04, flagged by Romuald; made
    // presence-sticky 2026-06-08) — if ≥1 MCP agent is connected to this
    // disc (joined via disc_join, status 'active'), it answers itself.
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
    let live_check_id = id.clone();
    // Fail-OPEN on a DB error (count → 0 → Kronn answers as usual): a
    // transient error must not leave the human with no reply at all. The
    // worst case is a one-off double-response, far less bad than silence —
    // but log it so a persistent error is visible (Codex review 2026-06-04).
    let live_agents = match state.db.with_conn(move |conn| {
        crate::db::discussion_sessions::count_live_participants(conn, &live_check_id)
    }).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!("send_message: count_live_participants failed for disc {id}, falling back to local runner: {e}");
            0
        }
    };
    if live_agents > 0 {
        tracing::info!(
            "send_message: {live_agents} live MCP agent(s) on disc {id} — skipping local runner (connected agents respond)"
        );
        let payload = serde_json::json!({
            "skipped": true,
            "reason": "live_mcp_agents",
            "live_agents": live_agents,
        }).to_string();
        let stream: SseStream = Box::pin(futures::stream::once(async move {
            Ok::<_, Infallible>(Event::default().event("skipped_live_agents").data(payload))
        }));
        return Sse::new(stream);
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
                crate::db::discussion_sessions::create_session(conn, disc, "Codex", Some("sess-x"), "peer")
            })
            .await
            .unwrap();

        let resp = send_message(
            State(state.clone()),
            Path(disc.to_string()),
            Json(SendMessageRequest { content: "hello peers".into(), target_agent: None }),
        )
        .await;
        let body = sse_body_to_string(resp).await;
        assert!(body.contains("skipped_live_agents"), "expected skip event, got: {body}");
        assert!(body.contains("live_mcp_agents"), "skip reason present");

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
                crate::db::discussion_sessions::create_session(conn, disc, "Codex", Some("sess-p"), "peer")
            })
            .await
            .unwrap();
        state
            .db
            .with_conn(move |conn| crate::db::discussion_sessions::set_session_status(conn, pk, "paused"))
            .await
            .unwrap();

        let live = state
            .db
            .with_conn(move |conn| crate::db::discussion_sessions::count_live_participants(conn, disc))
            .await
            .unwrap();
        assert_eq!(live, 0, "paused agent is not a live responder → Kronn would still answer");
    }
}
