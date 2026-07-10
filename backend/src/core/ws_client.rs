//! WS Client Manager — maintains outbound WebSocket connections to contacts.
//!
//! Spawned as a background task at startup. For each contact in the DB,
//! it opens a persistent WS connection to their `/api/ws` endpoint and
//! relays messages through the broadcast channel. Reconnects with
//! exponential backoff on disconnection.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
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
        // Load current contacts
        let contacts = state
            .db
            .with_conn(crate::db::contacts::list_contacts)
            .await
            .unwrap_or_default();

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

/// Connect to a single peer and maintain the connection with exponential backoff.
async fn connect_to_peer(state: AppState, contact: crate::models::Contact) {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    loop {
        let ws_url = format!(
            "{}/api/ws",
            contact.kronn_url.replace("http://", "ws://").replace("https://", "wss://")
        );

        tracing::debug!("WS client: connecting to {} ({})", contact.pseudo, ws_url);

        // Bound the handshake: a peer that accepts TCP but never completes the
        // WS upgrade (NAT/relay half-open) would otherwise pin this task inside
        // connect_async forever — and the manager loop only respawns FINISHED
        // tasks, so that contact would stay silently unreachable until restart.
        let connect = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            tokio_tungstenite::connect_async(&ws_url),
        )
        .await
        .unwrap_or_else(|_| {
            Err(tokio_tungstenite::tungstenite::Error::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "WS handshake timed out after 30s",
            )))
        });
        match connect {
            Ok((ws_stream, _)) => {
                let session_start = Instant::now();
                tracing::info!("WS client: connected to {}", contact.pseudo);

                // Update contact status to accepted (WS connection proves reachability)
                let cid = contact.id.clone();
                let _ = state
                    .db
                    .with_conn(move |conn| {
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
                handle_peer_connection(ws_stream, &state).await;

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

/// Handle messages on an established peer WS connection.
async fn handle_peer_connection(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    state: &AppState,
) {
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let broadcast_tx = state.ws_broadcast.clone();
    let mut broadcast_rx = state.ws_broadcast.subscribe();

    // Send our own presence to the peer. Use the SAME canonical builder as the
    // /api/contacts/invite-code endpoint so the code we send matches the code a
    // peer stored — an empty pseudo here yields `kronn:@host:port`, which the
    // peer rejects, triggering a reconnect/online-offline storm and an IP ban.
    let config = state.config.read().await;
    let our_pseudo = crate::api::contacts::invite_pseudo(&config.server);
    let our_invite_code = crate::api::contacts::build_invite_code(&config.server).await;
    drop(config);

    let presence_msg = WsMessage::Presence {
        from_pseudo: our_pseudo,
        from_invite_code: our_invite_code,
        online: true,
    };
    if let Ok(json) = serde_json::to_string(&presence_msg) {
        let _ = ws_sender.send(tungstenite::Message::Text(json.into())).await;
    }

    // F4 catch-up — on every (re)connect, ask this peer to re-send anything we
    // missed in each shared disc while either side was offline. Sent directly
    // on this socket (targeted, not broadcast) right after Presence. The peer
    // answers with ChatMessages we dedup on message_id, so it's a cheap no-op
    // when already in sync. Without this, messages authored while a peer was
    // disconnected were lost forever (fire-and-forget broadcast, no outbox).
    let sync_points = state
        .db
        .with_conn(crate::db::discussions::list_shared_sync_points)
        .await
        .unwrap_or_default();
    for (shared_discussion_id, since_timestamp) in sync_points {
        let req = WsMessage::DiscSyncRequest {
            shared_discussion_id,
            since_timestamp,
        };
        if let Ok(json) = serde_json::to_string(&req) {
            let _ = ws_sender.send(tungstenite::Message::Text(json.into())).await;
        }
    }

    // Loop guard shared by both halves: keys of chat/invite frames we received
    // FROM this peer, so we never echo them straight back (which would bounce
    // forever between the two instances).
    let echo_guard: Arc<Mutex<PeerEchoGuard>> = Arc::new(Mutex::new(PeerEchoGuard::default()));
    let recv_guard = echo_guard.clone();

    // Task 1: forward broadcast events → peer. Only peer-relayable frames cross
    // the wire (chat + invites); Presence/heartbeats/local UI signals are
    // filtered out — relaying Presence is what bounced the channel into overflow
    // and dropped the socket (~2 s cross-machine flap).
    let mut send_task = tokio::spawn(async move {
        // Keepalive: ping every 20 s. An idle cross-machine WS would otherwise be
        // silently dropped by a NAT / Tailscale idle-timeout — the TCP socket
        // dies but neither task errors, so the connection is never detected as
        // dead, never reconnects, and the peer shows "online" forever while no
        // message flows (observed: `connected` logged, then `ss` shows NO socket
        // and ZERO `disconnected` for minutes). The periodic ping both keeps the
        // socket alive AND surfaces a dead one: a failed send breaks the loop →
        // `connect_to_peer` reconnects with backoff.
        let mut keepalive = tokio::time::interval(WS_KEEPALIVE_INTERVAL);
        keepalive.tick().await; // consume the immediate first tick
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
                        // Don't echo a frame back to the peer it came from.
                        if let Some(key) = msg.relay_dedup_key() {
                            if echo_guard.lock().unwrap_or_else(|e| e.into_inner()).contains(&key) {
                                continue;
                            }
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
                    // A burst made us fall behind the 256-slot bus. Skip the gap
                    // and keep the socket instead of tearing it down (the old
                    // `while let Ok` treated Lagged as terminal → reconnect storm).
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        }
    });

    // Task 2: receive peer messages. Relayable frames are persisted via the same
    // ingest path as the inbound `/api/ws` handler (so chat is stored exactly
    // once whichever socket carries it) and re-broadcast ONLY if new — gating on
    // novelty breaks the cross-connection relay loop that a per-connection echo
    // guard can't (a frame bouncing A→B→A over the two directional sockets).
    // Any received frame (incl. the peer's keepalive Ping) resets the idle
    // timer; nothing within WS_IDLE_TIMEOUT → socket presumed dead → break →
    // `connect_to_peer` reconnects (no more silent zombies).
    let recv_state = state.clone();
    let mut recv_task = tokio::spawn(async move {
        loop {
            match tokio::time::timeout(WS_IDLE_TIMEOUT, ws_receiver.next()).await {
                Err(_idle) => break,            // no traffic in the window → dead
                Ok(None) => break,              // stream ended
                Ok(Some(Err(_))) => break,      // transport error
                Ok(Some(Ok(msg))) => match msg {
                    tungstenite::Message::Text(text) => {
                        if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                            let should_broadcast = if ws_msg.is_peer_relayable() {
                                crate::api::ws::ingest_relayable_frame(&recv_state, &ws_msg).await
                            } else {
                                true
                            };
                            if should_broadcast {
                                if let Some(key) = ws_msg.relay_dedup_key() {
                                    recv_guard.lock().unwrap_or_else(|e| e.into_inner()).record(key);
                                }
                                let _ = broadcast_tx.send(ws_msg);
                            }
                        }
                    }
                    tungstenite::Message::Close(_) => break,
                    // Ping/Pong/Binary: counted as liveness above; nothing to do.
                    _ => {}
                },
            }
        }
    });

    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
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
            kronn_url.replace("http://", "ws://").replace("https://", "wss://")
        );
        assert_eq!(ws_url, "ws://100.64.1.5:3456/api/ws");
    }

    #[test]
    fn ws_url_construction_https() {
        let kronn_url = "https://peer.example.com:3456";
        let ws_url = format!(
            "{}/api/ws",
            kronn_url.replace("http://", "ws://").replace("https://", "wss://")
        );
        assert_eq!(ws_url, "wss://peer.example.com:3456/api/ws");
    }
}
