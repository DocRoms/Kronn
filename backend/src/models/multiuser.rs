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
        /// Author role + agent identity, so a federated AGENT reply lands as
        /// an Agent message (with its CLI name) on the peer instead of a
        /// generic "User". `#[serde(default)]` keeps frames from an older peer
        /// (no field on the wire) decoding to the historical behaviour
        /// (role=User, agent_type=None).
        #[serde(default)]
        role: crate::models::MessageRole,
        #[serde(default)]
        agent_type: Option<crate::models::AgentType>,
    },
    /// Invitation to join a shared discussion.
    DiscussionInvite {
        shared_discussion_id: String,
        title: String,
        from_pseudo: String,
        from_invite_code: String,
    },
    /// Catch-up request sent to a peer on (re)connect: "re-send me every
    /// message in this shared discussion newer than `since_timestamp`". The
    /// peer answers by re-broadcasting the missing messages as `ChatMessage`s
    /// (idempotent on message_id). This is how a peer that was OFFLINE while
    /// messages were posted eventually becomes consistent — without it, those
    /// messages were lost forever (no outbox, fire-and-forget broadcast).
    DiscSyncRequest {
        shared_discussion_id: String,
        /// Unix-millis of the newest message the requester already has for this
        /// shared disc (0 if none). The responder sends everything strictly
        /// newer.
        since_timestamp: i64,
    },
    /// A file/doc is attached to a federated message. Federation is otherwise
    /// text-only, so without this a `context_file` (uploaded attachment or a
    /// generated PDF/DOCX) pinned to a shared-disc message never reaches the
    /// peer. The receiver fetches the binary from the host via the authenticated
    /// `POST /api/disc/fetch-file` endpoint (resolving `from_invite_code` to the
    /// host URL), stores it locally and links it to the SAME `message_id` /
    /// `file_id` (both are stable across instances). Idempotent on `file_id`.
    FileAttached {
        shared_discussion_id: String,
        message_id: String,
        file_id: String,
        filename: String,
        mime_type: String,
        size: i64,
        /// The HOST's invite code — the receiver resolves it to a contact URL to
        /// fetch the binary from.
        from_invite_code: String,
        /// F15+ — UI hint. The receiver emits this LOCALLY twice: `pending:true`
        /// the moment the announcement arrives (front shows "downloading…") and
        /// `pending:false` once the binary is stored (front shows the file). The
        /// cross-wire announcement carries `false` (default); only the receiver's
        /// own local emits use it. `#[serde(default)]` for old-peer compat.
        #[serde(default)]
        pending: bool,
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
    /// A batch child discussion's agent run just STARTED. Symmetric to
    /// `BatchRunProgress` (which fires on completion). Batch children run
    /// server-side with no SSE consumer on the client, so without this signal
    /// the per-disc `sendingMap` is never set to `true` and an in-flight child
    /// shows no "agent working" spinner. The frontend flips `sendingMap[disc]`
    /// on so the sidebar pill + open chat view render the in-progress state;
    /// `BatchRunProgress` / `BatchRunFinished` clear it on completion.
    BatchRunChildStarted {
        run_id: String,
        /// Id of the child discussion whose agent run is starting.
        discussion_id: String,
    },
    /// 0.8.2 — Linear workflow run state change. Fires on each step
    /// transition (StepStart, StepDone) AND every status flip (Running
    /// → WaitingApproval, → Success, → Failed, → Cancelled). Open
    /// WorkflowDetail panels listen and refetch the run so the user
    /// sees the live gate appear without refreshing the page.
    /// Different from `BatchRunProgress` which is fan-out specific.
    WorkflowRunUpdated {
        run_id: String,
        workflow_id: String,
        status: String,
        /// Index of the currently-running (or just-completed) step. -1 when
        /// the run starts and no step is in flight yet.
        step_index: i32,
        total_steps: u32,
        /// Step name at `step_index`, or null when between steps.
        current_step: Option<String>,
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

impl WsMessage {
    /// Whether this message may be relayed to a **remote peer** over a P2P
    /// WebSocket.
    ///
    /// Only shared-discussion traffic (chat + invites) crosses machines.
    /// Everything else is deliberately excluded:
    /// - `Presence` is delivered by the connection handshake, and peer
    ///   liveness is observed via socket close — relaying it creates an
    ///   **infinite bounce** between two instances (A broadcasts B's presence
    ///   back to B, who broadcasts it back to A…) that floods the 256-slot
    ///   broadcast channel until a subscriber lags, dropping the socket
    ///   (~2 s flap observed cross-machine).
    /// - `Ping`/`Pong` are per-connection heartbeats, not relayable events.
    /// - `BatchRun*` / `WorkflowRunUpdated` / `PartialResponseRecovered` are
    ///   **local UI signals**; forwarding them leaks one machine's internal
    ///   workflow state to another and adds to the flood.
    ///
    /// The local frontend still receives every variant (it subscribes to the
    /// broadcast bus directly); this gate applies only to the peer-facing
    /// forwarding half of a WebSocket connection.
    pub fn is_peer_relayable(&self) -> bool {
        matches!(
            self,
            WsMessage::ChatMessage { .. }
                | WsMessage::DiscussionInvite { .. }
                | WsMessage::DiscSyncRequest { .. }
                | WsMessage::FileAttached { .. }
        )
    }

    /// Stable identity used to suppress echoing a message back to the peer it
    /// arrived from (loop guard). `None` for non-relayable frames.
    pub fn relay_dedup_key(&self) -> Option<String> {
        match self {
            WsMessage::ChatMessage { message_id, .. } => Some(format!("c:{message_id}")),
            WsMessage::DiscussionInvite {
                shared_discussion_id,
                ..
            } => Some(format!("i:{shared_discussion_id}")),
            WsMessage::FileAttached { file_id, .. } => Some(format!("f:{file_id}")),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presence(code: &str) -> WsMessage {
        WsMessage::Presence {
            from_pseudo: "p".into(),
            from_invite_code: code.into(),
            online: true,
        }
    }

    fn chat(id: &str) -> WsMessage {
        WsMessage::ChatMessage {
            shared_discussion_id: "d".into(),
            message_id: id.into(),
            from_pseudo: "p".into(),
            from_avatar_email: None,
            from_invite_code: "kronn:p@h:1".into(),
            content: "hi".into(),
            timestamp: 0,
            role: crate::models::MessageRole::User,
            agent_type: None,
        }
    }

    #[test]
    fn only_shared_disc_traffic_is_peer_relayable() {
        // Presence must NOT cross the wire — that bounce is the flap root cause.
        assert!(!presence("kronn:a@h:1").is_peer_relayable());
        assert!(!WsMessage::Ping { timestamp: 0 }.is_peer_relayable());
        assert!(!WsMessage::Pong { timestamp: 0 }.is_peer_relayable());
        assert!(!WsMessage::PartialResponseRecovered { discussion_ids: vec![] }
            .is_peer_relayable());
        assert!(!WsMessage::BatchRunChildStarted {
            run_id: "r".into(),
            discussion_id: "d".into(),
        }
        .is_peer_relayable());
        // Shared-discussion traffic is the only thing relayed.
        assert!(chat("m1").is_peer_relayable());
        assert!(WsMessage::DiscussionInvite {
            shared_discussion_id: "d".into(),
            title: "t".into(),
            from_pseudo: "p".into(),
            from_invite_code: "kronn:p@h:1".into(),
        }
        .is_peer_relayable());
        // The catch-up request + its answers must reach the peer too.
        assert!(WsMessage::DiscSyncRequest {
            shared_discussion_id: "d".into(),
            since_timestamp: 0,
        }
        .is_peer_relayable());
    }

    #[test]
    fn relay_dedup_key_is_stable_per_message_and_none_for_presence() {
        assert_eq!(chat("m1").relay_dedup_key().as_deref(), Some("c:m1"));
        assert_ne!(chat("m1").relay_dedup_key(), chat("m2").relay_dedup_key());
        assert!(presence("x").relay_dedup_key().is_none());
    }

    #[test]
    fn batch_run_child_started_serializes_snake_case_and_round_trips() {
        let m = WsMessage::BatchRunChildStarted {
            run_id: "run-1".into(),
            discussion_id: "disc-1".into(),
        };
        let j = serde_json::to_value(&m).expect("serialize");
        // The frontend matches on this exact snake_case tag.
        assert_eq!(j["type"], "batch_run_child_started");
        assert_eq!(j["run_id"], "run-1");
        assert_eq!(j["discussion_id"], "disc-1");

        let back: WsMessage = serde_json::from_value(j).expect("round-trip");
        match back {
            WsMessage::BatchRunChildStarted { run_id, discussion_id } => {
                assert_eq!(run_id, "run-1");
                assert_eq!(discussion_id, "disc-1");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
