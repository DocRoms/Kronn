//! WS Client Manager — maintains outbound WebSocket connections to contacts.
//!
//! Spawned as a background task at startup. For each contact in the DB,
//! it opens a persistent WS connection to their `/api/ws` endpoint and
//! relays messages through the broadcast channel. Reconnects with
//! exponential backoff on disconnection.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite;

use crate::models::WsMessage;
use crate::AppState;

/// Background task that manages outbound WS connections to all contacts.
pub async fn run(state: AppState) {
    // contact_id → active connection task
    let mut connections: HashMap<String, JoinHandle<()>> = HashMap::new();

    loop {
        // P2P off: no contact is dialled, and live connections end (aborting
        // a connection task drops both of its socket halves).
        if !state.p2p.enabled() {
            for (_, handle) in connections.drain() {
                handle.abort();
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }
        // Load current contacts
        let contacts = state
            .db
            .with_conn(crate::db::contacts::list_contacts)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|contact| crate::db::contacts::dials_outbound(&contact.status))
            .collect::<Vec<_>>();

        let active_ids: std::collections::HashSet<String> =
            contacts.iter().map(|c| c.id.clone()).collect();

        // Remove tasks for contacts that no longer exist
        connections.retain(|id, handle| {
            if !active_ids.contains(id) {
                handle.abort();
                false
            } else {
                true
            }
        });

        // Spawn connection task for each contact without an active (running) task
        for contact in &contacts {
            let should_spawn = match connections.get(&contact.id) {
                Some(handle) => handle.is_finished(),
                None => true,
            };

            if should_spawn {
                let s = state.clone();
                let c = contact.clone();
                let handle = tokio::spawn(async move {
                    connect_to_peer(s, c).await;
                });
                connections.insert(contact.id.clone(), handle);
            }
        }

        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

/// The contact as stored now, if this instance may still dial it.
async fn dialable_contact(state: &AppState, id: &str) -> Option<crate::models::Contact> {
    if !state.p2p.enabled() {
        return None;
    }
    let id = id.to_owned();
    state
        .db
        .with_conn(move |conn| crate::db::contacts::get_contact(conn, &id))
        .await
        .ok()
        .flatten()
        .filter(|contact| crate::db::contacts::dials_outbound(&contact.status))
}

/// `ws(s)://host:port/api/ws` for a stored contact URL, or None when the URL
/// is not a bare host and port.
pub(crate) fn peer_ws_url(kronn_url: &str) -> Option<String> {
    let base = crate::db::contacts::contact_base_url(kronn_url)?;
    Some(format!(
        "{}/api/ws",
        base.replacen("http://", "ws://", 1)
            .replacen("https://", "wss://", 1)
    ))
}

/// Connect to a single peer and maintain the connection with exponential backoff.
async fn connect_to_peer(state: AppState, contact: crate::models::Contact) {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    loop {
        let Some(contact) = dialable_contact(&state, &contact.id).await else {
            return;
        };
        let Some(ws_url) = peer_ws_url(&contact.kronn_url) else {
            tracing::warn!(
                "WS client: contact {} has an invalid URL, not dialling",
                contact.pseudo
            );
            return;
        };

        tracing::debug!("WS client: connecting to {} ({})", contact.pseudo, ws_url);

        // Bound the handshake: a peer that accepts TCP but never completes the
        // WS upgrade (NAT/relay half-open) would otherwise pin this task inside
        // connect_async forever — and the manager loop only respawns FINISHED
        // tasks, so that contact would stay silently unreachable until restart.
        let connect = crate::api::federation::unless_p2p_off(
            &state,
            tokio::time::timeout(
                std::time::Duration::from_secs(30),
                tokio_tungstenite::connect_async(&ws_url),
            ),
        )
        .await;
        let Some(connect) = connect else {
            return;
        };
        let connect = connect.unwrap_or_else(|_| {
            Err(tokio_tungstenite::tungstenite::Error::Io(
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "WS handshake timed out after 30s",
                ),
            ))
        });
        match connect {
            Ok((ws_stream, _)) => {
                // The upgrade may have been held until P2P was turned off or
                // the contact refused: nothing is sent then.
                if dialable_contact(&state, &contact.id).await.is_none() {
                    return;
                }
                let session_start = Instant::now();
                tracing::info!("WS client: connected to {}", contact.pseudo);

                // Update contact status to accepted (WS connection proves reachability)
                let cid = contact.id.clone();
                let gate = state.p2p.clone();
                let _ = state
                    .db
                    .with_conn(move |conn| {
                        let on = gate.read();
                        if !*on {
                            return Ok(false);
                        }
                        crate::db::contacts::update_contact_status(conn, &cid, "accepted")
                    })
                    .await;

                // Emit presence online for this contact locally
                let _ = state.ws_broadcast.send(WsMessage::Presence {
                    from_pseudo: contact.pseudo.clone(),
                    from_invite_code: contact.invite_code.clone(),
                    online: true,
                });

                // Handle the connection
                handle_peer_connection(ws_stream, &state, &contact.id).await;

                // Connection lost — emit offline
                tracing::info!("WS client: disconnected from {}", contact.pseudo);
                let _ = state.ws_broadcast.send(WsMessage::Presence {
                    from_pseudo: contact.pseudo.clone(),
                    from_invite_code: contact.invite_code.clone(),
                    online: false,
                });

                // Only treat the connection as "good" (reset backoff to 1s) if it
                // actually stayed up. A peer that drops us during the presence
                // handshake (bad/malformed invite code, ban, auth) returns almost
                // instantly — resetting on the mere TCP upgrade would retry every
                // ~1s forever, flooding the frontend with online/offline churn.
                // Gating on session lifetime degrades any persistent rejection
                // gracefully (backoff grows to the 60s cap) instead of flapping.
                if session_start.elapsed() >= HEALTHY_SESSION {
                    backoff = Duration::from_secs(1);
                }
            }
            Err(e) => {
                tracing::debug!("WS client: failed to connect to {}: {}", contact.pseudo, e);
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Minimum time a peer connection must stay open to count as "healthy" and
/// reset the reconnect backoff. Below this, the session is treated as a failed
/// handshake and the backoff keeps growing (anti-flap guard).
const HEALTHY_SESSION: Duration = Duration::from_secs(20);

/// How often each side of a peer WS sends a keepalive Ping.
///
/// Must be shorter than the most aggressive middlebox idle-timeout on the
/// path. A cross-machine P2P socket is otherwise nearly silent (after the
/// echo-storm fix, Presence/heartbeats are no longer relayed), and WSL2's
/// virtual-switch NAT silently drops a fully-idle TCP flow in ~5 s (observed:
/// the socket dies with no Close frame, neither side errors → zombie that
/// never reconnects). Both directions ping independently, so each ~6-byte
/// frame also refreshes the NAT entry for its direction. Cost is negligible.
pub(crate) const WS_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(3);

/// If no frame (Text, Ping, or Pong) arrives from the peer within this window,
/// the connection is treated as dead and torn down so the manager reconnects.
/// Turns a silently half-dead socket ("zombie") into a detected disconnect.
///
/// Deliberately MANY keepalive intervals wide. Liveness/NAT survival is the
/// keepalive's job (`WS_KEEPALIVE_INTERVAL`); this timeout only catches a truly
/// dead socket, so it can be generous. It MUST stay well above any peer's
/// keepalive cadence — a version-skewed peer still pinging at the old 20 s
/// would otherwise be dropped every cycle (reconnect loop → presence flicker
/// and repeated notifications). 45 s tolerates a 20 s-era peer with margin
/// while still surfacing a dead link reasonably fast.
pub(crate) const WS_IDLE_TIMEOUT: Duration = Duration::from_secs(45);

/// Send our Presence and the F4 catch-up requests. False when the peer lost
/// its authorization or the socket failed: the connection then ends.
async fn bootstrap_peer<S>(
    ws_sender: &mut S,
    state: &AppState,
    peer: &crate::api::ws::PeerAuth,
    contact_id: &str,
) -> bool
where
    S: futures::Sink<tungstenite::Message> + Unpin,
{
    // Use the SAME canonical builder as the /api/contacts/invite-code endpoint
    // so the code we send matches the code a peer stored: an empty pseudo
    // yields `kronn:@host:port`, which the peer rejects (reconnect storm, ban).
    let config = state.config.read().await;
    let our_pseudo = crate::api::contacts::invite_pseudo(&config.server);
    let our_invite_code = crate::api::contacts::build_invite_code(&config.server).await;
    drop(config);
    let presence_msg = WsMessage::Presence {
        from_pseudo: our_pseudo,
        from_invite_code: our_invite_code,
        online: true,
    };
    if !send_if_authorized(ws_sender, state, peer, &presence_msg).await {
        return false;
    }

    // F4 catch-up: ask this peer to re-send what we missed in each shared
    // discussion while either side was offline (the answers dedup on
    // message_id). Only the shares this contact belongs to: a sync request
    // names its shared id.
    let member_id = contact_id.to_owned();
    let sync_points = state
        .db
        .with_conn(move |conn| {
            let points = crate::db::discussions::list_shared_sync_points(conn)?;
            Ok(points
                .into_iter()
                .filter(|(shared_id, _)| {
                    crate::db::discussions::shared_member(conn, shared_id, &member_id)
                        .unwrap_or(false)
                })
                .collect::<Vec<_>>())
        })
        .await
        .unwrap_or_default();
    for (shared_discussion_id, since_timestamp) in sync_points {
        let request = WsMessage::DiscSyncRequest {
            shared_discussion_id,
            since_timestamp,
        };
        if !send_if_authorized(ws_sender, state, peer, &request).await {
            return false;
        }
    }
    true
}

async fn send_if_authorized<S>(
    ws_sender: &mut S,
    state: &AppState,
    peer: &crate::api::ws::PeerAuth,
    msg: &WsMessage,
) -> bool
where
    S: futures::Sink<tungstenite::Message> + Unpin,
{
    if !peer.check(state).await {
        return false;
    }
    let Ok(json) = serde_json::to_string(msg) else {
        return true;
    };
    ws_sender
        .send(tungstenite::Message::Text(json.into()))
        .await
        .is_ok()
}

/// Handle messages on an established peer WS connection. Both halves and
/// the authorization re-check are futures of the caller's task, so aborting
/// that task, or revoking the contact, ends the whole connection.
async fn handle_peer_connection(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    state: &AppState,
    contact_id: &str,
) {
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let mut broadcast_rx = state.ws_broadcast.subscribe();
    let peer = crate::api::ws::PeerAuth::ContactId(contact_id.to_owned());
    // The whole bootstrap (Presence, then the sync requests) runs under the
    // shared cancellation, and each send is authorized right before it.
    let bootstrap = bootstrap_peer(&mut ws_sender, state, &peer, contact_id);
    if crate::api::federation::unless_p2p_off(state, bootstrap).await != Some(true) {
        return;
    }

    // Keys of chat/invite frames received FROM this peer, never echoed back
    // (they would bounce forever between the two instances).
    let echo_guard = Mutex::new(PeerEchoGuard::default());

    // Only peer-relayable frames cross the wire; relaying Presence or local UI
    // signals bounced the channel into overflow (~2 s cross-machine flap).
    let send = async {
        // A periodic ping keeps NAT/Tailscale idle-timeouts from silently
        // killing the socket, and surfaces a dead one as a failed send.
        let mut keepalive = tokio::time::interval(WS_KEEPALIVE_INTERVAL);
        keepalive.tick().await;
        loop {
            tokio::select! {
                _ = keepalive.tick() => {
                    if ws_sender
                        .send(tungstenite::Message::Ping(Vec::<u8>::new().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                recv = broadcast_rx.recv() => match recv {
                    Ok(msg) => {
                        if !msg.is_peer_relayable() {
                            continue;
                        }
                        if let Some(key) = msg.relay_dedup_key() {
                            if echo_guard.lock().unwrap_or_else(|e| e.into_inner()).contains(&key) {
                                continue;
                            }
                        }
                        // Only a member of that shared discussion gets it.
                        match peer.may_receive(state, &msg).await {
                            None => break,
                            Some(false) => continue,
                            Some(true) => {}
                        }
                        if let Ok(json) = serde_json::to_string(&msg) {
                            if ws_sender
                                .send(tungstenite::Message::Text(json.into()))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                    // Fell behind the bus: skip the gap, keep the socket.
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        }
    };

    // Relayable frames are persisted through the inbound handler's ingest path
    // (stored once whichever socket carries them) and re-broadcast only if new.
    // Any frame resets the idle timer; silence past WS_IDLE_TIMEOUT means a
    // dead socket, and `connect_to_peer` reconnects.
    let recv = async {
        loop {
            let msg = match tokio::time::timeout(WS_IDLE_TIMEOUT, ws_receiver.next()).await {
                Err(_) | Ok(None) | Ok(Some(Err(_))) => break,
                Ok(Some(Ok(msg))) => msg,
            };
            let text = match msg {
                tungstenite::Message::Text(text) => text,
                tungstenite::Message::Close(_) => break,
                _ => continue,
            };
            let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) else {
                continue;
            };
            // A dialled peer feeds shared discussions only.
            if !ws_msg.is_peer_relayable() {
                continue;
            }
            match crate::api::ws::ingest_relayable_frame(state, &ws_msg, &peer).await {
                crate::api::ws::Ingest::Revoked => break,
                crate::api::ws::Ingest::Skip => {}
                crate::api::ws::Ingest::Broadcast => {
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

    let recheck = async {
        let mut every = tokio::time::interval(crate::api::ws::PEER_RECHECK_INTERVAL);
        every.tick().await;
        loop {
            every.tick().await;
            if !peer.check(state).await {
                break;
            }
        }
    };

    tokio::select! {
        _ = send => {}
        _ = recv => {}
        _ = recheck => {}
        _ = crate::api::federation::until_p2p_off(state) => {}
    }
}

/// Bounded set of relay dedup-keys recently received **from** a peer, so a
/// connection never echoes a peer's own chat/invite frame straight back to it.
/// FIFO eviction keeps it O(1) and memory-bounded under sustained traffic.
#[derive(Default)]
pub(crate) struct PeerEchoGuard {
    set: HashSet<String>,
    order: VecDeque<String>,
}

impl PeerEchoGuard {
    const CAP: usize = 4096;

    pub(crate) fn record(&mut self, key: String) {
        if self.set.insert(key.clone()) {
            self.order.push_back(key);
            if self.order.len() > Self::CAP {
                if let Some(old) = self.order.pop_front() {
                    self.set.remove(&old);
                }
            }
        }
    }

    pub(crate) fn contains(&self, key: &str) -> bool {
        self.set.contains(key)
    }
}

/// Compute exponential backoff duration (exposed for testing).
pub fn compute_backoff(current: Duration, max: Duration) -> Duration {
    (current * 2).min(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles() {
        let d = compute_backoff(Duration::from_secs(1), Duration::from_secs(60));
        assert_eq!(d, Duration::from_secs(2));
    }

    #[test]
    fn backoff_caps_at_max() {
        let d = compute_backoff(Duration::from_secs(32), Duration::from_secs(60));
        assert_eq!(d, Duration::from_secs(60));
    }

    #[test]
    fn ws_url_construction() {
        let kronn_url = "http://100.64.1.5:3456";
        let ws_url = format!(
            "{}/api/ws",
            kronn_url
                .replace("http://", "ws://")
                .replace("https://", "wss://")
        );
        assert_eq!(ws_url, "ws://100.64.1.5:3456/api/ws");
    }

    #[test]
    fn ws_url_construction_https() {
        let kronn_url = "https://peer.example.com:3456";
        let ws_url = format!(
            "{}/api/ws",
            kronn_url
                .replace("http://", "ws://")
                .replace("https://", "wss://")
        );
        assert_eq!(ws_url, "wss://peer.example.com:3456/api/ws");
    }

    fn test_state() -> AppState {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        let mut config = crate::core::config::default_config();
        config.server.p2p_enabled = true;
        AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(config)),
            db,
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        )
    }

    async fn insert(state: &AppState, url: &str) -> crate::models::Contact {
        let contact = crate::models::Contact {
            id: uuid::Uuid::new_v4().to_string(),
            pseudo: "Peer".into(),
            avatar_email: None,
            kronn_url: url.into(),
            invite_code: format!("kronn:Peer@{}", url.trim_start_matches("http://")),
            status: "accepted".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let c = contact.clone();
        state
            .db
            .with_conn(move |conn| crate::db::contacts::insert_contact(conn, &c))
            .await
            .unwrap();
        contact
    }

    /// Accept one WS connection and report when its stream ends.
    async fn peer_server() -> (String, tokio::sync::oneshot::Receiver<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            while let Some(Ok(frame)) = ws.next().await {
                if frame.is_close() {
                    break;
                }
            }
            let _ = closed_tx.send(());
        });
        (url, closed_rx)
    }

    #[tokio::test]
    async fn deleting_a_dialled_contact_ends_the_connection() {
        let state = test_state();
        let (url, closed) = peer_server().await;
        let contact = insert(&state, &url).await;
        let (stream, _) = tokio_tungstenite::connect_async(peer_ws_url(&url).unwrap())
            .await
            .unwrap();
        let id = contact.id.clone();
        let task_state = state.clone();
        let connection =
            tokio::spawn(async move { handle_peer_connection(stream, &task_state, &id).await });
        let id = contact.id.clone();
        state
            .db
            .with_conn(move |conn| crate::db::contacts::delete_contact(conn, &id))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(12), connection)
            .await
            .expect("the connection ends after revocation")
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), closed)
            .await
            .expect("the peer sees the socket close")
            .unwrap();
    }

    #[tokio::test]
    async fn a_bootstrap_queued_behind_a_busy_db_sends_nothing_once_p2p_is_off() {
        let state = test_state();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let received = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let recorder = received.clone();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            while let Some(Ok(frame)) = ws.next().await {
                if let tungstenite::Message::Text(text) = frame {
                    recorder.lock().unwrap().push(text.to_string());
                }
            }
        });
        let contact = insert(&state, &url).await;
        let (stream, _) = tokio_tungstenite::connect_async(peer_ws_url(&url).unwrap())
            .await
            .unwrap();
        let db = state.db.clone();
        let busy = tokio::spawn(async move {
            let _ = db
                .with_conn(|_| {
                    std::thread::sleep(Duration::from_millis(600));
                    Ok(())
                })
                .await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let connection = {
            let state = state.clone();
            let id = contact.id.clone();
            tokio::spawn(async move { handle_peer_connection(stream, &state, &id).await })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        state.p2p.set(false);
        busy.await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), connection)
            .await
            .expect("the connection ends")
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            received.lock().unwrap().is_empty(),
            "no Presence nor sync request once P2P is off: {:?}",
            received.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn aborting_a_peer_task_closes_its_socket() {
        let state = test_state();
        let (url, closed) = peer_server().await;
        let contact = insert(&state, &url).await;
        let task = tokio::spawn(connect_to_peer(state.clone(), contact));
        tokio::time::sleep(Duration::from_millis(500)).await;
        task.abort();
        tokio::time::timeout(Duration::from_secs(2), closed)
            .await
            .expect("no half of the socket outlives the aborted task")
            .unwrap();
    }

    #[tokio::test]
    async fn p2p_off_or_an_invalid_url_dials_nothing() {
        let state = test_state();
        let contact = insert(&state, "http://127.0.0.1:9/admin#").await;
        assert!(dialable_contact(&state, &contact.id).await.is_some());
        assert!(peer_ws_url(&contact.kronn_url).is_none());
        state.p2p.set(false);
        assert!(dialable_contact(&state, &contact.id).await.is_none());
    }
}
