//! GitHub access per project (design note `agent-secret-boundary.md` §4.5).
//! No shape here ever carries a token value.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What a project persists: whether its agents receive a GitHub token, and from where.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GithubConnectionMode {
    NotConnected,
    /// The machine's token: `GH_TOKEN` / `GITHUB_TOKEN` of the backend, else `gh auth token`.
    GhLogin,
    /// A token pasted for this project, kept encrypted.
    StoredToken,
}

impl GithubConnectionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotConnected => "not_connected",
            Self::GhLogin => "gh_login",
            Self::StoredToken => "stored_token",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "gh_login" => Self::GhLogin,
            "stored_token" => Self::StoredToken,
            _ => Self::NotConnected,
        }
    }
}

/// What the UI shows: the persisted mode plus whether the machine has a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GithubConnectionState {
    NotConnected,
    /// A token exists on this machine; this project does not use it.
    AvailableButOff,
    ConnectedGhLogin,
    ConnectedStoredToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GithubMachineTokenSource {
    /// `GH_TOKEN` or `GITHUB_TOKEN` in the backend's environment.
    Environment,
    /// `gh auth token`.
    GhCli,
}

/// Read from the token prefix only; GitHub documents these prefixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GithubTokenKind {
    /// `ghp_`: personal access token (classic).
    Classic,
    /// `gho_`: OAuth token, what `gh auth login` stores.
    Oauth,
    /// `github_pat_`: fine-grained personal access token.
    FineGrained,
    /// `ghu_` / `ghs_`: GitHub App tokens.
    App,
    Unknown,
}

/// The scope a token actually has, as GitHub reported it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GithubScope {
    /// False when GitHub could not be asked or did not say; `reason` explains.
    pub verified: bool,
    pub token_kind: GithubTokenKind,
    pub login: Option<String>,
    /// OAuth scopes (`X-OAuth-Scopes`) of a classic or OAuth token.
    pub scopes: Vec<String>,
    /// Repositories a fine-grained token can reach (`owner/name`).
    pub repositories: Vec<String>,
    pub repositories_truncated: bool,
    /// The token reaches every repository of the account (e.g. classic `repo`).
    pub broad: bool,
    pub reason: Option<String>,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectGithubConnection {
    pub project_id: String,
    pub mode: GithubConnectionMode,
    pub state: GithubConnectionState,
    pub machine_token_available: bool,
    pub machine_token_source: Option<GithubMachineTokenSource>,
    /// The project's remote is on github.com.
    pub on_github: bool,
    /// Scope of the token this project uses (or would use when it is off).
    pub scope: Option<GithubScope>,
    /// Kept connected by the upgrade that introduced this setting; drives a one-time notice.
    pub connected_on_upgrade: bool,
    pub updated_at: Option<DateTime<Utc>>,
}

/// `PUT /api/projects/{id}/github`. `token` is read only for `stored_token`.
#[derive(Clone, Deserialize, TS)]
#[ts(export)]
pub struct SetProjectGithubConnectionRequest {
    pub mode: GithubConnectionMode,
    #[serde(default)]
    #[ts(optional)]
    pub token: Option<String>,
}

// Hand-written so a logged request never prints the token.
impl std::fmt::Debug for SetProjectGithubConnectionRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetProjectGithubConnectionRequest")
            .field("mode", &self.mode)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}
