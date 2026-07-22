// Git Operations — request/response types for the `/api/projects/:id/git/*`
// and `/api/discussions/:id/git/*` endpoints.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitStatusResponse {
    pub branch: String,
    pub default_branch: String,
    pub is_default_branch: bool,
    pub files: Vec<GitFileStatus>,
    /// Files committed on this branch but not yet on default branch.
    /// Empty when on the default branch or when no default branch resolves.
    /// Lets the "Fichiers" panel surface the disc's cumulative work
    /// (what would land in the next merge), not just the uncommitted slice.
    #[serde(default)]
    pub committed_files: Vec<GitFileStatus>,
    pub ahead: u32,
    pub behind: u32,
    pub has_upstream: bool,
    /// Tracking ref for the current branch (for example `origin/main`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    pub provider: String, // "github", "gitlab", or "unknown"
    /// Browser-safe repository URL derived from `remote.origin.url`.
    /// Credentials embedded in HTTPS remotes are deliberately stripped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
    /// Provider-specific shortcut to the repository's PR/MR listing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_requests_url: Option<String>,
    /// Most recently created tag in the local Git object database.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    /// Lightweight source-language breakdown, enriched for project requests.
    /// Discussion Git panels leave it empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<ProjectLanguageStat>,
    /// Timestamp of the source-language scan. Absent on discussion Git panels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub languages_checked_at: Option<DateTime<Utc>>,
    /// True when `languages` came from the bounded in-memory project cache.
    #[serde(default)]
    pub languages_cached: bool,
}

#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ProjectLanguageStat {
    pub language: String,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct CachedProjectLanguages {
    pub inserted_at: std::time::Instant,
    pub checked_at: DateTime<Utc>,
    pub exclusions: Vec<String>,
    pub languages: Vec<ProjectLanguageStat>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitFileStatus {
    pub path: String,
    pub status: String,
    pub staged: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitDiffResponse {
    pub path: String,
    pub diff: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitBlameLine {
    pub line_number: u32,
    pub commit: String,
    pub author: String,
    pub author_time: i64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitBlameResponse {
    pub path: String,
    pub lines: Vec<GitBlameLine>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct GitBlameQuery {
    pub path: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct GitDiffQuery {
    pub path: String,
    /// When true, return the COMMITTED diff for this path (`<default>...HEAD`)
    /// instead of the working-tree diff. Used by the GitPanel "committed on
    /// branch" section, where the working tree is clean so a plain `git diff`
    /// would be empty.
    #[serde(default)]
    pub committed: Option<bool>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct GitBranchRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitBranchResponse {
    pub branch: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct GitCommitRequest {
    pub files: Vec<String>,
    pub message: String,
    #[serde(default)]
    pub amend: bool,
    #[serde(default)]
    pub sign: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitCommitResponse {
    pub hash: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitPushResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct CreatePrRequest {
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default = "default_pr_base")]
    pub base: String,
}

fn default_pr_base() -> String {
    "main".into()
}

#[derive(Debug, Deserialize)]
pub struct ExecRequest {
    pub command: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ExecResponse {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
