use axum::{
    extract::{
        connect_info::ConnectInfo,
        ws::{Message, WebSocket},
        Query, State, WebSocketUpgrade,
    },
    http::HeaderMap,
    response::IntoResponse,
    Extension,
};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Query string of the WS upgrade. The frontend appends `?token=` when it holds
/// the API token (browsers cannot set an `Authorization` header on a WebSocket).
#[derive(Debug, Default, serde::Deserialize)]
pub struct WsAuthQuery {
    #[serde(default)]
    token: Option<String>,
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
    // Behind the nginx gateway (Docker) the socket peer is the gateway, and
    // `X-Real-IP` carries the real client. See `resolve_client_ip`.
    headers: HeaderMap,
    Query(query): Query<WsAuthQuery>,
    State(state): State<AppState>,
) -> axum::response::Response {
    let socket_ip = connect_info
        .map(|ext| ext.0 .0.ip())
        .unwrap_or_else(|| IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    let in_docker = crate::core::env::is_docker();
    let peer_ip = resolve_client_ip(&headers, socket_ip, in_docker);

    let config = state.config.read().await;
    // A browser always sends `Origin`: a foreign one is a page the operator
    // visits trying to reach the bus (cross-site WebSocket hijacking).
    let origin = classify_ws_origin(
        headers
            .get(axum::http::header::ORIGIN)
            .map(|value| value.to_str().unwrap_or("null")),
        headers
            .get(axum::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
        config.server.domain.as_deref(),
        config.server.listening_port(),
    );
    if origin == WsOrigin::Foreign {
        drop(config);
        tracing::warn!("WS: refusing upgrade from a foreign Origin (client {peer_ip})");
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }
    let auth_required = config.server.auth_enabled && config.server.auth_token.is_some();
    // Locked auth (token not decryptable): no connection is the trusted frontend.
    let auth_locked = config.server.auth_locked
        && config.server.auth_enabled
        && config.server.auth_token.is_none();
    let strict_localhost = config.server.auth_strict_localhost;
    let has_valid_token = {
        let expected = config.server.auth_token.as_deref();
        let bearer = crate::bearer_credential(&headers);
        bearer.is_some_and(|bearer| {
            crate::core::bridge_token::operator_token_matches(expected, bearer)
        }) || query
            .token
            .as_deref()
            .is_some_and(|token| crate::core::bridge_token::operator_token_matches(expected, token))
    };
    drop(config);

    // No Origin means a non-browser client (a peer, a CLI): never the frontend.
    let is_local = origin == WsOrigin::Allowed
        && !auth_locked
        && ws_client_is_local(WsTrust {
            client_ip: peer_ip,
            in_docker,
            auth_required,
            strict_localhost,
            has_valid_token,
        });
    ws.on_upgrade(move |socket| handle_socket(socket, state, peer_ip, is_local))
        .into_response()
}

/// Where a WS upgrade comes from, judged by its `Origin` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WsOrigin {
    /// No header: not a browser (a federation peer, a CLI, a test client).
    Absent,
    /// A page of this Kronn: its frontend, the desktop webview, a dev server.
    Allowed,
    /// Any other page, including the opaque `null` origin.
    Foreign,
}

/// Classify the upgrade's `Origin`. Allowed: the CORS list of `build_cors`,
/// the Tauri webview origins, a loopback page on any port (the Vite dev server
/// and the Docker gateway port vary), and a same-origin page whose host is an
/// IP literal (LAN or Tailscale access; an IP cannot be DNS-rebound).
pub(crate) fn classify_ws_origin(
    origin: Option<&str>,
    host_header: Option<&str>,
    domain: Option<&str>,
    port: u16,
) -> WsOrigin {
    let Some(origin) = origin.map(str::trim) else {
        return WsOrigin::Absent;
    };
    if crate::frontend_origins(&domain.map(str::to_owned), port)
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(origin))
    {
        return WsOrigin::Allowed;
    }
    let Some((scheme, host)) = origin_scheme_host(origin) else {
        return WsOrigin::Foreign;
    };
    let allowed = match scheme.as_str() {
        "tauri" => host == "localhost",
        "http" | "https" => {
            host == "tauri.localhost"
                || host_is_loopback(&host)
                || domain.is_some_and(|domain| domain.eq_ignore_ascii_case(&host))
                || (host.parse::<IpAddr>().is_ok()
                    && host_header
                        .and_then(authority_host)
                        .is_some_and(|request_host| request_host == host))
        }
        _ => false,
    };
    if allowed {
        WsOrigin::Allowed
    } else {
        WsOrigin::Foreign
    }
}

/// Lowercased scheme and host of a serialized origin (`scheme://host[:port]`).
fn origin_scheme_host(origin: &str) -> Option<(String, String)> {
    let (scheme, authority) = origin.split_once("://")?;
    if scheme.is_empty() || authority.contains(['/', '?', '#', '@']) {
        return None;
    }
    Some((scheme.to_ascii_lowercase(), authority_host(authority)?))
}

/// Host part of `host[:port]` or `[v6][:port]`, lowercased, brackets removed.
fn authority_host(authority: &str) -> Option<String> {
    let authority = authority.trim();
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split_once(']')?.0
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
            _ => authority,
        }
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn host_is_loopback(host: &str) -> bool {
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Resolve the real client IP for rate-limiting, ban and trust decisions.
///
/// `X-Real-IP` is honoured only under Docker AND when the TCP peer is itself
/// on a private network (the bundled nginx overwrites the header with
/// `$remote_addr`). Natively axum talks to clients directly, so the header is
/// attacker-controlled and ignored. `X-Forwarded-For` is never read: the
/// gateway does not set it, so it would pass through from the client as is.
pub(crate) fn resolve_client_ip(headers: &HeaderMap, socket_ip: IpAddr, in_docker: bool) -> IpAddr {
    if !in_docker || !is_trusted_client_ip(socket_ip) {
        return socket_ip;
    }
    headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
        .unwrap_or(socket_ip)
}

/// Inputs of the "is this the local frontend" decision for one WS upgrade.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WsTrust {
    pub client_ip: IpAddr,
    pub in_docker: bool,
    /// `auth_enabled` and a token configured, as in `auth_middleware`.
    pub auth_required: bool,
    pub strict_localhost: bool,
    pub has_valid_token: bool,
}

/// Whether the connection is the local frontend: it receives the full event
/// bus and may use the empty-invite Presence shortcut.
///
/// A valid token always qualifies. When auth is required, only the same local
/// set as the HTTP bypass qualifies (loopback, plus the Docker bridge range
/// under Docker), unless strict-localhost is on. When auth is off, natively
/// only loopback qualifies; under Docker any private source does, because
/// Docker Desktop NATs the host browser to a network gateway whose range
/// varies, and the HTTP API is open in that mode anyway.
pub(crate) fn ws_client_is_local(t: WsTrust) -> bool {
    if t.has_valid_token {
        return true;
    }
    let local_set = if t.in_docker {
        crate::is_local_ip(&t.client_ip.to_string())
    } else {
        t.client_ip.is_loopback()
    };
    if t.auth_required {
        return !t.strict_localhost && local_set;
    }
    if t.in_docker {
        is_trusted_client_ip(t.client_ip)
    } else {
        t.client_ip.is_loopback()
    }
}

/// Loopback OR private-range (RFC1918 / IPv6 ULA / link-local). Used for the
/// Docker gateway check and the Docker auth-off trust; never on its own to
/// grant trust natively.
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

async fn handle_socket(socket: WebSocket, state: AppState, peer_ip: IpAddr, is_local: bool) {
    // Reject up-front if this peer is currently banned for invite-code
    // brute-force. The local frontend (see `ws_client_is_local`) is exempt: it
    // is the only legitimate caller of the empty-invite shortcut, and behind
    // the Docker gateway many browsers share one source IP, so banning it
    // would reject every client (reconnect storm).
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
    // Nothing from the bus leaves before the client's Presence is verified.
    let verified_flag = Arc::new(AtomicBool::new(false));
    let send_verified = verified_flag.clone();

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
                        if !send_verified.load(Ordering::Acquire) {
                            continue;
                        }
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
        // Invite code of the verified peer, re-checked before each relayable frame.
        let mut peer_code: Option<String> = None;

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
                                if !admit_peer_presence(&state, from_invite_code, peer_ip, is_local)
                                    .await
                                {
                                    break;
                                }
                                peer_code = Some(from_invite_code.clone());
                            }
                            verified = true;
                            verified_flag.store(true, Ordering::Release);
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
                        // Only an accepted contact feeds shared discussions; the
                        // local frontend writes through the HTTP API.
                        let Some(code) = peer_code.as_deref() else {
                            tracing::warn!(
                                "WS: dropping a relayable frame from a non-peer client {peer_ip}"
                            );
                            continue;
                        };
                        if !contact_is_accepted(&state, code).await {
                            tracing::warn!(
                                "WS: peer {peer_ip} is no longer an accepted contact, closing"
                            );
                            break;
                        }
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

/// Whether `code` belongs to an accepted contact (DB errors deny).
async fn contact_is_accepted(state: &AppState, code: &str) -> bool {
    let code = code.to_owned();
    matches!(
        state
            .db
            .with_conn(move |conn| crate::db::contacts::authenticate_invite_code(conn, &code))
            .await,
        Ok(crate::db::contacts::InviteAuth::Accepted(_))
    )
}

/// Decide a peer's Presence: only an accepted contact is verified. An unknown
/// well-formed code is recorded as an incoming request (never contacted back
/// until the operator adds it), and every refusal counts toward the ban.
pub(crate) async fn admit_peer_presence(
    state: &AppState,
    invite_code: &str,
    peer_ip: IpAddr,
    is_local: bool,
) -> bool {
    let code = invite_code.to_owned();
    let verdict = state
        .db
        .with_conn(move |conn| crate::db::contacts::authenticate_invite_code(conn, &code))
        .await;
    match verdict {
        Ok(crate::db::contacts::InviteAuth::Accepted(_)) => return true,
        Ok(crate::db::contacts::InviteAuth::NotAccepted { pseudo, status }) => {
            tracing::warn!("WS: refusing contact {pseudo} from {peer_ip}: status {status}");
        }
        Ok(crate::db::contacts::InviteAuth::Unknown) => {
            match auto_add_peer(state, invite_code).await {
                Some(contact) => tracing::info!(
                    "WS: recorded a contact request from {} ({peer_ip})",
                    contact.pseudo
                ),
                None => tracing::warn!("WS: rejected invalid invite code from {peer_ip}"),
            }
        }
        Err(error) => tracing::warn!("WS: contact lookup failed for {peer_ip}: {error}"),
    }
    if !is_local && rate_limit::record_failure(peer_ip) {
        tracing::warn!("WS: peer {peer_ip} hit invite-code failure threshold and is now banned");
    }
    false
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
    channel: crate::models::MessageChannel,
    agent_type: Option<crate::models::AgentType>,
    targets: Vec<crate::models::MessageTarget>,
    reply_to_message_id: Option<&str>,
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
    let reply_to_message_id = reply_to_message_id.and_then(|target_id| {
        let target_exists = conn
            .query_row(
                "SELECT COUNT(*) > 0
                 FROM messages
                 WHERE id = ?1 AND discussion_id = ?2",
                rusqlite::params![target_id, disc_id],
                |row| row.get::<_, bool>(0),
            )
            .unwrap_or(false);
        if target_exists {
            Some(target_id.to_string())
        } else {
            tracing::warn!(
                "WS: reply target {} is unavailable in shared disc {}, preserving message without relation",
                target_id,
                shared_discussion_id
            );
            None
        }
    });
    let msg = crate::models::DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
        id: message_id.to_string(),
        role,
        channel,
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
        target_agent: targets.first().map(|target| target.agent_type.clone()),
        reply_to_message_id,
    };

    crate::db::discussions::insert_message(conn, &disc_id, &msg)?;
    crate::db::discussions::replace_message_targets(conn, message_id, &targets)?;
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
            channel,
            agent_type,
            target_agents,
            targets,
            reply_to_message_id,
            ..
        } => {
            let resolved_targets = if targets.is_empty() {
                target_agents
                    .iter()
                    .cloned()
                    .map(crate::models::MessageTarget::agent)
                    .collect()
            } else {
                targets.clone()
            };
            let (sid, mid, pseudo, avatar, text, ts, r, channel, at, targets, reply_to) = (
                shared_discussion_id.clone(),
                message_id.clone(),
                from_pseudo.clone(),
                from_avatar_email.clone(),
                content.clone(),
                *timestamp,
                role.clone(),
                *channel,
                agent_type.clone(),
                resolved_targets,
                reply_to_message_id.clone(),
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
                        channel,
                        at,
                        targets,
                        reply_to.as_deref(),
                    )
                })
                .await
                .unwrap_or(false)
        }
        WsMessage::MessageRevised {
            shared_discussion_id,
            event_id,
            target_message_id,
            previous_content_hash,
            expected_revision,
            revision,
            content,
            target_agent,
            target_agents,
            targets,
            idempotency_key,
            ..
        } => {
            let sid = shared_discussion_id.clone();
            let eid = event_id.clone();
            let mid = target_message_id.clone();
            let previous_hash = previous_content_hash.clone();
            let expected = expected_revision.clone();
            let revision_value = revision.clone();
            let revised_content = content.clone();
            let agent = target_agent.clone();
            let targets = if !targets.is_empty() {
                targets.clone()
            } else if target_agents.is_empty() {
                target_agent
                    .iter()
                    .cloned()
                    .map(crate::models::MessageTarget::agent)
                    .collect()
            } else {
                target_agents
                    .iter()
                    .cloned()
                    .map(crate::models::MessageTarget::agent)
                    .collect()
            };
            let key = idempotency_key.clone();
            state
                .db
                .with_conn(move |conn| {
                    let Some(discussion_id) =
                        crate::db::discussions::find_discussion_by_shared_id(conn, &sid)?
                    else {
                        return Ok(false);
                    };
                    let created_at = chrono::DateTime::parse_from_rfc3339(&revision_value)
                        .map(|value| value.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());
                    crate::db::discussions::apply_remote_message_revision_with_targets(
                        conn,
                        &crate::models::MessageRevisionEvent {
                            id: eid,
                            discussion_id,
                            target_message_id: mid,
                            previous_content_hash: previous_hash,
                            expected_revision: expected,
                            revision: revision_value,
                            content: revised_content,
                            target_agent: agent,
                            idempotency_key: key,
                            sort_order: 0,
                            dispatch_job_id: None,
                            created_at,
                        },
                        &targets,
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

/// Record an incoming contact request (`requested`) from an unknown invite code.
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
        status: crate::db::contacts::STATUS_REQUESTED.into(),
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
            channel: crate::models::MessageChannel::Main,
            agent_type: None,
            target_agents: vec![],
            targets: vec![],
            reply_to_message_id: None,
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
        const PUBLIC: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9));

        fn hdr(name: &'static str, val: &str) -> HeaderMap {
            let mut h = HeaderMap::new();
            h.insert(HeaderName::from_static(name), val.parse().unwrap());
            h
        }

        #[test]
        fn docker_gateway_x_real_ip_wins_over_socket() {
            // Behind nginx the socket IP is the gateway; X-Real-IP carries the
            // real client, so the gateway IP is never the one banned.
            let ip = resolve_client_ip(&hdr("x-real-ip", "127.0.0.1"), GATEWAY, true);
            assert_eq!(ip, IpAddr::V4(Ipv4Addr::LOCALHOST));
        }

        #[test]
        fn docker_gateway_x_real_ip_carries_a_lan_peer() {
            let ip = resolve_client_ip(&hdr("x-real-ip", "10.0.0.42"), GATEWAY, true);
            assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 42)));
        }

        #[test]
        fn native_ignores_forged_x_real_ip() {
            let ip = resolve_client_ip(&hdr("x-real-ip", "127.0.0.1"), PUBLIC, false);
            assert_eq!(ip, PUBLIC);
            let ip = resolve_client_ip(&hdr("x-real-ip", "127.0.0.1"), GATEWAY, false);
            assert_eq!(ip, GATEWAY);
        }

        #[test]
        fn docker_ignores_x_real_ip_from_a_public_peer() {
            // A public socket peer is not the gateway: its header is forged.
            let ip = resolve_client_ip(&hdr("x-real-ip", "127.0.0.1"), PUBLIC, true);
            assert_eq!(ip, PUBLIC);
        }

        #[test]
        fn x_forwarded_for_is_never_trusted() {
            let h = hdr("x-forwarded-for", "127.0.0.1, 172.19.0.4");
            assert_eq!(resolve_client_ip(&h, GATEWAY, true), GATEWAY);
            assert_eq!(resolve_client_ip(&h, PUBLIC, false), PUBLIC);
        }

        #[test]
        fn falls_back_to_socket_when_no_proxy_header() {
            assert_eq!(resolve_client_ip(&HeaderMap::new(), GATEWAY, true), GATEWAY);
        }

        #[test]
        fn garbage_header_falls_through_to_socket() {
            let ip = resolve_client_ip(&hdr("x-real-ip", "not-an-ip"), GATEWAY, true);
            assert_eq!(ip, GATEWAY);
        }
    }

    mod ws_client_is_local {
        use super::super::{ws_client_is_local, WsTrust};
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

        fn trust(ip: IpAddr, in_docker: bool, auth_required: bool) -> WsTrust {
            WsTrust {
                client_ip: ip,
                in_docker,
                auth_required,
                strict_localhost: false,
                has_valid_token: false,
            }
        }
        const LAN: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10));
        const BRIDGE: IpAddr = IpAddr::V4(Ipv4Addr::new(172, 18, 0, 1));
        const LOOP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

        #[test]
        fn native_lan_peer_without_token_is_not_local() {
            assert!(!ws_client_is_local(trust(LAN, false, true)));
            assert!(!ws_client_is_local(trust(LAN, false, false)));
            assert!(!ws_client_is_local(trust(BRIDGE, false, true)));
        }

        #[test]
        fn native_loopback_is_local() {
            assert!(ws_client_is_local(trust(LOOP, false, true)));
            assert!(ws_client_is_local(trust(
                IpAddr::V6(Ipv6Addr::LOCALHOST),
                false,
                false
            )));
        }

        #[test]
        fn valid_token_makes_a_lan_peer_local() {
            let mut t = trust(LAN, false, true);
            t.has_valid_token = true;
            assert!(ws_client_is_local(t));
        }

        #[test]
        fn strict_localhost_requires_the_token_even_on_loopback() {
            let mut t = trust(LOOP, false, true);
            t.strict_localhost = true;
            assert!(!ws_client_is_local(t));
            t.has_valid_token = true;
            assert!(ws_client_is_local(t));
        }

        #[test]
        fn docker_auth_on_trusts_only_the_http_local_set() {
            assert!(ws_client_is_local(trust(BRIDGE, true, true)));
            assert!(ws_client_is_local(trust(LOOP, true, true)));
            assert!(!ws_client_is_local(trust(LAN, true, true)));
        }

        #[test]
        fn docker_auth_off_keeps_private_sources_local() {
            // Docker Desktop NATs the host browser to a gateway in a varying range.
            assert!(ws_client_is_local(trust(LAN, true, false)));
            assert!(ws_client_is_local(trust(BRIDGE, true, false)));
            assert!(!ws_client_is_local(trust(
                IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
                true,
                false
            )));
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
            crate::models::MessageChannel::Main,
            None,
            vec![],
            None,
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
            crate::models::MessageChannel::Main,
            None,
            vec![],
            None,
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
            crate::models::MessageChannel::Main,
            None,
            vec![],
            None,
        )
        .unwrap());

        assert!(handle_incoming_chat_message(
            &c,
            "shared-2",
            "m2",
            "Romu",
            None,
            "reply",
            1,
            MessageRole::User,
            crate::models::MessageChannel::Main,
            None,
            vec![],
            Some("m1"),
        )
        .unwrap());
        let linked_target: Option<String> = c
            .query_row(
                "SELECT reply_to_message_id FROM messages WHERE id = 'm2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(linked_target.as_deref(), Some("m1"));

        assert!(handle_incoming_chat_message(
            &c,
            "shared-2",
            "m3",
            "Romu",
            None,
            "reply to missing",
            2,
            MessageRole::User,
            crate::models::MessageChannel::Main,
            None,
            vec![],
            Some("missing"),
        )
        .unwrap());
        let missing_target: Option<String> = c
            .query_row(
                "SELECT reply_to_message_id FROM messages WHERE id = 'm3'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(missing_target.is_none());

        assert!(handle_incoming_chat_message(
            &c,
            "shared-2",
            "m4",
            "Romu",
            None,
            "federated note",
            3,
            MessageRole::User,
            crate::models::MessageChannel::Note,
            None,
            vec![],
            None,
        )
        .unwrap());
        let channel: String = c
            .query_row("SELECT channel FROM messages WHERE id = 'm4'", [], |row| {
                row.get(0)
            })
            .unwrap();
        let awaiting_agent: bool = c
            .query_row(
                "SELECT awaiting_agent FROM discussions WHERE shared_id = 'shared-2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let dispatch_count: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM agent_dispatch_jobs
                 WHERE trigger_message_id = 'm4'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(channel, "note");
        assert!(!awaiting_agent);
        assert_eq!(dispatch_count, 0);
    }
}

#[cfg(test)]
mod origin_and_admission_tests {
    use super::*;

    fn classify(origin: &str, host: &str) -> WsOrigin {
        classify_ws_origin(Some(origin), Some(host), None, 3140)
    }

    #[test]
    fn no_origin_is_absent() {
        assert_eq!(
            classify_ws_origin(None, Some("127.0.0.1:3140"), None, 3140),
            WsOrigin::Absent
        );
    }

    #[test]
    fn foreign_and_opaque_origins_are_refused() {
        assert_eq!(
            classify("https://evil.example", "127.0.0.1:3140"),
            WsOrigin::Foreign
        );
        assert_eq!(classify("null", "127.0.0.1:3140"), WsOrigin::Foreign);
        assert_eq!(classify("file://", "127.0.0.1:3140"), WsOrigin::Foreign);
        assert_eq!(
            classify("http://localhost.evil.example", "localhost:3140"),
            WsOrigin::Foreign
        );
    }

    #[test]
    fn a_dns_rebound_name_is_refused_even_when_it_matches_host() {
        assert_eq!(
            classify(
                "http://rebind.evil.example:3140",
                "rebind.evil.example:3140"
            ),
            WsOrigin::Foreign
        );
    }

    #[test]
    fn local_frontend_origins_are_allowed() {
        for origin in [
            "http://localhost:3140",
            "http://127.0.0.1:3140",
            "http://localhost:5173",
            "http://[::1]:3140",
        ] {
            assert_eq!(
                classify(origin, "127.0.0.1:3140"),
                WsOrigin::Allowed,
                "{origin}"
            );
        }
    }

    #[test]
    fn the_desktop_webview_origins_are_allowed() {
        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
            "http://127.0.0.1:53591",
        ] {
            assert_eq!(
                classify_ws_origin(Some(origin), Some("127.0.0.1:53591"), None, 53591),
                WsOrigin::Allowed,
                "{origin}"
            );
        }
        assert_eq!(
            classify("tauri://evil", "127.0.0.1:3140"),
            WsOrigin::Foreign
        );
    }

    #[test]
    fn an_ip_literal_same_origin_page_is_allowed() {
        assert_eq!(
            classify("http://192.168.1.5:3140", "192.168.1.5:3140"),
            WsOrigin::Allowed
        );
        // nginx forwards `$host`, without the port.
        assert_eq!(
            classify("http://100.64.1.5:8080", "100.64.1.5"),
            WsOrigin::Allowed
        );
        assert_eq!(
            classify("http://192.168.1.6:3140", "192.168.1.5:3140"),
            WsOrigin::Foreign
        );
    }

    #[test]
    fn the_configured_domain_is_allowed() {
        assert_eq!(
            classify_ws_origin(
                Some("https://kronn.example.org"),
                Some("kronn.example.org"),
                Some("kronn.example.org"),
                3140
            ),
            WsOrigin::Allowed
        );
    }

    fn state() -> AppState {
        let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let config = Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        ));
        AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    async fn insert_contact(state: &AppState, code: &str, status: &str) {
        let contact = crate::models::Contact {
            id: uuid::Uuid::new_v4().to_string(),
            pseudo: "Peer".into(),
            avatar_email: None,
            kronn_url: "http://10.0.0.50:3456".into(),
            invite_code: code.into(),
            status: status.into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        state
            .db
            .with_conn(move |conn| crate::db::contacts::insert_contact(conn, &contact))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn an_unknown_well_formed_code_is_not_admitted_and_counts_toward_the_ban() {
        let state = state();
        let ip = IpAddr::V4(std::net::Ipv4Addr::new(10, 66, 0, 1));
        rate_limit::reset(ip);
        for n in 0..10 {
            let code = format!("kronn:Stranger{n}@10.0.0.{n}:3456");
            assert!(!admit_peer_presence(&state, &code, ip, false).await);
        }
        assert!(rate_limit::is_banned(ip), "unknown codes must count");
        let contacts = state
            .db
            .with_conn(crate::db::contacts::list_contacts)
            .await
            .unwrap();
        assert!(contacts
            .iter()
            .all(|c| c.status == crate::db::contacts::STATUS_REQUESTED));
        assert!(contacts
            .iter()
            .all(|c| !crate::db::contacts::dials_outbound(&c.status)));
        rate_limit::reset(ip);
    }

    #[tokio::test]
    async fn only_an_accepted_contact_is_admitted() {
        let state = state();
        let ip = IpAddr::V4(std::net::Ipv4Addr::new(10, 66, 0, 2));
        rate_limit::reset(ip);
        insert_contact(&state, "kronn:Ok@10.0.0.50:3456", "accepted").await;
        insert_contact(&state, "kronn:Wait@10.0.0.51:3456", "pending").await;
        insert_contact(&state, "kronn:Req@10.0.0.52:3456", "requested").await;
        insert_contact(&state, "kronn:No@10.0.0.53:3456", "refused").await;
        assert!(admit_peer_presence(&state, "kronn:Ok@10.0.0.50:3456", ip, false).await);
        for code in [
            "kronn:Wait@10.0.0.51:3456",
            "kronn:Req@10.0.0.52:3456",
            "kronn:No@10.0.0.53:3456",
        ] {
            assert!(
                !admit_peer_presence(&state, code, ip, false).await,
                "{code}"
            );
        }
        rate_limit::reset(ip);
    }
}
