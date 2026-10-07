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
use std::sync::Mutex;
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

    /// Count one refused admission before any side effect, and say whether
    /// the IP may still proceed (it is not banned once this one is counted).
    pub fn charge(ip: IpAddr) -> bool {
        if is_banned(ip) {
            return false;
        }
        if record_failure(ip) {
            tracing::warn!("WS: peer {ip} hit invite-code failure threshold and is now banned");
            return false;
        }
        true
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

/// Query string of the WS upgrade. `?token=` is what frontends before 0.14.3
/// sent; current ones offer the token as a subprotocol instead.
#[derive(Debug, Default, serde::Deserialize)]
pub struct WsAuthQuery {
    #[serde(default)]
    token: Option<String>,
}

/// GET /api/ws — WebSocket upgrade handler.
///
/// Accepts connections from:
/// - The local frontend: an allowed `Origin` plus the frontend credential
///   (token, or the loopback rule of `ws_client_is_local`). Full event bus.
/// - Federation peers, only when P2P is enabled: no browser origin, admitted
///   by an accepted contact's invite code. Peer-relayable frames only.
///
/// `ConnectInfo` is wrapped in `Option` so the handler also works in tests
/// that build the router without `into_make_service_with_connect_info`. When
/// the connect-info extension is missing we treat the connection as
/// loopback — this is safe because real production servers in `main.rs` and
/// `desktop/src-tauri/src/main.rs` always wire it.
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
    use axum::http::StatusCode;
    let socket_ip = connect_info
        .map(|ext| ext.0 .0.ip())
        .unwrap_or_else(|| IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    let in_docker = crate::core::env::is_docker();
    let peer_ip = resolve_client_ip(&headers, socket_ip, in_docker);
    let dev_ui = crate::core::child_env::var("KRONN_DEV_UI_URL").ok();

    let config = state.config.read().await;
    // A browser always sends `Origin`: a foreign one is a page the operator
    // visits trying to reach the bus (cross-site WebSocket hijacking).
    let origin = classify_ws_origin(
        headers
            .get(axum::http::header::ORIGIN)
            .map(|value| value.to_str().unwrap_or("null")),
        &allowed_ws_origins(&config.server, dev_ui.as_deref(), in_docker),
    );
    if origin == WsOrigin::Foreign {
        drop(config);
        tracing::warn!("WS: refusing upgrade from a foreign Origin (client {peer_ip})");
        return StatusCode::FORBIDDEN.into_response();
    }
    let auth_required = config.server.auth_enabled && config.server.auth_token.is_some();
    // Locked auth (token not decryptable): no connection is the trusted frontend.
    let auth_locked = config.server.auth_locked
        && config.server.auth_enabled
        && config.server.auth_token.is_none();
    let strict_localhost = config.server.auth_strict_localhost;
    let p2p_enabled = state.p2p.enabled();
    let has_valid_token =
        {
            let expected = config.server.auth_token.as_deref();
            let bearer = crate::bearer_credential(&headers);
            bearer.is_some_and(|bearer| {
                crate::core::bridge_token::operator_token_matches(expected, bearer)
            }) || query.token.as_deref().is_some_and(|token| {
                crate::core::bridge_token::operator_token_matches(expected, token)
            }) || subprotocol_credential(&headers).is_some_and(|token| {
                crate::core::bridge_token::operator_token_matches(expected, &token)
            })
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
    if !is_local {
        // Peers carry no secret yet: federation stays off unless enabled.
        if !p2p_enabled || origin == WsOrigin::Allowed {
            tracing::warn!(
                "WS: refusing a non-frontend client {peer_ip} (P2P off or no frontend credential)"
            );
            return StatusCode::FORBIDDEN.into_response();
        }
        if rate_limit::is_banned(peer_ip) {
            tracing::warn!("WS: rejecting banned peer {peer_ip}");
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
    }
    // Every connection holds a slot until its Presence is admitted.
    let Some(slot) = pending::acquire(peer_ip) else {
        tracing::warn!("WS: too many unadmitted connections from {peer_ip}");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    ws.protocols([WS_PROTOCOL])
        .on_upgrade(move |socket| handle_socket(socket, state, peer_ip, is_local, slot))
        .into_response()
}

/// Subprotocol the server selects; the credential rides beside it.
pub(crate) const WS_PROTOCOL: &str = "kronn";
/// Prefix of the subprotocol that carries the operator token, base64url
/// encoded: a browser cannot set headers on a WebSocket, and a URL query
/// ends up in proxy logs.
pub(crate) const WS_AUTH_PROTOCOL_PREFIX: &str = "kronn.auth.";

/// The operator token offered as a `kronn.auth.<base64url>` subprotocol.
pub(crate) fn subprotocol_credential(headers: &HeaderMap) -> Option<String> {
    use base64::Engine as _;
    headers
        .get_all(axum::http::header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .find_map(|item| item.trim().strip_prefix(WS_AUTH_PROTOCOL_PREFIX))
        .and_then(|encoded| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(encoded)
                .ok()
        })
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// Where a WS upgrade comes from, judged by its `Origin` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WsOrigin {
    /// No header: not a browser (a federation peer, a CLI, a test client).
    Absent,
    /// An exact origin of this Kronn's frontend.
    Allowed,
    /// Any other page, including the opaque `null` origin.
    Foreign,
}

/// Exact frontend origins: the CORS list for the listening port (the desktop
/// webview included), the gateway ports under Docker only, the configured domain, the Tauri
/// webview, the dev UI when `kronn start-dev` exported it, and the operator's
/// own list (LAN or Tailscale aliases).
pub(crate) fn allowed_ws_origins(
    server: &crate::models::ServerConfig,
    dev_ui: Option<&str>,
    in_docker: bool,
) -> Vec<String> {
    let port = server.listening_port();
    let mut origins = crate::frontend_origins(&None, port, in_docker);
    if server.domain.is_some() {
        origins.extend(crate::frontend_origins(&server.domain, port, in_docker));
    }
    origins.extend(
        [
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
        ]
        .map(String::from),
    );
    if let Some(dev) = dev_ui.and_then(normalize_origin) {
        for alias in [
            ("://localhost:", "://127.0.0.1:"),
            ("://127.0.0.1:", "://localhost:"),
        ] {
            if dev.contains(alias.0) {
                origins.push(dev.replacen(alias.0, alias.1, 1));
            }
        }
        origins.push(dev);
    }
    origins.extend(server.frontend_origins.iter().cloned());
    origins.iter().filter_map(|o| normalize_origin(o)).collect()
}

/// Classify the upgrade's `Origin` against the exact allowed list.
pub(crate) fn classify_ws_origin(origin: Option<&str>, allowed: &[String]) -> WsOrigin {
    let Some(origin) = origin else {
        return WsOrigin::Absent;
    };
    match normalize_origin(origin) {
        Some(origin) if allowed.contains(&origin) => WsOrigin::Allowed,
        _ => WsOrigin::Foreign,
    }
}

/// Canonical `scheme://host[:port]` (lowercase, default port dropped, a
/// trailing slash allowed), or None for anything else: paths, queries,
/// credentials, `null`, or an authority the URL parser does not read whole.
pub(crate) fn normalize_origin(origin: &str) -> Option<String> {
    let origin = origin.trim();
    if origin.contains(['\\', ' ', '\t']) {
        return None;
    }
    if origin.eq_ignore_ascii_case("tauri://localhost") {
        return Some("tauri://localhost".into());
    }
    let url = reqwest::Url::parse(origin).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.host().is_none()
    {
        return None;
    }
    // The parser forgives trailing garbage after an IPv6 literal or a port in
    // some forms: the serialization must give back what was written.
    let canonical = url.origin().ascii_serialization();
    let written = origin
        .strip_suffix('/')
        .unwrap_or(origin)
        .to_ascii_lowercase();
    let explicit_default = match url.scheme() {
        "http" => format!("{canonical}:80"),
        _ => format!("{canonical}:443"),
    };
    (written == canonical || written == explicit_default).then_some(canonical)
}

/// Unadmitted connections per client IP. The slot is released when the
/// Presence is admitted or the socket ends, whichever comes first.
mod pending {
    use std::collections::HashMap;
    use std::net::IpAddr;
    use std::sync::{Mutex, OnceLock};

    /// Most sockets one IP may hold open before its Presence is admitted.
    pub const MAX_PER_IP: u32 = 16;

    fn counts() -> &'static Mutex<HashMap<IpAddr, u32>> {
        static COUNTS: OnceLock<Mutex<HashMap<IpAddr, u32>>> = OnceLock::new();
        COUNTS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub struct Slot(IpAddr);

    pub fn acquire(ip: IpAddr) -> Option<Slot> {
        let mut map = counts().lock().unwrap_or_else(|p| p.into_inner());
        let count = map.entry(ip).or_insert(0);
        if *count >= MAX_PER_IP {
            return None;
        }
        *count += 1;
        Some(Slot(ip))
    }

    impl Drop for Slot {
        fn drop(&mut self) {
            let mut map = counts().lock().unwrap_or_else(|p| p.into_inner());
            if let Some(count) = map.get_mut(&self.0) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    map.remove(&self.0);
                }
            }
        }
    }
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

/// Who a connection was admitted as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Admission {
    Frontend,
    /// An accepted contact, by the invite code it presented.
    Peer(String),
}

/// A client must present itself within this window.
const PRESENCE_DEADLINE: Duration = Duration::from_secs(10);
/// How often a quiet peer's authorization is read again.
pub(crate) const PEER_RECHECK_INTERVAL: Duration = Duration::from_secs(5);
/// Minimum spacing between two answered heartbeats.
const HEARTBEAT_MIN_INTERVAL: Duration = Duration::from_secs(1);

/// The send half, the receive half and the peer re-check run as futures of
/// this one task: when any ends, the others are dropped with the socket, so
/// nothing outlives a revoked contact.
async fn handle_socket(
    socket: WebSocket,
    state: AppState,
    peer_ip: IpAddr,
    is_local: bool,
    slot: pending::Slot,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let mut broadcast_rx = state.ws_broadcast.subscribe();
    let (admission_tx, admission_rx) = tokio::sync::watch::channel::<Option<Admission>>(None);
    let watch_rx = admission_rx.clone();
    // Heartbeat answers go to this socket only, never onto the shared bus.
    let (direct_tx, mut direct_rx) = tokio::sync::mpsc::channel::<WsMessage>(4);
    // Keys of relayable frames received from this peer, never echoed back.
    let echo_guard = Mutex::new(PeerEchoGuard::default());

    let send = async {
        // A peer's socket is nearly silent; pinging keeps middleboxes (WSL2's
        // NAT drops idle TCP in ~5 s) from killing it without a Close frame.
        // The frontend pings on its own every 30 s.
        let keepalive_every = if is_local {
            Duration::from_secs(86_400)
        } else {
            crate::core::ws_client::WS_KEEPALIVE_INTERVAL
        };
        let mut keepalive = tokio::time::interval(keepalive_every);
        keepalive.tick().await;
        loop {
            let msg = tokio::select! {
                _ = keepalive.tick() => {
                    if ws_sender.send(Message::Ping(Vec::<u8>::new().into())).await.is_err() {
                        break;
                    }
                    continue;
                }
                Some(direct) = direct_rx.recv() => direct,
                recv = broadcast_rx.recv() => match recv {
                    Ok(msg) => {
                        let admission = admission_rx.borrow().clone();
                        match admission {
                            None => continue,
                            Some(Admission::Frontend) => {}
                            Some(Admission::Peer(code)) => {
                                // A peer only gets relayable frames, never its own echo.
                                if !msg.is_peer_relayable() {
                                    continue;
                                }
                                if let Some(key) = msg.relay_dedup_key() {
                                    if echo_guard.lock().unwrap_or_else(|e| e.into_inner()).contains(&key) {
                                        continue;
                                    }
                                }
                                // Only a member of that shared discussion gets it.
                                match PeerAuth::InviteCode(code).may_receive(&state, &msg).await {
                                    None => break,
                                    Some(false) => continue,
                                    Some(true) => {}
                                }
                            }
                        }
                        msg
                    }
                    // Fell behind a burst: skip the gap, keep the socket.
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            };
            if let Ok(json) = serde_json::to_string(&msg) {
                if ws_sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    };

    let recv = async {
        let mut slot = Some(slot);
        let mut peer_code: Option<String> = None;
        let mut last_heartbeat: Option<tokio::time::Instant> = None;
        let presence_deadline = tokio::time::Instant::now() + PRESENCE_DEADLINE;
        // The frontend may stay quiet for long; a peer must send something
        // (its keepalive counts) within WS_IDLE_TIMEOUT.
        let idle = if is_local {
            Duration::from_secs(86_400)
        } else {
            crate::core::ws_client::WS_IDLE_TIMEOUT
        };
        loop {
            let wait = if slot.is_none() {
                idle
            } else {
                presence_deadline.saturating_duration_since(tokio::time::Instant::now())
            };
            let frame = match tokio::time::timeout(wait, ws_receiver.next()).await {
                Err(_) | Ok(None) | Ok(Some(Err(_))) => break,
                Ok(Some(Ok(frame))) => frame,
            };
            let text = match frame {
                Message::Text(text) => text,
                Message::Close(_) => break,
                _ => continue,
            };
            let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) else {
                continue;
            };

            // Heartbeats are answered before admission (a peer resuming from
            // suspend races its Presence, TD-20260504), on this socket only.
            if classify_pre_presence(&ws_msg) == PrePresenceAction::Heartbeat {
                if let WsMessage::Ping { timestamp } = ws_msg {
                    if last_heartbeat.is_none_or(|at| at.elapsed() >= HEARTBEAT_MIN_INTERVAL) {
                        last_heartbeat = Some(tokio::time::Instant::now());
                        let _ = direct_tx.try_send(WsMessage::Pong { timestamp });
                    }
                }
                continue;
            }

            if slot.is_some() {
                let WsMessage::Presence {
                    ref from_invite_code,
                    ..
                } = ws_msg
                else {
                    tracing::debug!("WS: ignoring pre-presence frame from {peer_ip}");
                    continue;
                };
                let Some(admission) =
                    admit_presence(&state, from_invite_code, peer_ip, is_local).await
                else {
                    break;
                };
                slot = None;
                if let Admission::Peer(code) = &admission {
                    peer_code = Some(code.clone());
                    // The contact comes online for the local frontend.
                    let _ = state.ws_broadcast.send(ws_msg.clone());
                }
                admission_tx.send_replace(Some(admission));
                continue;
            }

            // Only an accepted contact feeds shared discussions; the frontend
            // writes through the HTTP API and sends nothing else here.
            let Some(code) = peer_code.as_deref() else {
                continue;
            };
            if !ws_msg.is_peer_relayable() {
                continue;
            }
            match ingest_relayable_frame(&state, &ws_msg, &PeerAuth::InviteCode(code.to_owned()))
                .await
            {
                Ingest::Revoked => {
                    tracing::warn!("WS: peer {peer_ip} is no longer authorized, closing");
                    break;
                }
                Ingest::Skip => {}
                Ingest::Broadcast => {
                    // Recorded before broadcasting so the send half never
                    // echoes it straight back.
                    if let Some(key) = ws_msg.relay_dedup_key() {
                        echo_guard
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .record(key);
                    }
                    let _ = state.ws_broadcast.send(ws_msg);
                }
            }
        }
    };

    // Revocation also ends a peer that only sends keepalives.
    let recheck = async {
        let mut every = tokio::time::interval(PEER_RECHECK_INTERVAL);
        every.tick().await;
        loop {
            every.tick().await;
            let admission = watch_rx.borrow().clone();
            if let Some(Admission::Peer(code)) = admission {
                if !peer_authorized(&state, &code).await {
                    tracing::warn!("WS: peer {peer_ip} was revoked, closing");
                    break;
                }
            }
        }
    };

    // The shared federation cancellation: a peer ends as soon as P2P is off.
    let p2p_off = async {
        if is_local {
            std::future::pending::<()>().await;
        }
        crate::api::federation::until_p2p_off(&state).await;
    };

    tokio::select! {
        _ = send => {}
        _ = recv => {}
        _ = recheck => {}
        _ = p2p_off => {}
    }
}

/// P2P is enabled and `code` is an accepted contact.
pub(crate) async fn peer_authorized(state: &AppState, code: &str) -> bool {
    PeerAuth::InviteCode(code.to_owned()).check(state).await
}

/// Decide a connection's Presence. The empty code is the frontend's, valid
/// only on a connection already trusted as the frontend at upgrade.
pub(crate) async fn admit_presence(
    state: &AppState,
    invite_code: &str,
    peer_ip: IpAddr,
    is_local: bool,
) -> Option<Admission> {
    if should_reject_empty_invite(invite_code, is_local) {
        tracing::warn!("WS: rejecting empty invite_code from non-frontend client {peer_ip}");
        rate_limit::charge(peer_ip);
        return None;
    }
    if invite_code.is_empty() {
        return Some(Admission::Frontend);
    }
    if invite_code.len() > crate::db::contacts::MAX_INVITE_CODE_LEN {
        rate_limit::charge(peer_ip);
        return None;
    }
    if !state.p2p.enabled() {
        return None;
    }
    admit_peer_presence(state, invite_code, peer_ip)
        .await
        .then(|| Admission::Peer(invite_code.to_owned()))
}

/// Decide a peer's Presence: only an accepted contact is admitted. Every
/// refusal is charged to the IP before any side effect; an unknown
/// well-formed code is then recorded as a contact request (never dialled nor
/// admitted until the operator adds it), within the request quota.
pub(crate) async fn admit_peer_presence(
    state: &AppState,
    invite_code: &str,
    peer_ip: IpAddr,
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
            if rate_limit::charge(peer_ip) {
                record_contact_request(state, invite_code, peer_ip).await;
            }
            return false;
        }
        Err(error) => tracing::warn!("WS: contact lookup failed for {peer_ip}: {error}"),
    }
    rate_limit::charge(peer_ip);
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
    inviter_contact_id: &str,
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

    crate::db::discussions::ensure_mirror_with_host(
        conn,
        shared_discussion_id,
        title,
        from_pseudo,
        inviter_contact_id,
        false,
    )?;
    tracing::info!(
        "WS: created shared discussion '{}' from invite by {}",
        title,
        from_pseudo
    );
    Ok(true)
}

/// The peer a relayable frame comes from, authorized again at each write.
#[derive(Debug, Clone)]
pub(crate) enum PeerAuth {
    /// Inbound: the invite code its Presence was admitted with.
    InviteCode(String),
    /// Outbound: the contact this instance dialled.
    ContactId(String),
}

impl PeerAuth {
    /// The id of the accepted contact behind this peer, or None once it is
    /// no longer accepted.
    pub(crate) fn accepted_contact_id(
        &self,
        conn: &rusqlite::Connection,
    ) -> anyhow::Result<Option<String>> {
        match self {
            Self::InviteCode(code) => Ok(
                match crate::db::contacts::authenticate_invite_code(conn, code)? {
                    crate::db::contacts::InviteAuth::Accepted(contact) => Some(contact.id),
                    _ => None,
                },
            ),
            Self::ContactId(id) => {
                Ok(crate::db::contacts::contact_id_is_accepted(conn, id)?.then(|| id.clone()))
            }
        }
    }

    /// Still an accepted contact (P2P being on is checked by the callers).
    pub(crate) fn authorized(&self, conn: &rusqlite::Connection) -> anyhow::Result<bool> {
        Ok(self.accepted_contact_id(conn)?.is_some())
    }

    /// None once the peer is no longer accepted; otherwise whether it is a
    /// member of the shared discussion `shared_id`.
    pub(crate) fn membership(
        &self,
        conn: &rusqlite::Connection,
        shared_id: &str,
    ) -> anyhow::Result<Option<bool>> {
        let Some(contact_id) = self.accepted_contact_id(conn)? else {
            return Ok(None);
        };
        let member = crate::db::discussions::shared_member(conn, shared_id, &contact_id)?;
        if !member {
            tracing::warn!(
                "federation: contact {contact_id} is not a member of shared discussion {shared_id}"
            );
        }
        Ok(Some(member))
    }

    /// `membership` with P2P on; None when P2P is off or the DB fails.
    pub(crate) async fn check_member(&self, state: &AppState, shared_id: &str) -> Option<bool> {
        if !state.p2p.enabled() {
            return None;
        }
        let (peer, sid, gate) = (self.clone(), shared_id.to_owned(), state.p2p.clone());
        let member = state
            .db
            .with_conn(move |conn| {
                let on = gate.read();
                if !*on {
                    return Ok(None);
                }
                peer.membership(conn, &sid)
            })
            .await
            .ok()
            .flatten();
        if !state.p2p.enabled() {
            return None;
        }
        member
    }

    /// Whether `msg` may go to this peer: None to close the connection.
    pub(crate) async fn may_receive(&self, state: &AppState, msg: &WsMessage) -> Option<bool> {
        match relay_shared_id(msg) {
            Some(shared_id) => self.check_member(state, shared_id).await,
            None => Some(false),
        }
    }

    /// P2P on and still an accepted contact (DB errors deny).
    pub(crate) async fn check(&self, state: &AppState) -> bool {
        if !state.p2p.enabled() {
            return false;
        }
        let (peer, gate) = (self.clone(), state.p2p.clone());
        let authorized = state
            .db
            .with_conn(move |conn| {
                let on = gate.read();
                Ok(*on && peer.authorized(conn)?)
            })
            .await
            .unwrap_or(false);
        // The DB may have been busy while P2P was turned off.
        authorized && state.p2p.enabled()
    }
}

/// The shared discussion a relayable frame belongs to.
pub(crate) fn relay_shared_id(msg: &WsMessage) -> Option<&str> {
    match msg {
        WsMessage::ChatMessage {
            shared_discussion_id,
            ..
        }
        | WsMessage::MessageRevised {
            shared_discussion_id,
            ..
        }
        | WsMessage::DiscussionInvite {
            shared_discussion_id,
            ..
        }
        | WsMessage::DiscSyncRequest {
            shared_discussion_id,
            ..
        }
        | WsMessage::FileAttached {
            shared_discussion_id,
            ..
        } => Some(shared_discussion_id),
        _ => None,
    }
}

/// What to do with an ingested relayable frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ingest {
    /// New: broadcast it on the local bus.
    Broadcast,
    /// Duplicate, dropped or consumed: do not broadcast.
    Skip,
    /// The peer lost its authorization: close the connection.
    Revoked,
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
pub(crate) async fn ingest_relayable_frame(
    state: &AppState,
    msg: &WsMessage,
    peer: &PeerAuth,
) -> Ingest {
    match ingest_authorized(state, msg, peer).await {
        Some(true) => Ingest::Broadcast,
        Some(false) => Ingest::Skip,
        None => Ingest::Revoked,
    }
}

/// `None` when the peer is no longer authorized. Each write checks the peer
/// inside the same DB call, so a revocation cannot slip between the two.
async fn ingest_authorized(state: &AppState, msg: &WsMessage, peer: &PeerAuth) -> Option<bool> {
    if !state.p2p.enabled() {
        return None;
    }
    let peer = peer.clone();
    let gate = state.p2p.clone();
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
                    // Held through the write: P2P cannot turn off mid-way.
                    let on = gate.read();
                    if !*on {
                        return Ok(None);
                    }
                    match peer.membership(conn, &sid)? {
                        None => return Ok(None),
                        Some(false) => return Ok(Some(false)),
                        Some(true) => {}
                    }
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
                    .map(Some)
                })
                .await
                .unwrap_or(Some(false))
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
                    // Held through the write: P2P cannot turn off mid-way.
                    let on = gate.read();
                    if !*on {
                        return Ok(None);
                    }
                    match peer.membership(conn, &sid)? {
                        None => return Ok(None),
                        Some(false) => return Ok(Some(false)),
                        Some(true) => {}
                    }
                    let Some(discussion_id) =
                        crate::db::discussions::find_discussion_by_shared_id(conn, &sid)?
                    else {
                        return Ok(Some(false));
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
                    .map(Some)
                })
                .await
                .unwrap_or(Some(false))
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
                .with_conn(move |conn| {
                    // Any accepted contact may invite: the mirror records it
                    // as the host, its only member.
                    // Held through the write: P2P cannot turn off mid-way.
                    let on = gate.read();
                    if !*on {
                        return Ok(None);
                    }
                    let Some(inviter) = peer.accepted_contact_id(conn)? else {
                        return Ok(None);
                    };
                    handle_discussion_invite(conn, &sid, &t, &p, &inviter).map(Some)
                })
                .await
                .unwrap_or(Some(false))
        }
        WsMessage::DiscSyncRequest {
            shared_discussion_id,
            since_timestamp,
        } => {
            // Answer with the missing messages (broadcast → relayed back to the
            // requester). The request itself is NEVER re-broadcast (return
            // false) — it is consumed here, so it can't bounce between peers.
            match peer.check_member(state, shared_discussion_id).await {
                None => return None,
                Some(false) => return Some(false),
                Some(true) => {}
            }
            crate::api::federation::respond_to_sync_request(
                state,
                shared_discussion_id,
                *since_timestamp,
            )
            .await;
            Some(false)
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
                let (fid, sid, mid) = (
                    file_id.clone(),
                    shared_discussion_id.clone(),
                    message_id.clone(),
                );
                state
                    .db
                    .with_conn(move |conn| {
                        let on = gate.read();
                        if !*on {
                            return Ok(None);
                        }
                        match peer.membership(conn, &sid)? {
                            None => return Ok(None),
                            // Not a member: handled like a file already held.
                            Some(false) => return Ok(Some(true)),
                            Some(true) => {}
                        }
                        // The message must belong to that same discussion.
                        let in_discussion =
                            match crate::db::discussions::find_discussion_by_shared_id(conn, &sid)? {
                                Some(did) => {
                                    crate::db::discussions::message_in_discussion(conn, &did, &mid)?
                                }
                                None => false,
                            };
                        if !in_discussion {
                            tracing::warn!("F8: FileAttached names message {mid} outside shared discussion {sid}");
                            return Ok(Some(true));
                        }
                        crate::db::discussions::context_file_exists(conn, &fid)
                            .map(Some)
                            .map_err(|e| anyhow::anyhow!(e))
                    })
                    .await
                    .unwrap_or(Some(true))
            };
            match exists {
                None => return None,
                Some(true) => return Some(false),
                Some(false) => {}
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
            // Turning P2P off cancels the transfer.
            tokio::spawn(async move {
                tokio::select! {
                    _ = crate::api::federation::fetch_and_store_attachment(
                        &st, &sid, &mid, &fid, &fname, &mime, sz, &host,
                    ) => {}
                    _ = crate::api::federation::until_p2p_off(&st) => {
                        tracing::info!("F8: P2P turned off, attachment {fid} transfer cancelled");
                    }
                }
            });
            Some(false)
        }
        _ => Some(false),
    }
}

/// Record an incoming contact request (`requested`) from an unknown invite
/// code, unless the code is malformed or the request quota is full.
async fn record_contact_request(state: &AppState, invite_code: &str, peer_ip: IpAddr) {
    let Some((pseudo, kronn_url)) = crate::db::contacts::parse_invite_code(invite_code) else {
        tracing::warn!("WS: rejected invalid invite code from {peer_ip}");
        return;
    };
    let now = Utc::now();
    let contact = crate::models::Contact {
        id: uuid::Uuid::new_v4().to_string(),
        pseudo,
        avatar_email: None,
        kronn_url,
        invite_code: invite_code.trim().to_string(),
        status: crate::db::contacts::STATUS_REQUESTED.into(),
        created_at: now,
        updated_at: now,
    };
    let pseudo = contact.pseudo.clone();
    let gate = state.p2p.clone();
    match state
        .db
        .with_conn(move |conn| {
            let on = gate.read();
            if !*on {
                return Ok(false);
            }
            crate::db::contacts::insert_contact_request(conn, &contact)
        })
        .await
    {
        Ok(true) => tracing::info!("WS: recorded a contact request from {pseudo} ({peer_ip})"),
        Ok(false) => {
            tracing::warn!("WS: contact request quota full, dropping {pseudo} ({peer_ip})")
        }
        Err(error) => tracing::warn!("WS: could not record a contact request: {error}"),
    }
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
        assert!(handle_discussion_invite(&c, "shared-1", "Title", "Romu", "contact-romu").unwrap());
        // Same shared_id again → already known → NOT new → no re-broadcast (loop dies).
        assert!(
            !handle_discussion_invite(&c, "shared-1", "Title", "Romu", "contact-romu").unwrap()
        );
    }

    #[test]
    fn a_mirror_has_its_inviter_as_only_member() {
        let c = conn();
        assert!(handle_discussion_invite(&c, "shared-m", "T", "Host", "contact-host").unwrap());
        assert!(crate::db::discussions::shared_member(&c, "shared-m", "contact-host").unwrap());
        // Another contact re-announcing the same share does not join it.
        assert!(!handle_discussion_invite(&c, "shared-m", "T", "Bob", "contact-bob").unwrap());
        assert!(!crate::db::discussions::shared_member(&c, "shared-m", "contact-bob").unwrap());
        // An explicit join adopts only a mirror that has no member yet.
        crate::db::discussions::ensure_mirror_with_host(
            &c,
            "shared-m",
            "T",
            "Bob",
            "contact-bob",
            true,
        )
        .unwrap();
        assert!(!crate::db::discussions::shared_member(&c, "shared-m", "contact-bob").unwrap());
        let legacy =
            crate::db::discussions::ensure_mirror_by_shared_id(&c, "shared-old", "T", "H").unwrap();
        assert!(!crate::db::discussions::shared_member(&c, "shared-old", "contact-host").unwrap());
        crate::db::discussions::ensure_mirror_with_host(
            &c,
            "shared-old",
            "T",
            "H",
            "contact-host",
            true,
        )
        .unwrap();
        assert!(crate::db::discussions::shared_member(&c, "shared-old", "contact-host").unwrap());
        let _ = legacy;
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
        assert!(handle_discussion_invite(&c, "shared-2", "T", "Romu", "contact-romu").unwrap());
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
    use std::sync::Arc;

    fn server() -> crate::models::ServerConfig {
        crate::core::config::default_config().server
    }

    fn classify(origin: &str, server: &crate::models::ServerConfig) -> WsOrigin {
        classify_ws_origin(Some(origin), &allowed_ws_origins(server, None, false))
    }

    #[test]
    fn no_origin_is_absent() {
        assert_eq!(
            classify_ws_origin(None, &allowed_ws_origins(&server(), None, false)),
            WsOrigin::Absent
        );
    }

    #[test]
    fn foreign_and_opaque_origins_are_refused() {
        let server = server();
        for origin in [
            "https://evil.example",
            "null",
            "file://",
            "http://localhost.evil.example:3140",
            "http://rebind.evil.example:3140",
        ] {
            assert_eq!(classify(origin, &server), WsOrigin::Foreign, "{origin}");
        }
    }

    #[test]
    fn another_local_port_is_refused() {
        let server = server();
        for origin in [
            "http://localhost:8000",
            "http://127.0.0.1:8000",
            "http://localhost:5173",
            "http://[::1]:3140",
        ] {
            assert_eq!(classify(origin, &server), WsOrigin::Foreign, "{origin}");
        }
    }

    #[test]
    fn the_frontend_origins_for_the_listening_port_are_allowed() {
        let mut server = server();
        for origin in ["http://localhost:3140", "http://127.0.0.1:3140"] {
            assert_eq!(classify(origin, &server), WsOrigin::Allowed, "{origin}");
        }
        // The desktop's runtime port, and nothing else.
        server.runtime_port = Some(53591);
        assert_eq!(
            classify("http://127.0.0.1:53591", &server),
            WsOrigin::Allowed
        );
        assert_eq!(
            classify("http://127.0.0.1:53592", &server),
            WsOrigin::Foreign
        );
    }

    #[test]
    fn the_gateway_ports_are_trusted_only_under_docker() {
        let mut desktop = server();
        desktop.runtime_port = Some(53591);
        for origin in [
            "http://localhost:3141",
            "http://127.0.0.1:3141",
            "http://localhost:3140",
        ] {
            assert_eq!(classify(origin, &desktop), WsOrigin::Foreign, "{origin}");
            assert_eq!(
                classify_ws_origin(Some(origin), &allowed_ws_origins(&server(), None, true)),
                WsOrigin::Allowed,
                "{origin} under Docker"
            );
        }
    }

    #[test]
    fn the_desktop_webview_origins_are_allowed() {
        let server = server();
        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
        ] {
            assert_eq!(classify(origin, &server), WsOrigin::Allowed, "{origin}");
        }
        assert_eq!(classify("tauri://evil", &server), WsOrigin::Foreign);
    }

    #[test]
    fn the_dev_ui_is_allowed_only_when_exported() {
        let server = server();
        let with_dev = allowed_ws_origins(&server, Some("http://localhost:5173/"), false);
        for origin in ["http://localhost:5173", "http://127.0.0.1:5173"] {
            assert_eq!(
                classify_ws_origin(Some(origin), &with_dev),
                WsOrigin::Allowed
            );
        }
        assert_eq!(
            classify("http://localhost:5173", &server),
            WsOrigin::Foreign
        );
    }

    #[test]
    fn the_configured_domain_keeps_its_port() {
        let mut server = server();
        server.domain = Some("kronn.example.org".into());
        assert_eq!(
            classify("https://kronn.example.org", &server),
            WsOrigin::Allowed
        );
        assert_eq!(
            classify("http://kronn.example.org:3140", &server),
            WsOrigin::Allowed
        );
        assert_eq!(
            classify("http://kronn.example.org:8000", &server),
            WsOrigin::Foreign
        );
    }

    #[test]
    fn a_listed_lan_alias_is_allowed_and_an_unlisted_one_refused() {
        let mut server = server();
        server.frontend_origins = vec!["http://kronn-mac.tail1234.ts.net:3140".into()];
        assert_eq!(
            classify("http://kronn-mac.tail1234.ts.net:3140", &server),
            WsOrigin::Allowed
        );
        assert_eq!(
            classify("http://192.168.1.5:3140", &server),
            WsOrigin::Foreign
        );
        assert_eq!(
            classify("http://kronn-mac.tail1234.ts.net:8000", &server),
            WsOrigin::Foreign
        );
    }

    #[test]
    fn the_operator_token_is_read_from_the_subprotocol() {
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("op-token");
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::SEC_WEBSOCKET_PROTOCOL,
            format!("kronn, kronn.auth.{encoded}").parse().unwrap(),
        );
        assert_eq!(
            subprotocol_credential(&headers).as_deref(),
            Some("op-token")
        );
        assert_eq!(subprotocol_credential(&HeaderMap::new()), None);
    }

    #[test]
    fn origins_are_normalized() {
        assert_eq!(
            normalize_origin("HTTP://Host:80/").as_deref(),
            Some("http://host")
        );
        assert_eq!(
            normalize_origin("https://h:443").as_deref(),
            Some("https://h")
        );
        assert_eq!(
            normalize_origin("http://[::1]:3140").as_deref(),
            Some("http://[::1]:3140")
        );
        for bad in [
            "http://[::1]garbage",
            "http://h:3140garbage",
            "http://h?x",
            "http://h#x",
            "http:\\\\h",
            "null",
            "http://h/path",
            "ftp://h",
            "http://h:99999",
            "http://u@h",
            "http://",
        ] {
            assert!(normalize_origin(bad).is_none(), "{bad}");
        }
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

    async fn contacts(state: &AppState) -> Vec<crate::models::Contact> {
        state
            .db
            .with_conn(crate::db::contacts::list_contacts)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn an_unknown_well_formed_code_is_charged_before_it_is_recorded() {
        let state = state();
        state.p2p.set(true);
        let ip = IpAddr::V4(std::net::Ipv4Addr::new(10, 66, 0, 1));
        rate_limit::reset(ip);
        for n in 0..10 {
            let code = format!("kronn:Stranger{n}@10.0.0.{n}:3456");
            assert!(!admit_peer_presence(&state, &code, ip).await);
        }
        assert!(rate_limit::is_banned(ip), "unknown codes must count");
        let recorded = contacts(&state).await;
        // The tenth attempt crossed the threshold: charged, never inserted.
        assert_eq!(recorded.len(), 9);
        assert!(recorded
            .iter()
            .all(|c| c.status == crate::db::contacts::STATUS_REQUESTED
                && !crate::db::contacts::dials_outbound(&c.status)));
        rate_limit::reset(ip);
    }

    #[tokio::test]
    async fn requests_beyond_the_quota_are_not_recorded() {
        let state = state();
        state.p2p.set(true);
        for n in 0..(crate::db::contacts::MAX_PENDING_REQUESTS + 5) {
            // A fresh IP each time: the quota holds across IPs.
            let ip = IpAddr::V4(std::net::Ipv4Addr::new(
                10,
                67,
                (n / 200) as u8,
                (n % 200) as u8 + 1,
            ));
            rate_limit::reset(ip);
            let code = format!("kronn:Flood{n}@10.1.0.{}:3456", n % 250 + 1);
            assert!(!admit_peer_presence(&state, &code, ip).await);
            rate_limit::reset(ip);
        }
        assert_eq!(
            contacts(&state).await.len() as i64,
            crate::db::contacts::MAX_PENDING_REQUESTS
        );
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
        assert!(admit_peer_presence(&state, "kronn:Ok@10.0.0.50:3456", ip).await);
        for code in [
            "kronn:Wait@10.0.0.51:3456",
            "kronn:Req@10.0.0.52:3456",
            "kronn:No@10.0.0.53:3456",
        ] {
            assert!(!admit_peer_presence(&state, code, ip).await, "{code}");
        }
        rate_limit::reset(ip);
    }

    #[tokio::test]
    async fn p2p_off_admits_no_peer_and_an_oversized_code_is_refused() {
        let state = state();
        let ip = IpAddr::V4(std::net::Ipv4Addr::new(10, 66, 0, 3));
        rate_limit::reset(ip);
        insert_contact(&state, "kronn:Ok@10.0.0.50:3456", "accepted").await;
        assert_eq!(
            admit_presence(&state, "kronn:Ok@10.0.0.50:3456", ip, false).await,
            None,
            "P2P is off by default"
        );
        state.p2p.set(true);
        assert_eq!(
            admit_presence(&state, "kronn:Ok@10.0.0.50:3456", ip, false).await,
            Some(Admission::Peer("kronn:Ok@10.0.0.50:3456".into()))
        );
        let long = format!("kronn:{}@10.0.0.50:3456", "x".repeat(300));
        assert_eq!(admit_presence(&state, &long, ip, false).await, None);
        assert!(contacts(&state).await.len() == 1, "nothing recorded");
        assert_eq!(
            admit_presence(&state, "", ip, true).await,
            Some(Admission::Frontend)
        );
        assert_eq!(admit_presence(&state, "", ip, false).await, None);
        rate_limit::reset(ip);
    }

    /// Room A (shared with contact A) and room B, each with one message.
    async fn two_rooms(state: &AppState) -> &'static str {
        let code = "kronn:A@10.0.0.50:3456";
        state.p2p.set(true);
        state
            .db
            .with_conn(move |conn| {
                crate::db::contacts::insert_contact(
                    conn,
                    &crate::models::Contact {
                        id: "c-a".into(),
                        pseudo: "A".into(),
                        avatar_email: None,
                        kronn_url: "http://10.0.0.50:3456".into(),
                        invite_code: code.into(),
                        status: "accepted".into(),
                        created_at: Utc::now(),
                        updated_at: Utc::now(),
                    },
                )?;
                for (shared, msg) in [("shared-a", "a-msg"), ("shared-b", "b-msg")] {
                    crate::db::discussions::ensure_mirror_by_shared_id(conn, shared, "T", "H")?;
                    handle_incoming_chat_message(
                        conn,
                        shared,
                        msg,
                        "H",
                        None,
                        "hi",
                        0,
                        crate::models::MessageRole::User,
                        crate::models::MessageChannel::Main,
                        None,
                        vec![],
                        None,
                    )?;
                }
                let room_a =
                    crate::db::discussions::find_discussion_by_shared_id(conn, "shared-a")?
                        .unwrap();
                crate::db::discussions::update_discussion_sharing(
                    conn,
                    &room_a,
                    "shared-a",
                    &["c-a".to_string()],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        code
    }

    #[tokio::test]
    async fn a_member_of_one_room_cannot_attach_to_another_rooms_message() {
        let state = state();
        let code = two_rooms(&state).await;
        let mut bus = state.ws_broadcast.subscribe();
        let frame = WsMessage::FileAttached {
            shared_discussion_id: "shared-a".into(),
            message_id: "b-msg".into(),
            file_id: "f-evil".into(),
            filename: "x.pdf".into(),
            mime_type: "application/pdf".into(),
            size: 1,
            from_invite_code: code.into(),
            pending: false,
        };
        assert_eq!(
            ingest_relayable_frame(&state, &frame, &PeerAuth::InviteCode(code.into())).await,
            Ingest::Skip
        );
        assert!(bus.try_recv().is_err(), "no placeholder reaches room B");
        let listed = state
            .db
            .with_conn(|conn| {
                let room_b =
                    crate::db::discussions::find_discussion_by_shared_id(conn, "shared-b")?
                        .unwrap();
                crate::db::discussions::list_context_files_for_message_in(conn, &room_b, "b-msg")
                    .map_err(anyhow::Error::from)
            })
            .await
            .unwrap();
        assert!(listed.is_empty(), "room B's agents never see it");
    }

    #[tokio::test]
    async fn a_download_publishes_nothing_once_the_sender_is_revoked() {
        let state = state();
        let code = two_rooms(&state).await;
        let base = tempfile::TempDir::new().unwrap();
        let room_a = state
            .db
            .with_conn(|conn| {
                crate::db::discussions::find_discussion_by_shared_id(conn, "shared-a")
            })
            .await
            .unwrap()
            .unwrap();
        let attachment =
            |file_id: &str, message_id: &str| crate::api::federation::FederatedAttachment {
                host: PeerAuth::InviteCode(code.into()),
                shared_id: "shared-a".into(),
                discussion_id: room_a.clone(),
                file_id: file_id.into(),
                message_id: message_id.into(),
                filename: "doc.pdf".into(),
                mime_type: "application/pdf".into(),
                size: 3,
            };
        let stage = |file_id: &str| {
            crate::core::context_files::stage_federated_file_in(
                base.path(),
                &room_a,
                file_id,
                "doc.pdf",
                b"abc",
            )
            .unwrap()
        };
        let room_dir = base.path().join("federated").join(&room_a);

        // Another room's message: refused, nothing published.
        let staged = stage("f-1");
        let target = staged.target().to_path_buf();
        assert!(crate::api::federation::commit_federated_attachment(
            &state,
            staged,
            attachment("f-1", "b-msg")
        )
        .await
        .is_err());
        assert!(!target.exists());

        // Authorized: published and linked.
        let staged = stage("f-2");
        let target = staged.target().to_path_buf();
        crate::api::federation::commit_federated_attachment(
            &state,
            staged,
            attachment("f-2", "a-msg"),
        )
        .await
        .unwrap();
        assert!(target.exists());

        // The sender is revoked while the bytes were in flight.
        let staged = stage("f-3");
        let target = staged.target().to_path_buf();
        state
            .db
            .with_conn(|conn| crate::db::contacts::delete_contact(conn, "c-a"))
            .await
            .unwrap();
        assert!(crate::api::federation::commit_federated_attachment(
            &state,
            staged,
            attachment("f-3", "a-msg")
        )
        .await
        .is_err());
        assert!(!target.exists(), "nothing published after revocation");
        let leftovers: Vec<_> = std::fs::read_dir(&room_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".staging-"))
            .collect();
        assert!(leftovers.is_empty(), "rejected staging files are removed");
    }

    /// Keep the single DB connection busy for `ms` on a blocking thread.
    async fn hold_db(state: &AppState, ms: u64) -> tokio::task::JoinHandle<()> {
        let db = state.db.clone();
        let busy = tokio::spawn(async move {
            let _ = db
                .with_conn(move |_| {
                    std::thread::sleep(Duration::from_millis(ms));
                    Ok(())
                })
                .await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        busy
    }

    #[tokio::test]
    async fn a_publication_queued_behind_a_busy_db_respects_p2p_turned_off() {
        let state = state();
        let code = two_rooms(&state).await;
        let base = tempfile::TempDir::new().unwrap();
        let room_a = state
            .db
            .with_conn(|conn| {
                crate::db::discussions::find_discussion_by_shared_id(conn, "shared-a")
            })
            .await
            .unwrap()
            .unwrap();
        let staged = crate::core::context_files::stage_federated_file_in(
            base.path(),
            &room_a,
            "f-busy",
            "doc.pdf",
            b"abc",
        )
        .unwrap();
        let target = staged.target().to_path_buf();
        let attachment = crate::api::federation::FederatedAttachment {
            host: PeerAuth::InviteCode(code.into()),
            shared_id: "shared-a".into(),
            discussion_id: room_a,
            file_id: "f-busy".into(),
            message_id: "a-msg".into(),
            filename: "doc.pdf".into(),
            mime_type: "application/pdf".into(),
            size: 3,
        };
        let busy = hold_db(&state, 600).await;
        let commit = {
            let state = state.clone();
            tokio::spawn(async move {
                crate::api::federation::commit_federated_attachment(&state, staged, attachment)
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        state.p2p.set(false);
        busy.await.unwrap();
        assert!(
            commit.await.unwrap().is_err(),
            "the queued publication is refused"
        );
        assert!(!target.exists(), "nothing published once P2P is off");
    }

    #[tokio::test]
    async fn an_unrelated_config_save_does_not_cut_an_authorized_peer() {
        let state = state();
        let code = two_rooms(&state).await;
        let peer = PeerAuth::InviteCode(code.into());
        // Another setting being saved holds the config lock meanwhile.
        let saving = state.config.write().await;
        let wait = Duration::from_secs(2);
        assert_eq!(
            tokio::time::timeout(wait, peer.check(&state)).await,
            Ok(true)
        );
        assert_eq!(
            tokio::time::timeout(wait, peer.check_member(&state, "shared-a")).await,
            Ok(Some(true))
        );
        drop(saving);
    }

    #[tokio::test]
    async fn turning_p2p_off_waits_for_a_publication_in_progress_and_none_starts_after() {
        let state = state();
        let code = two_rooms(&state).await;
        // A publication in progress holds the gate's read side.
        let gate = state.p2p.clone();
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let publication = std::thread::spawn(move || {
            let on = gate.read();
            assert!(*on);
            held_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            drop(on);
            std::time::Instant::now()
        });
        held_rx.recv().unwrap();
        let gate = state.p2p.clone();
        let turning_off = std::thread::spawn(move || {
            gate.set(false);
            std::time::Instant::now()
        });
        let finished = publication.join().unwrap();
        let off = turning_off.join().unwrap();
        assert!(
            off >= finished,
            "off took effect only once the publication finished"
        );

        let base = tempfile::TempDir::new().unwrap();
        let room_a = state
            .db
            .with_conn(|conn| {
                crate::db::discussions::find_discussion_by_shared_id(conn, "shared-a")
            })
            .await
            .unwrap()
            .unwrap();
        let staged = crate::core::context_files::stage_federated_file_in(
            base.path(),
            &room_a,
            "f-after",
            "doc.pdf",
            b"abc",
        )
        .unwrap();
        let target = staged.target().to_path_buf();
        let attachment = crate::api::federation::FederatedAttachment {
            host: PeerAuth::InviteCode(code.into()),
            shared_id: "shared-a".into(),
            discussion_id: room_a,
            file_id: "f-after".into(),
            message_id: "a-msg".into(),
            filename: "doc.pdf".into(),
            mime_type: "application/pdf".into(),
            size: 3,
        };
        assert!(
            crate::api::federation::commit_federated_attachment(&state, staged, attachment)
                .await
                .is_err()
        );
        assert!(!target.exists(), "no publication starts after P2P is off");
    }

    #[tokio::test]
    async fn a_chat_write_queued_behind_a_busy_db_is_refused_once_p2p_is_off() {
        let state = state();
        let code = two_rooms(&state).await;
        let frame = WsMessage::ChatMessage {
            shared_discussion_id: "shared-a".into(),
            message_id: "late-msg".into(),
            from_pseudo: "A".into(),
            from_avatar_email: None,
            from_invite_code: code.into(),
            content: "late".into(),
            timestamp: 1,
            role: crate::models::MessageRole::User,
            channel: crate::models::MessageChannel::Main,
            agent_type: None,
            target_agents: vec![],
            targets: vec![],
            reply_to_message_id: None,
        };
        let busy = hold_db(&state, 600).await;
        let ingest = {
            let state = state.clone();
            tokio::spawn(async move {
                ingest_relayable_frame(&state, &frame, &PeerAuth::InviteCode(code.into())).await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        state.p2p.set(false);
        busy.await.unwrap();
        assert_eq!(ingest.await.unwrap(), Ingest::Revoked);
        let stored = state
            .db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM messages WHERE id = 'late-msg'",
                    [],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(stored, 0, "nothing written once P2P is off");
    }

    #[test]
    fn unadmitted_connections_are_capped_per_ip() {
        let ip = IpAddr::V4(std::net::Ipv4Addr::new(10, 68, 0, 1));
        let slots: Vec<_> = (0..pending::MAX_PER_IP)
            .map(|_| pending::acquire(ip).expect("under the cap"))
            .collect();
        assert!(pending::acquire(ip).is_none(), "beyond the cap");
        drop(slots);
        assert!(
            pending::acquire(ip).is_some(),
            "released slots are reusable"
        );
    }
}
