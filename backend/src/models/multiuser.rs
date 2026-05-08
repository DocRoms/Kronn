// Multi-user / P2P concerns: contacts (peer Kronn instances) and the
// real-time WebSocket protocol used to exchange presence + chat + batch
// progress events between them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Contact {
    pub id: String,
    pub pseudo: String,
    pub avatar_email: Option<String>,
    pub kronn_url: String,
    pub invite_code: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct AddContactRequest {
    pub invite_code: String,
}

/// Result of adding a contact, with optional diagnostic hint for unreachable peers.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AddContactResult {
    pub contact: Contact,
    /// Human-readable hint explaining why the contact is pending (network mismatch, etc.)
    pub warning: Option<String>,
}

/// Network info for multi-user connectivity.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct NetworkInfo {
    /// Detected Tailscale IPv4 address (100.x.x.x), if available.
    pub tailscale_ip: Option<String>,
    /// The host used in invite codes (domain > tailscale > host).
    pub advertised_host: String,
    /// Backend port.
    pub port: u16,
    /// Configured domain, if any.
    pub domain: Option<String>,
    /// All detected network IPs (tailscale, vpn, lan).
    pub detected_ips: Vec<crate::core::tailscale::DetectedIp>,
}

/// Real-time message exchanged between Kronn instances via WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    /// Presence announcement: a peer is online or offline.
    Presence {
        from_pseudo: String,
        from_invite_code: String,
        online: bool,
    },
    /// Heartbeat ping (sent by client).
    Ping { timestamp: i64 },
    /// Heartbeat pong (reply to ping).
    Pong { timestamp: i64 },
    /// Chat message in a shared discussion.
    ChatMessage {
        shared_discussion_id: String,
        message_id: String,
        from_pseudo: String,
        from_avatar_email: Option<String>,
        from_invite_code: String,
        content: String,
        timestamp: i64,
    },
    /// Invitation to join a shared discussion.
    DiscussionInvite {
        shared_discussion_id: String,
        title: String,
        from_pseudo: String,
        from_invite_code: String,
    },
    /// A batch WorkflowRun finished (all child discussions are done).
    /// Sent by the backend to the frontend so the sidebar badge and any
    /// open batch monitors update live.
    BatchRunFinished {
        run_id: String,
        /// Id of the child discussion whose completion triggered the final tick.
        /// The frontend uses it to clear its per-disc `sendingMap` spinner, since
        /// batch children are fire-and-forget (no SSE stream consumer on the client
        /// to drive the usual cleanup path).
        discussion_id: String,
        batch_name: Option<String>,
        batch_total: u32,
        batch_completed: u32,
        batch_failed: u32,
    },
    /// Progress update for a running batch (child disc just finished).
    /// Fires on every child completion so the sidebar pill can tick live.
    BatchRunProgress {
        run_id: String,
        /// Id of the child discussion that just completed — frontend uses it to
        /// clear the per-disc sendingMap indicator.
        discussion_id: String,
        batch_total: u32,
        batch_completed: u32,
        batch_failed: u32,
    },
    /// Broadcast once at backend boot when `recover_partial_responses`
    /// resurrected in-flight agent responses that were cut short by a
    /// restart. Each id in the list got a new Agent message with an
    /// "interrupted" footer — the frontend refetches those discs + toasts
    /// the user so they don't resend their prompt on top of a silently
    /// recovered conversation.
    PartialResponseRecovered {
        discussion_ids: Vec<String>,
    },
}
