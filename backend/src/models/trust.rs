//! KT-1492 — trust levels: how much a human lets their agents do without
//! asking, set for the instance, a workflow or a room.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How far approvals that only protect the user from their own agents are
/// granted automatically. Ordered: each level includes the one before.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize, TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TrustLevel {
    /// Every sensitive action asks (the default).
    #[default]
    Standard,
    /// Agent-written command lines, room scope requests and Docker validations
    /// are granted and journalled.
    Trusted,
    /// Trusted, plus code write on other projects, unconfined validations and
    /// host access.
    Full,
}

impl TrustLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Trusted => "trusted",
            Self::Full => "full",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "standard" => Some(Self::Standard),
            "trusted" => Some(Self::Trusted),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

/// Where a trust level is set. Narrowest wins: room, workflow, instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TrustScope {
    Instance,
    Workflow,
    Room,
}

impl TrustScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Instance => "instance",
            Self::Workflow => "workflow",
            Self::Room => "room",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "instance" => Some(Self::Instance),
            "workflow" => Some(Self::Workflow),
            "room" => Some(Self::Room),
            _ => None,
        }
    }
}

/// An approval a trust level may grant on its own. Anything that is not here
/// is never covered by any level (see [`NeverCovered`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TrustApproval {
    /// An agent-written Quick Exec passing values to a CLI (KT-1490).
    QuickExecLine,
    /// An agent-written workflow Exec step passing values to a CLI.
    ExecStepLine,
    /// A room's request to read another of the user's projects (KT-1474).
    RoomScopeRead,
    /// A room's request to plan in another of the user's projects (KT-1474).
    RoomScopePlan,
    /// Integration validations reaching Docker (KT-1433).
    ValidationsDocker,
    /// Code write on another project (KT-1491).
    CodeWriteOtherProject,
    /// Integration validations without the lane sandbox (KT-1433).
    ValidationsUnconfined,
    /// Agents running with the host's rights.
    HostAccess,
}

impl TrustApproval {
    pub const ALL: [TrustApproval; 8] = [
        Self::QuickExecLine,
        Self::ExecStepLine,
        Self::RoomScopeRead,
        Self::RoomScopePlan,
        Self::ValidationsDocker,
        Self::CodeWriteOtherProject,
        Self::ValidationsUnconfined,
        Self::HostAccess,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::QuickExecLine => "quick_exec_line",
            Self::ExecStepLine => "exec_step_line",
            Self::RoomScopeRead => "room_scope_read",
            Self::RoomScopePlan => "room_scope_plan",
            Self::ValidationsDocker => "validations_docker",
            Self::CodeWriteOtherProject => "code_write_other_project",
            Self::ValidationsUnconfined => "validations_unconfined",
            Self::HostAccess => "host_access",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

/// What no level ever covers. These are not approvals a level can grant: the
/// policy table has no row for them, and each is held by its own gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum NeverCovered {
    /// Credentials and secrets stay out of agents' reach and out of prompts.
    Credentials,
    /// A bridge token (an agent) cannot change a trust setting.
    TrustSettings,
    /// Deleting Kronn data (a project, a workflow) keeps its confirmation.
    DestructiveConfirmations,
    /// A P2P peer never inherits this instance's trust.
    PeerInheritance,
}

impl NeverCovered {
    pub const ALL: [NeverCovered; 4] = [
        Self::Credentials,
        Self::TrustSettings,
        Self::DestructiveConfirmations,
        Self::PeerInheritance,
    ];
}

/// One row of the policy table, as the risk panel lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrustEffect {
    pub approval: TrustApproval,
    /// The lowest level that grants it.
    pub from_level: TrustLevel,
    /// Whether a gate in this build consults the table for it. `false`: the
    /// approval is in the policy, its gate ships with another ticket.
    pub enforced: bool,
}

/// The level in force for a room or a workflow, and where it comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EffectiveTrust {
    pub level: TrustLevel,
    /// The scope whose setting won; `Instance` for the default too.
    pub source: TrustScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_id: Option<String>,
    /// A human set it (false: the Standard default).
    pub explicit: bool,
}

/// The one setting of a scope, if a human set it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrustSetting {
    pub scope: TrustScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scope_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub level: Option<TrustLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub set_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub set_at: Option<String>,
}

/// GET /api/trust
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrustOverview {
    pub instance: TrustSetting,
    pub effects: Vec<TrustEffect>,
    pub never_covered: Vec<NeverCovered>,
    pub sandbox: TrustSandboxView,
    pub proof: TrustProofView,
}

/// The protection of Kronn's vault from what a level grants: the data directory
/// is hidden from level-approved commands when the host has a sandbox (macOS
/// Seatbelt, Linux bubblewrap) and the owner kept "Protect the Kronn vault from
/// agents" on. Without it a level is still allowed; the panel names the missing
/// protection and raising it needs an acknowledgement of that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrustSandboxView {
    /// The host can apply a sandbox.
    pub available: bool,
    /// Why not, when `available` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<String>,
    /// The "Protect the Kronn vault from agents" setting (on by default).
    pub protect_vault: bool,
    /// The vault is protected: `available` and `protect_vault`.
    pub protected: bool,
    /// The sentence naming the missing protection, when `protected` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub missing: Option<String>,
}

/// The state of the strong confirmation (TOTP) that raising a level needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrustProofView {
    /// A confirmed enrolment exists.
    pub enrolled: bool,
    /// A seed was issued but its first code was not confirmed yet.
    pub pending: bool,
    pub recovery_codes_left: u32,
    /// Seconds left of a lockout (5 wrong codes lock it, 60 s then longer); 0 when open.
    pub locked_for_secs: u32,
}

/// A proof of the strong confirmation: the authenticator's current code, or a
/// recovery code. Needed to disable the confirmation or renew its recovery codes.
#[derive(Debug, Clone, Default, Deserialize, TS)]
#[ts(export)]
pub struct TrustProofRequest {
    #[serde(default)]
    pub proof: Option<String>,
    /// The human-admin secret (the `human-admin-secret` file in the Kronn data
    /// directory). Needed to start the first enrolment: until a code is
    /// enrolled it proves nothing, so the root of trust is the file.
    #[serde(default)]
    pub admin_secret: Option<String>,
}

/// The first code of an authenticator, to confirm the enrolment.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct ConfirmTrustProofRequest {
    pub code: String,
    /// The human-admin secret, as for starting the enrolment.
    #[serde(default)]
    pub admin_secret: Option<String>,
}

/// The seed an authenticator app takes, shown once at enrolment.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrustEnrolment {
    /// The base32 key, for a manual entry.
    pub secret: String,
    /// The `otpauth://` URI a QR code carries.
    pub uri: String,
}

/// Ten single-use recovery codes, shown once: only their digests are kept.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrustRecoveryCodes {
    pub codes: Vec<String>,
}

/// One journal line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrustAuditEntry {
    pub id: String,
    pub at: String,
    /// `grant` (an approval granted on its own), `setting` (a level changed) or
    /// `withdrawn` (a grant taken back because its level was lowered), `proof`
    /// (a confirmation code was spent on a call) or `proof_refused` (a code was
    /// wrong, missing or already used). Both name the channel: ui, http, bridge.
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval: Option<TrustApproval>,
    pub level: TrustLevel,
    pub source: TrustScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub discussion_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub workflow_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<String>,
    pub subject: String,
    pub detail: String,
    /// What the grant approved: `quick_exec`, `workflow_step`, `room_scope_grant`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub object_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub object_id: Option<String>,
    /// A step name, or the room of a scope grant.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub object_ref: Option<String>,
    /// The object still holds the approval: lowering the level withdraws it.
    pub active: bool,
}

/// PUT /api/trust/...
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct SetTrustRequest {
    pub level: TrustLevel,
    /// The human read the risk panel and confirmed. Required above Standard.
    #[serde(default)]
    pub acknowledged_risks: bool,
    /// The human read that the Kronn vault is not protected on this machine (no
    /// sandbox, or the protection turned off in the config) and still chose the
    /// level. Required to raise a level while the vault is unprotected.
    #[serde(default)]
    pub acknowledged_vault_unprotected: bool,
    /// The strong confirmation (authenticator code or recovery code). Required
    /// above Standard; lowering never needs it. The `X-Kronn-Confirm` header
    /// carries it as well, on any channel.
    #[serde(default)]
    pub proof: Option<String>,
}
