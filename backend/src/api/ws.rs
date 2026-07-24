use axum::{
    extract::{
        connect_info::ConnectInfo,
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::HeaderMap,
    response::IntoResponse,
    Extension,
};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

use crate::{core::ws_client::PeerEchoGuard, models::WsMessage, AppState};

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
                || s.first_failure
                    .is_some_and(|t| now.duration_since(t) < WINDOW)
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
pub(crate) fn should_reject_empty_invite(invite_code: &str, is_local: bool) -> bool {
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
    // Behind the nginx gateway (the docker-compose deployment), the socket
    // peer is ALWAYS the gateway's container IP — never the real client. The
    // gateway sets `X-Real-IP` to the true client address; we trust it because
    // in this topology only the gateway can reach the backend port. Without
    // this, every browser looks like one non-loopback peer (the gateway), so
    // the local frontend's empty-invite Presence is treated as brute-force and
    // bans the gateway IP → ALL clients' WS get rejected (reconnect storm).
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let socket_ip = connect_info
        .map(|ext| ext.0 .0.ip())
        .unwrap_or_else(|| IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    let peer_ip = resolve_client_ip(&headers, socket_ip);
    ws.on_upgrade(move |socket| handle_socket(socket, state, peer_ip))
}

/// Resolve the real client IP for rate-limiting/ban decisions.
///
/// Order: `X-Real-IP` (set by the nginx gateway) → first hop of
/// `X-Forwarded-For` → the socket peer IP. Returns the socket IP when no
/// proxy header is present or parseable (direct connection, e.g. the desktop
/// app or tests). Pure + exported so the derivation is unit-tested directly —
/// the bug it fixes (banning the gateway IP for everyone) had zero coverage
/// precisely because the handler read `ConnectInfo` only.
pub(crate) fn resolve_client_ip(headers: &HeaderMap, socket_ip: IpAddr) -> IpAddr {
    if let Some(real) = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
    {
        return real;
    }
    if let Some(fwd) = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
    {
        return fwd;
    }
    socket_ip
}

/// Is `ip` a TRUSTED client — i.e. the local Kronn UI or a same-host/LAN
/// caller — for the empty-invite-code shortcut and ban exemption?
///
/// Trusted = loopback OR private-range (RFC1918 / IPv6 ULA / link-local).
/// Rationale: the self-hosted topology is "Kronn behind its own reverse proxy
/// on a private docker/LAN network". The owner's own browser reaches the
/// backend through that private network, so its source IP is private, never
/// loopback. Real cross-internet peers arrive with a PUBLIC IP (preserved via
/// `resolve_client_ip` / X-Real-IP) → untrusted → must present a valid invite
/// code, and brute-force attempts are still rate-limited per real IP.
pub(crate) fn is_trusted_client_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // ULA   fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
        }
    }
}

async fn handle_socket(socket: WebSocket, state: AppState, peer_ip: IpAddr) {
    // Reject up-front if this peer is currently banned for invite-code
    // brute-force. TRUSTED clients (loopback OR private-range) are exempt:
    // the local Kronn UI is the only legitimate caller of the empty-invite
    // shortcut, and behind the docker/nginx gateway its connection arrives
    // with a PRIVATE source IP (the bridge gateway, e.g. 172.x / 192.168.x),
    // never loopback. Treating only loopback as trusted banned that shared
    // gateway IP for everyone → all WS rejected → reconnect storm. Real
    // external peers arrive with a PUBLIC IP (via X-Real-IP, see
    // `resolve_client_ip`) → still untrusted → must send a valid invite, and
    // brute-force from them is still rate-limited per real IP.
    let is_local = is_trusted_client_ip(peer_ip);
    if !is_local && rate_limit::is_banned(peer_ip) {
        tracing::warn!("WS: rejecting banned peer {}", peer_ip);
        return;
    }

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let mut broadcast_rx = state.ws_broadcast.subscribe();
    let broadcast_tx = state.ws_broadcast.clone();

    // Loop guard shared between the two halves of THIS connection: keys of
    // relayable frames we received from the peer, so the send half never echoes
    // them straight back. Only consulted for peer connections.
    let echo_guard: Arc<Mutex<PeerEchoGuard>> = Arc::new(Mutex::new(PeerEchoGuard::default()));
    let recv_guard = echo_guard.clone();

    // Task 1: forward broadcast events → WS client.
    //
    // The local frontend (`is_local`) subscribes to the full bus and must see
    // every variant. A **remote peer** (`!is_local`) only gets peer-relayable
    // frames (chat + invites) and never a frame it just sent us — forwarding
    // Presence/local-UI signals to a peer is what bounced the 256-slot channel
    // into overflow and dropped the socket (~2 s cross-machine flap).
    let send_task_is_local = is_local;
    let mut send_task = tokio::spawn(async move {
        // Keepalive: a remote peer's socket is nearly silent (Presence/heartbeats
        // are no longer relayed), so without periodic traffic a middlebox (WSL2's
        // NAT drops idle TCP in ~5 s) kills it with no Close frame → zombie. The
        // server side pings too so BOTH directions of the flow stay warm. The
        // local frontend keeps the existing cadence (its own 30 s app-ping), so
        // its keepalive interval is effectively disabled here.
        let keepalive_every = if send_task_is_local {
            Duration::from_secs(86_400)
        } else {
            crate::core::ws_client::WS_KEEPALIVE_INTERVAL
        };
        let mut keepalive = tokio::time::interval(keepalive_every);
        keepalive.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = keepalive.tick() => {
                    if ws_sender.send(Message::Ping(Vec::<u8>::new().into())).await.is_err() {
                        break;
                    }
                }
                recv = broadcast_rx.recv() => match recv {
                    Ok(msg) => {
                        if !send_task_is_local {
                            if !msg.is_peer_relayable() {
                                continue;
                            }
                            if let Some(key) = msg.relay_dedup_key() {
                                if echo_guard.lock().unwrap_or_else(|e| e.into_inner()).contains(&key) {
                                    continue;
                                }
                            }
                        }
                        if let Ok(json) = serde_json::to_string(&msg) {
                            // axum 0.8 — `Message::Text` now wraps `Utf8Bytes`
                            // instead of `String`, providing zero-copy from Bytes.
                            // `.into()` covers `String -> Utf8Bytes`.
                            if ws_sender.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    // A burst made us fall behind. Skip the gap and keep the socket
                    // rather than tearing it down (the old `while let Ok` treated
                    // Lagged as terminal → reconnect storm + dropped UI updates).
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        }
    });

    // Task 2: receive WS messages → broadcast
    let recv_is_local = is_local;
    let mut recv_task = tokio::spawn(async move {
        // Pre-Presence handshake : `verified=false` until a `Presence`
        // is seen. Heartbeats (`Ping`) are answered before the gate
        // (cf. TD-20260504 — Ping racing reconnect Presence over a
        // paused-Docker boundary used to close the channel forever).
        // Other message types are silently dropped pre-Presence so the
        // attacker model stays the same: no ChatMessage / Invite goes
        // through without a peer-authenticating Presence.
        let mut verified = false;

        // Idle dead-detection: a remote peer must produce *some* frame (its
        // keepalive Ping counts) within WS_IDLE_TIMEOUT, else the socket is
        // presumed dead and dropped (the peer's manager reconnects) instead of
        // blocking forever on a silently-killed connection. The local frontend
        // is exempt (effectively-infinite window) — it pings only every 30 s
        // and must never be dropped just for being quiet.
        let idle = if recv_is_local {
            Duration::from_secs(86_400)
        } else {
            crate::core::ws_client::WS_IDLE_TIMEOUT
        };

        loop {
            let msg = match tokio::time::timeout(idle, ws_receiver.next()).await {
                Err(_idle) => break,
                Ok(None) | Ok(Some(Err(_))) => break,
                Ok(Some(Ok(m))) => m,
            };
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
                                    peer_ip,
                                    ws_msg
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
                                            peer_ip,
                                            from_invite_code
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

                    // Relayable frames (chat / invite) from a peer are persisted
                    // here, and re-broadcast onto the local bus ONLY if new — a
                    // duplicate must not be re-broadcast or it bounces back out to
                    // peers and loops (duplicate toasts/notifications). Other
                    // frames (Presence …) are always forwarded to the frontend.
                    let should_broadcast = if ws_msg.is_peer_relayable() {
                        ingest_relayable_frame(&state, &ws_msg).await
                    } else {
                        true
                    };

                    if should_broadcast {
                        // Record this frame as seen-from-this-peer *before*
                        // broadcasting, so the send half (Task 1) won't echo it
                        // straight back to the peer it came from.
                        if let Some(key) = ws_msg.relay_dedup_key() {
                            recv_guard
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .record(key);
                        }
                        let _ = broadcast_tx.send(ws_msg);
                    }
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
///
/// Returns `true` when a NEW message was inserted (caller should re-broadcast
/// it to the local frontend), `false` for a duplicate or a drop (caller must
/// NOT re-broadcast — re-broadcasting a frame already applied is what bounces
/// it back out to peers and loops, producing duplicate notifications).
#[allow(clippy::too_many_arguments)]
fn handle_incoming_chat_message(
    conn: &rusqlite::Connection,
    shared_discussion_id: &str,
    message_id: &str,
    from_pseudo: &str,
    from_avatar_email: Option<&str>,
    content: &str,
    timestamp: i64,
    role: crate::models::MessageRole,
    agent_type: Option<crate::models::AgentType>,
) -> anyhow::Result<bool> {
    // Find local discussion by shared_id
    let Some(disc_id) =
        crate::db::discussions::find_discussion_by_shared_id(conn, shared_discussion_id)?
    else {
        tracing::warn!(
            "WS: ChatMessage for unknown shared_id {}, dropping",
            shared_discussion_id
        );
        return Ok(false);
    };

    // Check for duplicate (idempotent — same message_id won't be inserted twice)
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM messages WHERE id = ?1",
            rusqlite::params![message_id],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if exists {
        return Ok(false);
    }

    let ts = chrono::DateTime::from_timestamp_millis(timestamp).unwrap_or_else(Utc::now);
    // role + agent_type come from the wire (F2): a federated AGENT reply lands
    // as an Agent message carrying its CLI name, not a generic User. Frames
    // from an older peer carry no fields → serde defaults (User / None), i.e.
    // the historical behaviour.
    let msg = crate::models::DiscussionMessage {
        model: None,
        lint_report: None,
        id: message_id.to_string(),
        role,
        content: content.to_string(),
        agent_type,
        timestamp: ts,
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: Some(from_pseudo.to_string()),
        author_avatar_email: from_avatar_email.map(|s| s.to_string()),
        source_msg_id: None,
        duration_ms: None,
    };

    crate::db::discussions::insert_message(conn, &disc_id, &msg)?;
    tracing::info!(
        "WS: inserted remote message from {} in shared disc {}",
        from_pseudo,
        shared_discussion_id
    );
    Ok(true)
}

/// Create a local discussion from a remote invitation.
///
/// Returns `true` when a NEW local copy was created (caller should re-broadcast
/// so the frontend refreshes + toasts), `false` when the shared disc is already
/// known (caller must NOT re-broadcast — avoids duplicate toasts and a relay
/// loop between two instances).
fn handle_discussion_invite(
    conn: &rusqlite::Connection,
    shared_discussion_id: &str,
    title: &str,
    from_pseudo: &str,
) -> anyhow::Result<bool> {
    // Check if we already have this shared discussion — if so, NOT new, don't
    // re-broadcast (loop guard). Otherwise create the mirror via the shared
    // helper so this path and the HTTP `claim-by-token` join path converge on
    // an identical local representation (same title format, same defaults).
    if crate::db::discussions::find_discussion_by_shared_id(conn, shared_discussion_id)?.is_some() {
        tracing::debug!(
            "WS: DiscussionInvite for already-known shared_id {}",
            shared_discussion_id
        );
        return Ok(false);
    }

    crate::db::discussions::ensure_mirror_by_shared_id(
        conn,
        shared_discussion_id,
        title,
        from_pseudo,
    )?;
    tracing::info!(
        "WS: created shared discussion '{}' from invite by {}",
        title,
        from_pseudo
    );
    Ok(true)
}

/// Persist an inbound peer frame (chat message / discussion invite) and report
/// whether it was **new** — i.e. whether the caller should re-broadcast it onto
/// the local bus so the frontend updates.
///
/// Returning `false` for duplicates is the single guard that breaks the
/// cross-connection relay loop: between two instances there are two directional
/// sockets, so a per-connection echo guard can't stop a frame bouncing
/// A→B→A→B…; gating the re-broadcast on novelty (the DB already has this
/// message_id / shared_id) stops it dead while still delivering the first copy.
/// Shared by the inbound `handle_socket` and the outbound `ws_client` receive
/// halves so a frame is persisted exactly once regardless of which socket it
/// arrives on.
pub(crate) async fn ingest_relayable_frame(state: &AppState, msg: &WsMessage) -> bool {
    match msg {
        WsMessage::ChatMessage {
            shared_discussion_id,
            message_id,
            from_pseudo,
            from_avatar_email,
            content,
            timestamp,
            role,
            agent_type,
            ..
        } => {
            let (sid, mid, pseudo, avatar, text, ts, r, at) = (
                shared_discussion_id.clone(),
                message_id.clone(),
                from_pseudo.clone(),
                from_avatar_email.clone(),
                content.clone(),
                *timestamp,
                role.clone(),
                agent_type.clone(),
            );
            state
                .db
                .with_conn(move |conn| {
                    handle_incoming_chat_message(
                        conn,
                        &sid,
                        &mid,
                        &pseudo,
                        avatar.as_deref(),
                        &text,
                        ts,
                        r,
                        at,
                    )
                })
                .await
                .unwrap_or(false)
        }
        WsMessage::DiscussionInvite {
            shared_discussion_id,
            title,
            from_pseudo,
            ..
        } => {
            let (sid, t, p) = (
                shared_discussion_id.clone(),
                title.clone(),
                from_pseudo.clone(),
            );
            state
                .db
                .with_conn(move |conn| handle_discussion_invite(conn, &sid, &t, &p))
                .await
                .unwrap_or(false)
        }
        WsMessage::DiscSyncRequest {
            shared_discussion_id,
            since_timestamp,
        } => {
            // Answer with the missing messages (broadcast → relayed back to the
            // requester). The request itself is NEVER re-broadcast (return
            // false) — it is consumed here, so it can't bounce between peers.
            crate::api::federation::respond_to_sync_request(
                state,
                shared_discussion_id,
                *since_timestamp,
            )
            .await;
            false
        }
        WsMessage::FileAttached {
            shared_discussion_id,
            message_id,
            file_id,
            filename,
            mime_type,
            size,
            from_invite_code,
            ..
        } => {
            // Already have this file? Do nothing. This is the idempotency guard
            // AND it breaks the pending/ready re-broadcast ping-pong: the origin
            // peer (which holds the file) receives our local emits below, finds
            // the file present, and stops — so the frames don't bounce.
            let exists = {
                let fid = file_id.clone();
                state
                    .db
                    .with_conn(move |conn| {
                        crate::db::discussions::context_file_exists(conn, &fid)
                            .map_err(|e| anyhow::anyhow!(e))
                    })
                    .await
                    .unwrap_or(false)
            };
            if exists {
                return false;
            }

            // F15+ — announce the incoming file to the LOCAL UI immediately
            // (pending:true) so it shows a "downloading…" placeholder before the
            // binary lands. fetch_and_store_attachment emits pending:false once
            // stored.
            let _ = state.ws_broadcast.send(WsMessage::FileAttached {
                shared_discussion_id: shared_discussion_id.clone(),
                message_id: message_id.clone(),
                file_id: file_id.clone(),
                filename: filename.clone(),
                mime_type: mime_type.clone(),
                size: *size,
                from_invite_code: from_invite_code.clone(),
                pending: true,
            });

            // Fetch the binary in the background — a network round-trip we must
            // NOT block the recv loop on.
            let st = state.clone();
            let (sid, mid, fid, fname, mime, host) = (
                shared_discussion_id.clone(),
                message_id.clone(),
                file_id.clone(),
                filename.clone(),
                mime_type.clone(),
                from_invite_code.clone(),
            );
            let sz = *size;
            tokio::spawn(async move {
                crate::api::federation::fetch_and_store_attachment(
                    &st, &sid, &mid, &fid, &fname, &mime, sz, &host,
                )
                .await;
            });
            false
        }
        _ => false,
    }
}

/// Auto-create a pending contact from an incoming invite code.
/// Returns the created contact, or None if the code is invalid.
async fn auto_add_peer(state: &AppState, invite_code: &str) -> Option<crate::models::Contact> {
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
            role: crate::models::MessageRole::User,
            agent_type: None,
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

    mod resolve_client_ip {
        use super::super::resolve_client_ip;
        use axum::http::{HeaderMap, HeaderName};
        use std::net::{IpAddr, Ipv4Addr};

        const GATEWAY: IpAddr = IpAddr::V4(Ipv4Addr::new(172, 19, 0, 4));

        fn hdr(name: &'static str, val: &str) -> HeaderMap {
            let mut h = HeaderMap::new();
            h.insert(HeaderName::from_static(name), val.parse().unwrap());
            h
        }

        #[test]
        fn x_real_ip_wins_over_socket() {
            // The core regression: behind nginx the socket IP is the gateway,
            // but X-Real-IP carries the real client (here loopback). Resolving
            // to loopback is what makes the empty-invite Presence accepted and
            // stops the gateway-ban reconnect storm.
            let ip = resolve_client_ip(&hdr("x-real-ip", "127.0.0.1"), GATEWAY);
            assert_eq!(ip, IpAddr::V4(Ipv4Addr::LOCALHOST));
            assert!(ip.is_loopback());
        }

        #[test]
        fn x_real_ip_carries_a_lan_peer() {
            let ip = resolve_client_ip(&hdr("x-real-ip", "10.0.0.42"), GATEWAY);
            assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 42)));
        }

        #[test]
        fn falls_back_to_x_forwarded_for_first_hop() {
            let ip = resolve_client_ip(&hdr("x-forwarded-for", "203.0.113.7, 172.19.0.4"), GATEWAY);
            assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)));
        }

        #[test]
        fn falls_back_to_socket_when_no_proxy_header() {
            // Direct connection (desktop app / tests) — keep the socket IP.
            let ip = resolve_client_ip(&HeaderMap::new(), GATEWAY);
            assert_eq!(ip, GATEWAY);
        }

        #[test]
        fn garbage_header_falls_through_to_socket() {
            let ip = resolve_client_ip(&hdr("x-real-ip", "not-an-ip"), GATEWAY);
            assert_eq!(ip, GATEWAY);
        }
    }

    mod is_trusted_client_ip {
        use super::super::is_trusted_client_ip;
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

        fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
            IpAddr::V4(Ipv4Addr::new(a, b, c, d))
        }

        #[test]
        fn loopback_and_private_are_trusted() {
            assert!(is_trusted_client_ip(v4(127, 0, 0, 1))); // loopback
            assert!(is_trusted_client_ip(v4(172, 19, 0, 4))); // docker bridge (the storm IP)
            assert!(is_trusted_client_ip(v4(172, 17, 0, 1))); // docker default gateway
            assert!(is_trusted_client_ip(v4(10, 0, 0, 5))); // RFC1918 10/8
            assert!(is_trusted_client_ip(v4(192, 168, 1, 50))); // RFC1918 192.168/16
            assert!(is_trusted_client_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
            assert!(is_trusted_client_ip("fd00::1".parse().unwrap())); // IPv6 ULA
        }

        #[test]
        fn public_ips_are_not_trusted() {
            // A real cross-internet peer: must present a valid invite + is ban-eligible.
            assert!(!is_trusted_client_ip(v4(8, 8, 8, 8)));
            assert!(!is_trusted_client_ip(v4(203, 0, 113, 7)));
            assert!(!is_trusted_client_ip(
                "2001:4860:4860::8888".parse().unwrap()
            ));
        }
    }
}

/// Novelty-gating invariants that break the cross-connection relay loop:
/// a relayable frame is re-broadcast (handler returns `true`) only the FIRST
/// time it is applied; any duplicate returns `false` so it is never bounced
/// back out to peers (the bug that produced duplicate invites/notifications).
#[cfg(test)]
mod relay_dedup_tests {
    use super::*;
    use rusqlite::Connection;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&c).unwrap();
        c
    }

    #[test]
    fn discussion_invite_is_new_once_then_duplicate() {
        let c = conn();
        // First invite creates the local copy → new → re-broadcast.
        assert!(handle_discussion_invite(&c, "shared-1", "Title", "Romu").unwrap());
        // Same shared_id again → already known → NOT new → no re-broadcast (loop dies).
        assert!(!handle_discussion_invite(&c, "shared-1", "Title", "Romu").unwrap());
    }

    #[test]
    fn chat_message_drops_unknown_then_inserts_once() {
        let c = conn();
        use crate::models::MessageRole;
        // No local disc with this shared_id yet → dropped → NOT new.
        assert!(!handle_incoming_chat_message(
            &c,
            "shared-2",
            "m1",
            "Romu",
            None,
            "hi",
            0,
            MessageRole::User,
            None
        )
        .unwrap());
        // Create the shared disc, then the same chat inserts once → new…
        assert!(handle_discussion_invite(&c, "shared-2", "T", "Romu").unwrap());
        assert!(handle_incoming_chat_message(
            &c,
            "shared-2",
            "m1",
            "Romu",
            None,
            "hi",
            0,
            MessageRole::User,
            None
        )
        .unwrap());
        // …and a duplicate message_id is NOT new (idempotent + loop-safe).
        assert!(!handle_incoming_chat_message(
            &c,
            "shared-2",
            "m1",
            "Romu",
            None,
            "hi",
            0,
            MessageRole::User,
            None
        )
        .unwrap());
    }
}
