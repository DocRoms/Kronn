use axum::{
    extract::{
        connect_info::ConnectInfo,
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
    Extension,
};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use std::net::{IpAddr, SocketAddr};

use crate::{models::WsMessage, AppState};

// ── Invite-code brute-force protection ─────────────────────────────────────
//
// A peer that wants to talk to this Kronn instance must send a Presence
// message with a valid invite code as its first WS payload. Without rate
// limiting, an attacker could open many WebSocket connections and brute-force
// invite codes by spraying random values until one matches a contact in the
// local DB.
//
// We track failed invite-code attempts per remote IP in a process-local map
// (no DB, no shared state between restarts — fine for a desktop app where the
// process lives a few hours at most). After `MAX_FAILED_ATTEMPTS` failures
// inside `WINDOW`, the IP is rejected for `BAN_DURATION`.
mod rate_limit {
    use std::collections::HashMap;
    use std::net::IpAddr;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    /// How many failed invite-code attempts an IP can make before being banned.
    const MAX_FAILED_ATTEMPTS: u32 = 10;
    /// Sliding window over which failed attempts are counted.
    const WINDOW: Duration = Duration::from_secs(60);
    /// How long an IP stays banned after exceeding the threshold.
    const BAN_DURATION: Duration = Duration::from_secs(300);

    #[derive(Debug, Default)]
    struct AttemptState {
        first_failure: Option<Instant>,
        failure_count: u32,
        banned_until: Option<Instant>,
    }

    fn state() -> &'static Mutex<HashMap<IpAddr, AttemptState>> {
        static STATE: OnceLock<Mutex<HashMap<IpAddr, AttemptState>>> = OnceLock::new();
        STATE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// Returns true if `ip` is currently banned.
    pub fn is_banned(ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut map = match state().lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        // Opportunistic GC: drop entries that are neither banned nor in-window
        map.retain(|_, s| {
            s.banned_until.is_some_and(|until| until > now)
                || s.first_failure.is_some_and(|t| now.duration_since(t) < WINDOW)
        });
        map.get(&ip)
            .and_then(|s| s.banned_until)
            .is_some_and(|until| until > now)
    }

    /// Record one failed invite-code attempt from `ip`. Returns true when the
    /// IP has just crossed the ban threshold (caller should log).
    pub fn record_failure(ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut map = match state().lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let entry = map.entry(ip).or_default();

        // Reset window if it has elapsed since the first counted failure
        if let Some(first) = entry.first_failure {
            if now.duration_since(first) >= WINDOW {
                entry.first_failure = Some(now);
                entry.failure_count = 0;
            }
        } else {
            entry.first_failure = Some(now);
        }

        entry.failure_count += 1;
        if entry.failure_count >= MAX_FAILED_ATTEMPTS && entry.banned_until.is_none() {
            entry.banned_until = Some(now + BAN_DURATION);
            return true;
        }
        false
    }

    /// Clear bookkeeping for a specific IP (used by tests).
    #[cfg(test)]
    pub fn reset(ip: IpAddr) {
        if let Ok(mut map) = state().lock() {
            map.remove(&ip);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::net::Ipv4Addr;

        #[test]
        fn ban_after_threshold() {
            let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
            reset(ip);
            assert!(!is_banned(ip));
            for i in 0..MAX_FAILED_ATTEMPTS - 1 {
                let crossed = record_failure(ip);
                assert!(!crossed, "should not ban before threshold (iter {})", i);
                assert!(!is_banned(ip));
            }
            let crossed = record_failure(ip);
            assert!(crossed, "the threshold-crossing call must signal ban");
            assert!(is_banned(ip), "ip must be banned after threshold");
            reset(ip);
        }

        #[test]
        fn other_ip_not_affected_by_ban() {
            let bad = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
            let good = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3));
            reset(bad);
            reset(good);
            for _ in 0..MAX_FAILED_ATTEMPTS {
                record_failure(bad);
            }
            assert!(is_banned(bad));
            assert!(!is_banned(good), "ban must be per-IP");
            reset(bad);
            reset(good);
        }
    }
}

/// What the recv-task should do with a single inbound frame, before
/// the peer has sent its `Presence`. Pure decision — no side-effect —
/// so we can unit-test the handshake policy without standing up a
/// full WebSocket harness. Mirrors the in-line logic in `handle_socket`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrePresenceAction {
    /// Heartbeat — answer Pong, stay unverified.
    Heartbeat,
    /// Caller can run the Presence verification path.
    Presence,
    /// Anything else: drop silently (debug-log), wait for Presence.
    Drop,
}

pub(crate) fn classify_pre_presence(msg: &WsMessage) -> PrePresenceAction {
    match msg {
        WsMessage::Ping { .. } => PrePresenceAction::Heartbeat,
        WsMessage::Presence { .. } => PrePresenceAction::Presence,
        _ => PrePresenceAction::Drop,
    }
}

/// Whether to reject a `Presence` frame *before* the contact lookup.
///
/// Empty `from_invite_code` is reserved for the local frontend, which
/// connects on the loopback interface and never carries a user-facing
/// invite code. Any non-loopback peer that sends an empty code is
/// trying to slip past the contact-lookup + rate-limit gate (which
/// only fires for non-empty codes) — the post-Presence verified
/// state would then let them broadcast `ChatMessage` /
/// `DiscussionInvite` into the local shared discussions.
///
/// Pure decision so we can unit-test the policy without mounting a
/// full WebSocket harness.
pub(crate) fn should_reject_empty_invite(
    invite_code: &str,
    is_local: bool,
) -> bool {
    invite_code.is_empty() && !is_local
}

/// GET /api/ws — WebSocket upgrade handler.
///
/// Accepts connections from:
/// - The local frontend (for real-time presence updates)
/// - Remote Kronn instances (peer-to-peer sync)
///
/// All inbound WsMessages are forwarded to the broadcast channel,
/// and all broadcast events are forwarded to the WebSocket client.
///
/// `ConnectInfo` is wrapped in `Option` so the handler also works in tests
/// that build the router without `into_make_service_with_connect_info`. When
/// the connect-info extension is missing we treat the connection as
/// loopback (rate limiting bypass) — this is safe because real production
/// servers in `main.rs` and `desktop/src-tauri/src/main.rs` always wire it.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    // axum 0.8 dropped `OptionalFromRequestParts` for `ConnectInfo`; the
    // underlying request extension still lives behind `Extension<…>`, which
    // does implement it, so we extract that instead.
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let peer_ip = connect_info
        .map(|ext| ext.0.0.ip())
        .unwrap_or_else(|| IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    ws.on_upgrade(move |socket| handle_socket(socket, state, peer_ip))
}

async fn handle_socket(socket: WebSocket, state: AppState, peer_ip: IpAddr) {
    // Reject up-front if this peer is currently banned for invite-code
    // brute-force. Local-loopback IPs (127.0.0.1, ::1) are exempt because
    // they're the desktop frontend's own connection, which never sends an
    // invite code anyway.
    let is_local = peer_ip.is_loopback();
    if !is_local && rate_limit::is_banned(peer_ip) {
        tracing::warn!("WS: rejecting banned peer {}", peer_ip);
        return;
    }

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let mut broadcast_rx = state.ws_broadcast.subscribe();
    let broadcast_tx = state.ws_broadcast.clone();

    // Task 1: forward broadcast events → WS client
    let mut send_task = tokio::spawn(async move {
        while let Ok(msg) = broadcast_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                // axum 0.8 — `Message::Text` now wraps `Utf8Bytes`
                // instead of `String`, providing zero-copy from Bytes.
                // `.into()` covers `String -> Utf8Bytes`.
                if ws_sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    // Task 2: receive WS messages → broadcast
    let mut recv_task = tokio::spawn(async move {
        // Pre-Presence handshake : `verified=false` until a `Presence`
        // is seen. Heartbeats (`Ping`) are answered before the gate
        // (cf. TD-20260504 — Ping racing reconnect Presence over a
        // paused-Docker boundary used to close the channel forever).
        // Other message types are silently dropped pre-Presence so the
        // attacker model stays the same: no ChatMessage / Invite goes
        // through without a peer-authenticating Presence.
        let mut verified = false;

        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                Message::Text(text) => {
                    let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) else {
                        continue;
                    };

                    // Pre-Presence policy (single source of truth via
                    // `classify_pre_presence`, unit-tested in
                    // `handshake_tests`). Heartbeats are answered
                    // before the gate so a remote peer resuming from
                    // suspend keeps a usable channel; non-Presence
                    // non-heartbeat frames are dropped silently —
                    // attack vectors stay closed because the
                    // post-verify block is the only place ChatMessage /
                    // DiscussionInvite get broadcast.
                    if !verified {
                        match classify_pre_presence(&ws_msg) {
                            PrePresenceAction::Heartbeat => {
                                if let WsMessage::Ping { timestamp } = &ws_msg {
                                    let pong = WsMessage::Pong {
                                        timestamp: *timestamp,
                                    };
                                    let _ = broadcast_tx.send(pong);
                                }
                                continue;
                            }
                            PrePresenceAction::Drop => {
                                tracing::debug!(
                                    "WS: ignoring pre-presence frame from {}: {:?}",
                                    peer_ip, ws_msg
                                );
                                continue;
                            }
                            PrePresenceAction::Presence => {
                                // Fall through to the verification block below.
                            }
                        }
                    }

                    // Post-verify Ping handler (regular heartbeat).
                    if let WsMessage::Ping { timestamp } = &ws_msg {
                        let pong = WsMessage::Pong {
                            timestamp: *timestamp,
                        };
                        let _ = broadcast_tx.send(pong);
                        continue;
                    }

                    // Presence verification path. Always reached when
                    // `!verified` and the frame is a Presence (the
                    // classifier above already filtered the rest).
                    if !verified {
                        if let WsMessage::Presence {
                            ref from_invite_code,
                            ..
                        } = ws_msg
                        {
                            // Reject the empty-invite-code shortcut from
                            // non-loopback peers (security). The local
                            // frontend connects on 127.0.0.1 and is the
                            // only legitimate caller for the empty path.
                            if should_reject_empty_invite(from_invite_code, is_local) {
                                tracing::warn!(
                                    "WS: rejecting empty invite_code from non-loopback peer {} \
                                     (only the local frontend may use the empty-code shortcut)",
                                    peer_ip
                                );
                                let _crossed = rate_limit::record_failure(peer_ip);
                                break;
                            }
                            if !from_invite_code.is_empty() {
                                let code = from_invite_code.clone();
                                let found = state
                                    .db
                                    .with_conn(move |conn| {
                                        crate::db::contacts::find_contact_by_invite_code(
                                            conn, &code,
                                        )
                                    })
                                    .await;

                                if !matches!(&found, Ok(Some(_))) {
                                    // Unknown peer — auto-create as pending contact
                                    if let Some(contact) =
                                        auto_add_peer(&state, from_invite_code).await
                                    {
                                        tracing::info!(
                                            "WS: auto-added peer {} from invite code",
                                            contact.pseudo
                                        );
                                    } else {
                                        // Invalid invite code — count this attempt against
                                        // the remote IP (loopback exempted because the
                                        // local frontend never has an invalid code).
                                        if !is_local {
                                            let crossed = rate_limit::record_failure(peer_ip);
                                            if crossed {
                                                tracing::warn!(
                                                    "WS: peer {} hit invite-code failure threshold and is now banned",
                                                    peer_ip
                                                );
                                            }
                                        }
                                        tracing::warn!(
                                            "WS: rejected invalid invite code from {}: {}",
                                            peer_ip, from_invite_code
                                        );
                                        break;
                                    }
                                }
                            }
                            verified = true;
                        }
                        // The else branch is unreachable: classify_pre_presence
                        // already returned `Drop` for non-Presence frames above.
                    }

                    // Handle incoming chat messages from remote peers:
                    // insert into local DB, then broadcast to local frontend.
                    if let WsMessage::ChatMessage {
                        ref shared_discussion_id,
                        ref message_id,
                        ref from_pseudo,
                        ref from_avatar_email,
                        ref content,
                        timestamp,
                        ..
                    } = ws_msg
                    {
                        let sid = shared_discussion_id.clone();
                        let mid = message_id.clone();
                        let pseudo = from_pseudo.clone();
                        let avatar = from_avatar_email.clone();
                        let text = content.clone();
                        let ts = timestamp;
                        let _ = state
                            .db
                            .with_conn(move |conn| {
                                handle_incoming_chat_message(
                                    conn, &sid, &mid, &pseudo, avatar.as_deref(), &text, ts,
                                )
                            })
                            .await;
                    }

                    // Handle discussion invites: create local discussion copy.
                    if let WsMessage::DiscussionInvite {
                        ref shared_discussion_id,
                        ref title,
                        ref from_pseudo,
                        ..
                    } = ws_msg
                    {
                        let sid = shared_discussion_id.clone();
                        let t = title.clone();
                        let p = from_pseudo.clone();
                        let _ = state
                            .db
                            .with_conn(move |conn| {
                                handle_discussion_invite(conn, &sid, &t, &p)
                            })
                            .await;
                    }

                    let _ = broadcast_tx.send(ws_msg);
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // Wait for either task to finish, then abort the other
    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }
}

/// Insert a remote chat message into the local discussion.
/// If no discussion exists for this shared_id, the message is silently dropped
/// (the DiscussionInvite should have created it first).
fn handle_incoming_chat_message(
    conn: &rusqlite::Connection,
    shared_discussion_id: &str,
    message_id: &str,
    from_pseudo: &str,
    from_avatar_email: Option<&str>,
    content: &str,
    timestamp: i64,
) -> anyhow::Result<()> {
    // Find local discussion by shared_id
    let Some(disc_id) = crate::db::discussions::find_discussion_by_shared_id(conn, shared_discussion_id)? else {
        tracing::warn!("WS: ChatMessage for unknown shared_id {}, dropping", shared_discussion_id);
        return Ok(());
    };

    // Check for duplicate (idempotent — same message_id won't be inserted twice)
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM messages WHERE id = ?1",
        rusqlite::params![message_id],
        |row| row.get(0),
    ).unwrap_or(false);
    if exists {
        return Ok(());
    }

    let ts = chrono::DateTime::from_timestamp_millis(timestamp)
        .unwrap_or_else(Utc::now);
    let msg = crate::models::DiscussionMessage {
        id: message_id.to_string(),
        role: crate::models::MessageRole::User,
        content: content.to_string(),
        agent_type: None,
        timestamp: ts,
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: Some(from_pseudo.to_string()),
        author_avatar_email: from_avatar_email.map(|s| s.to_string()),
    };

    crate::db::discussions::insert_message(conn, &disc_id, &msg)?;
    tracing::info!("WS: inserted remote message from {} in shared disc {}", from_pseudo, shared_discussion_id);
    Ok(())
}

/// Create a local discussion from a remote invitation.
fn handle_discussion_invite(
    conn: &rusqlite::Connection,
    shared_discussion_id: &str,
    title: &str,
    from_pseudo: &str,
) -> anyhow::Result<()> {
    // Check if we already have this shared discussion
    if crate::db::discussions::find_discussion_by_shared_id(conn, shared_discussion_id)?.is_some() {
        tracing::debug!("WS: DiscussionInvite for already-known shared_id {}", shared_discussion_id);
        return Ok(());
    }

    let now = Utc::now();
    let disc = crate::models::Discussion {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: None,
        title: format!("{} (shared by {})", title, from_pseudo),
        agent: crate::models::AgentType::ClaudeCode,
        language: "fr".into(),
        participants: vec![],
        messages: vec![],
        message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
            pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: crate::models::ModelTier::Default,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: crate::models::SummaryStrategy::Auto,
        introspection_call_count: 0,
        shared_id: Some(shared_discussion_id.to_string()),
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };

    crate::db::discussions::insert_discussion(conn, &disc)?;
    tracing::info!("WS: created shared discussion '{}' from invite by {}", title, from_pseudo);
    Ok(())
}

/// Auto-create a pending contact from an incoming invite code.
/// Returns the created contact, or None if the code is invalid.
async fn auto_add_peer(
    state: &AppState,
    invite_code: &str,
) -> Option<crate::models::Contact> {
    let (pseudo, kronn_url) = crate::db::contacts::parse_invite_code(invite_code)?;

    let now = Utc::now();
    let contact = crate::models::Contact {
        id: uuid::Uuid::new_v4().to_string(),
        pseudo,
        avatar_email: None,
        kronn_url,
        invite_code: invite_code.to_string(),
        status: "pending".into(),
        created_at: now,
        updated_at: now,
    };

    let c = contact.clone();
    state
        .db
        .with_conn(move |conn| crate::db::contacts::insert_contact(conn, &c))
        .await
        .ok()?;

    Some(contact)
}

#[cfg(test)]
mod handshake_tests {
    use super::*;

    #[test]
    fn ping_is_heartbeat_pre_presence() {
        let m = WsMessage::Ping { timestamp: 1 };
        assert_eq!(classify_pre_presence(&m), PrePresenceAction::Heartbeat);
    }

    #[test]
    fn presence_is_presence_pre_presence() {
        let m = WsMessage::Presence {
            from_pseudo: "x".into(),
            from_invite_code: "".into(),
            online: true,
        };
        assert_eq!(classify_pre_presence(&m), PrePresenceAction::Presence);
    }

    #[test]
    fn pong_is_dropped_pre_presence() {
        // Pong is a server-→-client frame; if a client sends one,
        // either bug or noise — drop, don't verify.
        let m = WsMessage::Pong { timestamp: 1 };
        assert_eq!(classify_pre_presence(&m), PrePresenceAction::Drop);
    }

    #[test]
    fn chat_message_is_dropped_pre_presence() {
        // Pre-0.7.2 this would have closed the channel. Now drop
        // silently and wait for Presence — fixes TD-20260504.
        let m = WsMessage::ChatMessage {
            shared_discussion_id: "d".into(),
            message_id: "m".into(),
            from_pseudo: "p".into(),
            from_avatar_email: None,
            from_invite_code: "i".into(),
            content: "hello".into(),
            timestamp: 1,
        };
        assert_eq!(classify_pre_presence(&m), PrePresenceAction::Drop);
    }

    #[test]
    fn invite_is_dropped_pre_presence() {
        let m = WsMessage::DiscussionInvite {
            shared_discussion_id: "d".into(),
            title: "t".into(),
            from_pseudo: "p".into(),
            from_invite_code: "i".into(),
        };
        assert_eq!(classify_pre_presence(&m), PrePresenceAction::Drop);
    }

    // ─── Empty-invite-code rejection (security regression test) ───────────
    //
    // Pre-fix, a remote peer could bypass the contact lookup + rate-limit
    // gate by sending `Presence { from_invite_code: "" }` — the empty
    // shortcut was meant ONLY for the loopback-frontend connection but
    // had no `is_local` guard, leaving the channel verified=true and
    // open for `ChatMessage` / `DiscussionInvite` injection into local
    // shared discussions.

    #[test]
    fn empty_invite_from_loopback_is_accepted() {
        // The local frontend connects on 127.0.0.1 with an empty
        // invite_code — must continue to work.
        assert!(!should_reject_empty_invite("", true));
    }

    #[test]
    fn empty_invite_from_remote_is_rejected() {
        // Non-loopback + empty invite_code = the bypass attempt that
        // pre-fix slipped through.
        assert!(should_reject_empty_invite("", false));
    }

    #[test]
    fn nonempty_invite_is_not_short_circuit_rejected() {
        // The empty-code rejection must not fire for real codes —
        // those go through the normal contact-lookup path, regardless
        // of where the peer connects from.
        assert!(!should_reject_empty_invite("kronn:peer@host:9090", false));
        assert!(!should_reject_empty_invite("kronn:peer@host:9090", true));
    }
}
