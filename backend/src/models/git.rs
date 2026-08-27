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
    /// Bounded page of commits attributable to the selected workspace/range,
    /// newest first. Empty is an honest value: a Direct main checkout without
    /// a declared baseline cannot be retroactively attributed to one discussion.
    #[serde(default)]
    pub commits: Vec<GitCommitSummary>,
    /// Total number of attributable commits in the range, independently of
    /// the bounded page returned in `commits`.
    #[serde(default)]
    pub commits_total: u32,
    /// Zero-based offset of the bounded page returned in `commits`.
    #[serde(default)]
    pub commits_offset: u32,
    /// True when at least one later page remains after `commits`.
    #[serde(default)]
    pub commits_truncated: bool,
    /// Effective workspace provenance for discussion-scoped requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<GitWorkspaceProvenance>,
    /// Human-readable explanation when no file/commit can be shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub empty_reason: Option<String>,
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
pub struct GitCommitSummary {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub author_name: String,
    pub author_time: i64,
}

#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct GitWorkspaceProvenance {
    pub workspace_id: Option<String>,
    pub ownership: String,
    pub state: String,
    pub path: Option<String>,
    pub branch: String,
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
    pub integrated_sha: Option<String>,
    pub task_execution_id: Option<String>,
    pub task_reference: Option<String>,
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

/// KT-67 — what an annotated line is actually about. `git blame` gives a hash
/// and an author; this is the story behind it.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitCommitDetail {
    pub sha: String,
    pub short_sha: String,
    pub author_name: String,
    pub author_email: String,
    /// Unix seconds — the frontend already formats blame dates this way.
    pub author_time: i64,
    pub committer_name: String,
    pub commit_time: i64,
    pub subject: String,
    /// Body without the subject. Empty when the commit has a one-line message.
    pub body: String,
    /// Branches containing this commit, bounded — a commit near the root of a
    /// busy repo is contained by hundreds, and nobody reads that list.
    pub branches: Vec<String>,
    /// True when `branches` was cut, so the UI can say "and N more" honestly
    /// instead of implying the list is complete.
    pub branches_truncated: bool,
    /// Files touched, for scale. The diff itself stays out: this is a tooltip,
    /// not a review surface, and a merge commit would dwarf the response.
    pub files_changed: u32,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct GitCommitQuery {
    /// Commit-ish as reported by blame. Validated before reaching git.
    pub sha: String,
}

/// KT-75 — the commit's own patch, parent → commit. Not a comparison against
/// the current branch: the point is to read the change as it was made, whatever
/// happened to the file since.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitCommitPatch {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    /// Unified diff for every file in the commit. Empty for a commit that
    /// touched nothing (an empty commit is legal).
    pub patch: String,
    /// True when the patch hit the byte cap. A merge or a vendored-tree import
    /// runs to megabytes, and a viewer that silently stops mid-hunk reads as a
    /// complete diff.
    pub truncated: bool,
    pub files_changed: u32,
    /// A root commit has no parent, so its patch is the whole file set added.
    /// Worth saying out loud rather than letting the reader wonder.
    pub is_root: bool,
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

#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct GitBranchSummary {
    /// Short ref name (`main`, `feature/foo` or `origin/main`).
    pub name: String,
    /// Full ref name, retained so the backend never has to reconstruct it.
    pub ref_name: String,
    pub commit: String,
    pub subject: String,
    pub author: String,
    pub committed_at: i64,
    pub is_current: bool,
    pub is_remote: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct GitGraphCommit {
    pub hash: String,
    pub short_hash: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
    pub subject: String,
    pub author: String,
    pub committed_at: i64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitBranchesResponse {
    pub current_branch: String,
    pub default_branch: String,
    pub branches: Vec<GitBranchSummary>,
    pub commits: Vec<GitGraphCommit>,
    /// True when more commits exist than the bounded graph returned here.
    pub truncated: bool,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct GitSwitchBranchRequest {
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
