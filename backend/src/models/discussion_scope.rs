//! KT-1270 — the projects a room may reach besides its primary one.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ScopeAccess {
    Read,
    Write,
}

impl ScopeAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            _ => anyhow::bail!("unknown scope access: {value}"),
        }
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
}

impl ScopeGrantSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Link => "link",
            Self::Request => "request",
            Self::All => "all",
        }
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "link" => Ok(Self::Link),
            "request" => Ok(Self::Request),
            "all" => Ok(Self::All),
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
}

/// The confirmation a human types to grant write access to every project.
pub const ALL_PROJECTS_WRITE_CONFIRMATION: &str = "ALL-PROJECTS-WRITE";

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct GrantDiscussionProjectRequest {
    /// The project to open; omitted with `all: true`.
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub all: bool,
    pub access: ScopeAccess,
    /// Required for every project in write: [`ALL_PROJECTS_WRITE_CONFIRMATION`].
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
}

/// Withdraw one project from a workflow's scope, or all of it.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct RevokeWorkflowScopeRequest {
    /// `None` withdraws every project.
    #[serde(default)]
    pub project_id: Option<String>,
}
