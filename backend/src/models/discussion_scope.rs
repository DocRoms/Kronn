//! KT-1270 — the projects a room may reach besides its primary one.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::AgentType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ScopeAccess {
    /// Reads the project: its planning in Kronn's tools, and its checkout as a
    /// read-only directory.
    Read,
    /// Plans in the project through Kronn's tools (tasks); its files stay
    /// read-only. This is what `write` always meant: the stored and wire
    /// `write` of before KT-1491 is still read as `plan`.
    #[serde(alias = "write")]
    Plan,
    /// Plans, and the agent edits the project's checkout like its own. Never
    /// opens the project's credentials, and is never granted for every project.
    Code,
}

impl ScopeAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Plan => "plan",
            Self::Code => "code",
        }
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "read" => Ok(Self::Read),
            // `write` is the stored and wire value from before KT-1491.
            "plan" | "write" => Ok(Self::Plan),
            "code" => Ok(Self::Code),
            _ => anyhow::bail!("unknown scope access: {value}"),
        }
    }

    /// Whether the level changes the project's planning (`plan` and `code`).
    pub fn plans(self) -> bool {
        self != Self::Read
    }

    fn rank(self) -> u8 {
        match self {
            Self::Read => 0,
            Self::Plan => 1,
            Self::Code => 2,
        }
    }

    /// Whether this level contains `other`: read < plan < code.
    pub fn covers(self, other: Self) -> bool {
        self.rank() >= other.rank()
    }

    /// The stronger of two levels.
    pub fn stronger(self, other: Self) -> Self {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

/// What a launch found at a granted project's registered path. Asked before any
/// launch (a grant receipt), nothing is known yet: [`Self::Openable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutState {
    /// A Git checkout root the launch can open for editing.
    Openable,
    /// Its `core.hooksPath` is the checkout itself: every file would be a hook.
    HooksAtRoot,
    /// No checkout the launch can open: a link, a plain folder, the working
    /// directory, Kronn's data, or a host without the sandbox.
    NotOpenable,
    /// Granted beyond what the launch's sandbox carries (KT-1971).
    LeftOut,
}

/// Why an agent cannot edit a granted project in one launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditBlock {
    /// The grant is `read` or `plan`: only `code` lets an agent edit.
    GrantLevel,
    /// Codex with its full access off: its sandbox writes no project.
    CodexSandbox,
    /// A native ACP runtime: its permission broker refuses every file outside
    /// the session's own project, full access or not (KT-2103).
    NativeBroker,
    /// An HTTP model: its file tools reach the room's own project only.
    HttpTools,
    /// [`CheckoutState::HooksAtRoot`].
    HooksAtRoot,
    /// [`CheckoutState::NotOpenable`].
    NotOpenable,
    /// [`CheckoutState::LeftOut`].
    BeyondTheSandbox,
}

/// Whether an agent can edit a granted project in one launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAccess {
    /// Its checkout is opened writable for this launch (Claude Code).
    Opened,
    /// The launch is not sandboxed: it writes the project as it writes its own.
    Unconfined,
    Blocked(EditBlock),
}

impl EditAccess {
    pub fn allowed(self) -> bool {
        !matches!(self, Self::Blocked(_))
    }
}

/// KT-2102 — the one place that decides whether `agent` can edit a project
/// granted at `access`, and what stops it: the room's scope block and the grant
/// receipt are both worded from it. `full_access` is what the launch really
/// runs with (`AgentsConfig::effective_full_access`), not the stored switch.
pub fn edit_access(
    agent: &AgentType,
    full_access: bool,
    access: ScopeAccess,
    checkout: CheckoutState,
) -> EditAccess {
    if access != ScopeAccess::Code {
        return EditAccess::Blocked(EditBlock::GrantLevel);
    }
    match agent {
        AgentType::OpenCode
        | AgentType::GeminiCli
        | AgentType::CopilotCli
        | AgentType::Kiro
        | AgentType::Vibe => EditAccess::Blocked(EditBlock::NativeBroker),
        AgentType::Ollama | AgentType::LiteLlm | AgentType::Nvidia | AgentType::Custom => {
            EditAccess::Blocked(EditBlock::HttpTools)
        }
        AgentType::ClaudeCode | AgentType::Codex if full_access => EditAccess::Unconfined,
        AgentType::Codex => EditAccess::Blocked(EditBlock::CodexSandbox),
        AgentType::ClaudeCode => match checkout {
            CheckoutState::Openable => EditAccess::Opened,
            CheckoutState::HooksAtRoot => EditAccess::Blocked(EditBlock::HooksAtRoot),
            CheckoutState::NotOpenable => EditAccess::Blocked(EditBlock::NotOpenable),
            CheckoutState::LeftOut => EditAccess::Blocked(EditBlock::BeyondTheSandbox),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ScopeGrantSource {
    /// Set by a human in the room settings.
    Link,
    /// An agent's request a human approved.
    Request,
    /// Every project.
    All,
    /// The general resources (KT-1346): a room without a project writes the
    /// Quick Prompts, Quick APIs, Pages and workflows every project sees.
    General,
}

impl ScopeGrantSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Link => "link",
            Self::Request => "request",
            Self::All => "all",
            Self::General => "general",
        }
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "link" => Ok(Self::Link),
            "request" => Ok(Self::Request),
            "all" => Ok(Self::All),
            "general" => Ok(Self::General),
            _ => anyhow::bail!("unknown scope grant source: {value}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionProjectGrant {
    pub id: String,
    pub discussion_id: String,
    /// `None` = every project.
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub access: ScopeAccess,
    pub source: ScopeGrantSource,
    pub request_id: Option<String>,
    pub granted_by: String,
    pub granted_at: String,
    pub revoked_by: Option<String>,
    pub revoked_at: Option<String>,
    pub revoked_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ScopeRequestStatus {
    Pending,
    Approved,
    Declined,
}

impl ScopeRequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Declined => "declined",
        }
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "declined" => Ok(Self::Declined),
            _ => anyhow::bail!("unknown scope request status: {value}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionScopeRequest {
    pub id: String,
    pub discussion_id: String,
    /// `None` when the agent named no single project.
    pub project_id: Option<String>,
    /// The project as the agent named it.
    pub requested_name: String,
    pub project_name: Option<String>,
    pub access: ScopeAccess,
    pub reason: String,
    pub requested_by: String,
    pub status: ScopeRequestStatus,
    pub decided_by: Option<String>,
    pub decided_at: Option<String>,
    pub granted_access: Option<ScopeAccess>,
    pub created_at: String,
}

/// A room's whole scope, as the human panel and the agent see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionScope {
    pub discussion_id: String,
    pub primary_project_id: Option<String>,
    /// Active grants.
    pub grants: Vec<DiscussionProjectGrant>,
    /// Revoked grants, newest first: the journal.
    pub revoked: Vec<DiscussionProjectGrant>,
    /// Pending requests, then the latest decided ones.
    pub requests: Vec<DiscussionScopeRequest>,
    /// Digest of the active grants: a human decision names the scope it saw.
    pub fingerprint: String,
    /// Names of the registered projects this room could ask for (KT-1417):
    /// none granted, never the primary, and none for a room without a project.
    /// The only place an agent learns them, on demand: the prompt names none.
    pub requestable: Vec<String>,
}

/// The confirmation a human types to grant write access to every project.
pub const ALL_PROJECTS_WRITE_CONFIRMATION: &str = "ALL-PROJECTS-WRITE";

/// The confirmation a human types to let a room without a project write the
/// general resources.
pub const GENERAL_WRITE_CONFIRMATION: &str = "GENERAL-WRITE";

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct GrantDiscussionProjectRequest {
    /// The project to open; omitted with `all: true`.
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub all: bool,
    /// The general resources, for a room without a project; write only.
    #[serde(default)]
    pub general: bool,
    pub access: ScopeAccess,
    /// Required for every project in write: [`ALL_PROJECTS_WRITE_CONFIRMATION`];
    /// for the general resources: [`GENERAL_WRITE_CONFIRMATION`].
    #[serde(default)]
    pub confirm: Option<String>,
    /// The scope fingerprint the human's screen showed.
    #[serde(default)]
    pub expected_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct RevokeDiscussionProjectRequest {
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DecideScopeRequest {
    /// Approve only: the access granted, at most the one asked for.
    #[serde(default)]
    pub access: Option<ScopeAccess>,
}

/// An agent's request for another project.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct FileScopeRequest {
    /// The project's name or id.
    pub project: String,
    pub access: ScopeAccess,
    pub reason: String,
}

/// One project a workflow's runs reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowScopeEntry {
    pub project_id: String,
    pub project_name: Option<String>,
    pub access: ScopeAccess,
}

/// The projects a workflow's runs reach (KT-1270 DoD: the scope a workflow
/// declares at launch). `valid` is false once the workflow changed after a
/// human validated it: its runs then reach nothing more than their project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowScope {
    pub workflow_id: String,
    /// False for a global or multi-project workflow, which cannot carry one.
    pub supported: bool,
    pub entries: Vec<WorkflowScopeEntry>,
    /// Every project (KT-1336), besides the named entries.
    #[serde(default)]
    pub all: Option<ScopeAccess>,
    /// Digest of the scope as stored: every project in write names the one
    /// the human saw.
    #[serde(default)]
    pub fingerprint: String,
    pub valid: bool,
    pub validated_by: Option<String>,
    pub validated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowScopeItem {
    pub project_id: String,
    pub access: ScopeAccess,
}

/// Validate a workflow's scope as it is now; an empty list clears it.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct SetWorkflowScopeRequest {
    pub entries: Vec<WorkflowScopeItem>,
    /// Every project (KT-1336), read or write.
    #[serde(default)]
    pub all: Option<ScopeAccess>,
    /// Required for every project in write: [`ALL_PROJECTS_WRITE_CONFIRMATION`].
    #[serde(default)]
    pub confirm: Option<String>,
    /// The scope fingerprint the human's screen showed.
    #[serde(default)]
    pub expected_fingerprint: Option<String>,
}

/// Withdraw one project from a workflow's scope, or all of it.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct RevokeWorkflowScopeRequest {
    /// `None` (and `all` false) withdraws everything.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Withdraw only the every-project entry.
    #[serde(default)]
    pub all: bool,
}
