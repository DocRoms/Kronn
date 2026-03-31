use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use chrono::Utc;
use futures::{SinkExt, StreamExt};

use crate::{models::WsMessage, AppState};

/// GET /api/ws — WebSocket upgrade handler.
///
/// Accepts connections from:
/// - The local frontend (for real-time presence updates)
/// - Remote Kronn instances (peer-to-peer sync)
///
/// All inbound WsMessages are forwarded to the broadcast channel,
/// and all broadcast events are forwarded to the WebSocket client.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let mut broadcast_rx = state.ws_broadcast.subscribe();
    let broadcast_tx = state.ws_broadcast.clone();

    // Task 1: forward broadcast events → WS client
    let mut send_task = tokio::spawn(async move {
        while let Ok(msg) = broadcast_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if ws_sender.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    });

    // Task 2: receive WS messages → broadcast
    let mut recv_task = tokio::spawn(async move {
        let mut verified = false;

        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                Message::Text(text) => {
                    let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) else {
                        continue;
                    };

                    // First message from a remote peer MUST be Presence with a valid invite code.
                    // Local frontend connections send Presence with empty invite code (accepted).
                    // Any other message type as first message → reject.
                    if !verified {
                        if let WsMessage::Presence {
                            ref from_invite_code,
                            ..
                        } = ws_msg
                        {
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
                                        tracing::warn!(
                                            "WS: rejected invalid invite code: {}",
                                            from_invite_code
                                        );
                                        break;
                                    }
                                }
                            }
                            verified = true;
                        } else {
                            // First message is NOT Presence → reject
                            tracing::warn!("WS: first message must be Presence, got {:?}", ws_msg);
                            break;
                        }
                    }

                    // Handle ping/pong at protocol level
                    if let WsMessage::Ping { timestamp } = &ws_msg {
                        let pong = WsMessage::Pong {
                            timestamp: *timestamp,
                        };
                        let _ = broadcast_tx.send(pong);
                        continue;
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
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: crate::models::ModelTier::Default,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        shared_id: Some(shared_discussion_id.to_string()),
        shared_with: vec![],
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
