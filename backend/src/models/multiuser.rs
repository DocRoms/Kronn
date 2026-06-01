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

#[cfg(test)]
mod tests {
    use super::*;

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
