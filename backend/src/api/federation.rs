//! Cross-instance message federation — the single point where a freshly
//! inserted message is broadcast to peer instances of a SHARED discussion.
//!
//! Before this existed, the broadcast was hand-rolled at three call sites (the
//! UI `send_message`, the MCP `disc_append`) and entirely MISSING at a fourth
//! (the native agent runner in `streaming.rs`), so a reply produced by the
//! local runner never reached the peer. Routing every cross-wire insert through
//! here guarantees they federate identically and carry the author's role +
//! agent identity.

use base64::Engine as _;

use crate::models::{DiscussionMessage, WsMessage};
use crate::AppState;

/// Broadcast `msg` to peers iff `disc_id` is a shared discussion. No-op for a
/// purely local disc (no `shared_id`).
///
/// Gating is on `shared_id` alone — a mirror disc on the joining side
/// legitimately has an empty `shared_with` yet must still federate its replies
/// back to the host, so we must NOT also require `shared_with` to be non-empty.
///
/// The frame carries `role` + `agent_type` so the receiver can preserve author
/// fidelity (an Agent reply lands as an Agent message with its CLI name rather
/// than a generic "User").
pub async fn federate_message(state: &AppState, disc_id: &str, msg: &DiscussionMessage) {
    let did = disc_id.to_string();
    let shared_id = state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::get_discussion(conn, &did).map(|d| d.and_then(|d| d.shared_id))
        })
        .await
        .ok()
        .flatten();
    let Some(shared_id) = shared_id else {
        return; // not a shared disc — nothing to federate
    };

    let config = state.config.read().await;
    let from_pseudo = crate::api::contacts::invite_pseudo(&config.server);
    let from_avatar_email = config.server.avatar_email.clone();
    let from_invite_code = crate::api::contacts::build_invite_code(&config.server).await;
    drop(config);

    let _ = state.ws_broadcast.send(WsMessage::ChatMessage {
        shared_discussion_id: shared_id.clone(),
        message_id: msg.id.clone(),
        from_pseudo,
        from_avatar_email,
        from_invite_code: from_invite_code.clone(),
        content: msg.content.clone(),
        timestamp: msg.timestamp.timestamp_millis(),
        role: msg.role.clone(),
        agent_type: msg.agent_type.clone(),
    });

    // F8 — also announce any files pinned to this message so the peer can fetch
    // the binaries (federation is otherwise text-only).
    emit_attachments(state, &shared_id, &msg.id, &from_invite_code).await;
}

/// Broadcast a `FileAttached` for every `context_file` pinned to `message_id`,
/// so peers of the shared disc can fetch the binary. No-op when the message has
/// no attachments. Shared by the live-federation and the catch-up re-send paths.
async fn emit_attachments(state: &AppState, shared_id: &str, message_id: &str, our_invite_code: &str) {
    let mid = message_id.to_string();
    let files = state
        .db
        .with_conn(move |conn| crate::db::discussions::list_context_files_for_message(conn, &mid).map_err(|e| anyhow::anyhow!(e)))
        .await
        .unwrap_or_default();
    for f in files {
        let _ = state.ws_broadcast.send(WsMessage::FileAttached {
            shared_discussion_id: shared_id.to_string(),
            message_id: message_id.to_string(),
            file_id: f.id.clone(),
            filename: f.filename.clone(),
            mime_type: f.mime_type.clone(),
            size: f.original_size as i64,
            from_invite_code: our_invite_code.to_string(),
            pending: false, // cross-wire announcement; receiver derives pending locally
        });
    }
}

/// Answer a peer's `DiscSyncRequest` (F4 catch-up): re-broadcast every message
/// in the shared disc newer than `since_timestamp` as a `ChatMessage`. The
/// receiver dedups on message_id, so re-sending to an already-synced peer costs
/// nothing; a peer that was OFFLINE gets exactly the messages it missed.
///
/// No-op when we don't host a local copy of that shared_id (a peer may probe us
/// for a disc we're not in). The original author identity is preserved for
/// messages we relayed from another peer; messages we authored locally carry
/// our own identity.
pub async fn respond_to_sync_request(state: &AppState, shared_id: &str, since_timestamp: i64) {
    let sid = shared_id.to_string();
    let disc_id = state
        .db
        .with_conn(move |conn| crate::db::discussions::find_discussion_by_shared_id(conn, &sid))
        .await
        .ok()
        .flatten();
    let Some(disc_id) = disc_id else {
        return;
    };

    let did = disc_id.clone();
    let messages = state
        .db
        .with_conn(move |conn| crate::db::discussions::list_messages(conn, &did))
        .await
        .unwrap_or_default();

    let config = state.config.read().await;
    let our_pseudo = crate::api::contacts::invite_pseudo(&config.server);
    let our_avatar = config.server.avatar_email.clone();
    let our_invite_code = crate::api::contacts::build_invite_code(&config.server).await;
    drop(config);

    let mut sent = 0u32;
    for m in messages
        .into_iter()
        .filter(|m| m.timestamp.timestamp_millis() > since_timestamp)
    {
        let from_pseudo = m.author_pseudo.clone().unwrap_or_else(|| our_pseudo.clone());
        let from_avatar_email = m.author_avatar_email.clone().or_else(|| our_avatar.clone());
        let _ = state.ws_broadcast.send(WsMessage::ChatMessage {
            shared_discussion_id: shared_id.to_string(),
            message_id: m.id.clone(),
            from_pseudo,
            from_avatar_email,
            from_invite_code: our_invite_code.clone(),
            content: m.content.clone(),
            timestamp: m.timestamp.timestamp_millis(),
            role: m.role.clone(),
            agent_type: m.agent_type.clone(),
        });
        // Re-announce attachments too, so a catching-up peer also recovers files.
        emit_attachments(state, shared_id, &m.id, &our_invite_code).await;
        sent += 1;
    }
    if sent > 0 {
        tracing::info!(
            "WS: answered DiscSyncRequest for shared {shared_id} — re-sent {sent} message(s)"
        );
    }
}

/// Receiver side of F8: a `FileAttached` arrived for a shared disc we host a
/// mirror of. Fetch the binary from the announcing host over HTTP, store it on
/// disk, and link it to the (cross-instance-stable) `message_id`. Idempotent on
/// `file_id` — a re-received announcement, or a file we already have, is a
/// no-op. Best-effort: any failure is logged, never fatal (the text message is
/// already delivered; the attachment can be re-announced on the next sync).
#[allow(clippy::too_many_arguments)]
pub async fn fetch_and_store_attachment(
    state: &AppState,
    shared_id: &str,
    message_id: &str,
    file_id: &str,
    filename: &str,
    mime_type: &str,
    size: i64,
    host_invite_code: &str,
) {
    // Idempotency — we already have this file.
    let fid = file_id.to_string();
    if state
        .db
        .with_conn(move |conn| crate::db::discussions::context_file_exists(conn, &fid).map_err(|e| anyhow::anyhow!(e)))
        .await
        .unwrap_or(false)
    {
        return;
    }

    // Resolve the announcing host's URL from its invite code.
    let code = host_invite_code.to_string();
    let host = state
        .db
        .with_conn(move |conn| crate::db::contacts::find_contact_by_invite_code(conn, &code))
        .await
        .ok()
        .flatten();
    let Some(host) = host else {
        tracing::warn!("F8: FileAttached from unknown host, cannot fetch {file_id}");
        return;
    };

    // Our own code authenticates the fetch (same trust model as claim-by-token).
    let our_code = {
        let cfg = state.config.read().await;
        crate::api::contacts::build_invite_code(&cfg.server).await
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let url = format!("{}/api/disc/fetch-file", host.kronn_url.trim_end_matches('/'));
    let body = serde_json::json!({ "file_id": file_id, "from_invite_code": our_code });
    let resp = match client.post(&url).json(&body).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("F8: fetch-file request to {} failed: {e}", host.pseudo);
            return;
        }
    };
    let parsed: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return,
    };
    let data_b64 = parsed
        .get("data")
        .and_then(|d| d.get("data_base64"))
        .and_then(|s| s.as_str());
    let Some(data_b64) = data_b64 else {
        tracing::warn!("F8: host did not return bytes for {file_id}");
        return;
    };
    let bytes = match base64::engine::general_purpose::STANDARD.decode(data_b64) {
        Ok(b) => b,
        Err(_) => return,
    };

    // Mirror discs have no project work_dir → persistent config-dir store.
    let disk_path = match crate::core::context_files::save_file_to_disk(file_id, filename, &bytes) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("F8: failed to save fetched attachment {file_id}: {e}");
            return;
        }
    };

    let sid = shared_id.to_string();
    let disc_id = state
        .db
        .with_conn(move |conn| crate::db::discussions::find_discussion_by_shared_id(conn, &sid))
        .await
        .ok()
        .flatten();
    let Some(disc_id) = disc_id else { return };

    let (fid, did, mid, fname, mime) = (
        file_id.to_string(),
        disc_id,
        message_id.to_string(),
        filename.to_string(),
        mime_type.to_string(),
    );
    let sz = size.max(0) as u64;
    if let Err(e) = state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::insert_federated_context_file(
                conn, &fid, &did, &mid, &fname, &mime, sz, &disk_path,
            )
            .map_err(|e| anyhow::anyhow!(e))
        })
        .await
    {
        tracing::warn!("F8: failed to link fetched attachment {file_id}: {e}");
        return;
    }
    tracing::info!("F8: fetched + stored federated attachment {file_id} ({filename})");

    // F15 — now that the binary has landed, emit `file_attached` on the LOCAL
    // bus so this instance's UI re-renders the message with its attachment.
    // The inbound `FileAttached` from the peer is consumed by ingest (returns
    // false → not re-broadcast), so without this the front never learns the
    // file arrived and it only shows on the next manual refresh. The front
    // listens for this event (reload-on-file_attached). A re-broadcast reaches
    // the origin peer too, but it already has the file → idempotent no-op.
    let _ = state.ws_broadcast.send(WsMessage::FileAttached {
        shared_discussion_id: shared_id.to_string(),
        message_id: message_id.to_string(),
        file_id: file_id.to_string(),
        filename: filename.to_string(),
        mime_type: mime_type.to_string(),
        size,
        from_invite_code: host_invite_code.to_string(),
        pending: false, // F15+ ready: binary stored → front swaps placeholder for the file
    });
}
