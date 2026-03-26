//! WS Client Manager — maintains outbound WebSocket connections to contacts.
//!
//! Spawned as a background task at startup. For each contact in the DB,
//! it opens a persistent WS connection to their `/api/ws` endpoint and
//! relays messages through the broadcast channel. Reconnects with
//! exponential backoff on disconnection.

use std::collections::HashMap;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
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

        match tokio_tungstenite::connect_async(&ws_url).await {
            Ok((ws_stream, _)) => {
                backoff = Duration::from_secs(1); // reset on successful connect
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
                handle_peer_connection(ws_stream, &state, &contact).await;

                // Connection lost — emit offline
                tracing::info!("WS client: disconnected from {}", contact.pseudo);
                let _ = state.ws_broadcast.send(WsMessage::Presence {
                    from_pseudo: contact.pseudo.clone(),
                    from_invite_code: contact.invite_code.clone(),
                    online: false,
                });
            }
            Err(e) => {
                tracing::debug!("WS client: failed to connect to {}: {}", contact.pseudo, e);
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Handle messages on an established peer WS connection.
async fn handle_peer_connection(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    state: &AppState,
    contact: &crate::models::Contact,
) {
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let broadcast_tx = state.ws_broadcast.clone();
    let mut broadcast_rx = state.ws_broadcast.subscribe();

    // Send our own presence to the peer
    let config = state.config.read().await;
    let our_pseudo = config.server.pseudo.clone().unwrap_or_default();
    let our_host = crate::api::contacts::advertised_host_async(&config.server).await;
    let our_port = config.server.port;
    drop(config);

    let our_invite_code = format!("kronn:{}@{}:{}", our_pseudo, our_host, our_port);
    let presence_msg = WsMessage::Presence {
        from_pseudo: our_pseudo,
        from_invite_code: our_invite_code,
        online: true,
    };
    if let Ok(json) = serde_json::to_string(&presence_msg) {
        let _ = ws_sender.send(tungstenite::Message::Text(json.into())).await;
    }

    let contact_invite = contact.invite_code.clone();

    // Task 1: forward broadcast events → peer (only our own events, not echoes from this peer)
    let mut send_task = tokio::spawn(async move {
        while let Ok(msg) = broadcast_rx.recv().await {
            // Don't echo messages back to the peer they came from
            if let WsMessage::Presence {
                ref from_invite_code,
                ..
            } = msg
            {
                if from_invite_code == &contact_invite {
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
    });

    // Task 2: receive peer messages → broadcast
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                tungstenite::Message::Text(text) => {
                    if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                        let _ = broadcast_tx.send(ws_msg);
                    }
                }
                tungstenite::Message::Close(_) => break,
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
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
