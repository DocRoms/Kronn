// Git Operations — request/response types for the `/api/projects/:id/git/*`
// and `/api/discussions/:id/git/*` endpoints.

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
    pub provider: String,  // "github", "gitlab", or "unknown"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
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

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct GitDiffQuery {
    pub path: String,
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

fn default_pr_base() -> String { "main".into() }

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

