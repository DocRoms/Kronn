//! Discussion-scoped Git Operations — git status, diff, commit, push, worktree lock/unlock, exec.

use axum::{
    extract::{Path, Query, State},
    Json,
};

use crate::core::cmd::sync_cmd;
use crate::models::*;
use crate::AppState;

// ═══════════════════════════════════════════════════════════════════════════════
// Discussion-scoped Git Operations
// ═══════════════════════════════════════════════════════════════════════════════

/// Resolve the working directory for a discussion.
/// Returns (work_dir, project_path) — work_dir is the worktree path if isolated, else project path.
/// Resolve GitHub token from MCP configs for git operations (push, PR creation).
async fn resolve_github_token_from_state(state: &AppState) -> Option<String> {
    let cfg = state.config.read().await;
    let secret = cfg.encryption_secret.clone()?;
    drop(cfg);
    let db = state.db.clone();
    db.with_conn(move |conn| Ok(super::git_ops::resolve_github_token(conn, &secret)))
        .await
        .ok()
        .flatten()
}

async fn resolve_discussion_work_dir(
    state: &AppState,
    discussion_id: &str,
    workspace_id: Option<&str>,
) -> Result<(std::path::PathBuf, String), String> {
    if let Some(workspace_id) = workspace_id {
        let did = discussion_id.to_string();
        let wid = workspace_id.to_string();
        let (workspace, project) = state
            .db
            .with_read_conn(move |conn| {
                let workspace =
                    crate::db::discussion_workspaces::get_visible_for_discussion(conn, &did, &wid)?
                        .ok_or_else(|| anyhow::anyhow!("Workspace not found in this discussion"))?;
                let project = crate::db::projects::get_project(conn, &workspace.project_id)?
                    .ok_or_else(|| anyhow::anyhow!("Project not found"))?;
                Ok((workspace, project))
            })
            .await
            .map_err(|error| format!("DB error: {error}"))?;
        if workspace.state != "attached" {
            return Err(format!("Workspace is {}", workspace.state));
        }
        let path = workspace
            .canonical_path
            .or(workspace.workspace_path)
            .ok_or_else(|| "Workspace has no attached path".to_string())?;
        let resolved = crate::core::scanner::resolve_host_path(&path);
        if !resolved.exists() {
            return Err(format!("Worktree path not found: {}", resolved.display()));
        }
        return Ok((resolved, project.path));
    }

    let did = discussion_id.to_string();
    let disc = state
        .db
        .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &did))
        .await
        .map_err(|e| format!("DB error: {}", e))?;
    let disc = disc.ok_or_else(|| "Discussion not found".to_string())?;

    let project_id = disc
        .project_id
        .ok_or_else(|| "Discussion has no project".to_string())?;

    let pid = project_id.clone();
    let project = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
        .await
        .map_err(|e| format!("DB error: {}", e))?;
    let project = project.ok_or_else(|| "Project not found".to_string())?;

    if let Some(ref wp) = disc.workspace_path {
        let resolved = crate::core::scanner::resolve_host_path(wp);
        if !resolved.exists() {
            return Err(format!("Worktree path not found: {}", resolved.display()));
        }
        Ok((resolved, project.path))
    } else {
        let resolved = crate::core::scanner::resolve_host_path(&project.path);
        if !resolved.exists() {
            return Err(format!("Project path not found: {}", resolved.display()));
        }
        Ok((resolved, project.path))
    }
}

async fn selected_workspace_context(
    state: &AppState,
    discussion_id: &str,
    workspace_id: &str,
) -> Result<
    (
        crate::db::discussion_workspaces::DiscussionWorkspace,
        Project,
        Option<TaskExecution>,
        Option<TaskExecutionDelivery>,
    ),
    String,
> {
    let did = discussion_id.to_string();
    let wid = workspace_id.to_string();
    state
        .db
        .with_read_conn(move |conn| {
            let workspace =
                crate::db::discussion_workspaces::get_visible_for_discussion(conn, &did, &wid)?
                    .ok_or_else(|| anyhow::anyhow!("Workspace not found in this discussion"))?;
            let project = crate::db::projects::get_project(conn, &workspace.project_id)?
                .ok_or_else(|| anyhow::anyhow!("Project not found"))?;
            let execution = workspace
                .task_execution_id
                .as_deref()
                .map(|id| crate::db::orchestration::get_task_execution(conn, id))
                .transpose()?
                .flatten();
            let delivery = workspace
                .task_execution_id
                .as_deref()
                .map(|id| crate::db::worker_deliveries::list_deliveries(conn, id))
                .transpose()?
                .and_then(|items| items.into_iter().last());
            Ok((workspace, project, execution, delivery))
        })
        .await
        .map_err(|error| format!("DB error: {error}"))
}

fn workspace_provenance(
    workspace: &crate::db::discussion_workspaces::DiscussionWorkspace,
    execution: Option<&TaskExecution>,
    state: &str,
) -> GitWorkspaceProvenance {
    GitWorkspaceProvenance {
        workspace_id: Some(workspace.id.clone()),
        ownership: workspace.ownership.clone(),
        state: state.to_string(),
        path: workspace.workspace_path.clone(),
        branch: workspace.branch.clone(),
        base_sha: workspace.base_sha.clone(),
        head_sha: execution
            .and_then(|item| item.candidate_target_sha.clone())
            .or_else(|| workspace.head_sha.clone()),
        integrated_sha: execution.and_then(|item| item.integrated_sha.clone()),
        task_execution_id: workspace.task_execution_id.clone(),
        task_reference: workspace.task_reference.clone(),
    }
}

async fn status_for_selected_workspace(
    state: &AppState,
    discussion_id: &str,
    workspace_id: &str,
    commit_offset: u32,
    commit_limit: u32,
) -> Result<GitStatusResponse, String> {
    let (workspace, project, execution, delivery) =
        selected_workspace_context(state, discussion_id, workspace_id).await?;
    let checkout = workspace
        .canonical_path
        .as_deref()
        .map(crate::core::scanner::resolve_host_path);
    let attached =
        workspace.state == "attached" && checkout.as_ref().is_some_and(|path| path.exists());
    let effective_state = if attached {
        "attached"
    } else if workspace.state == "attached" {
        "missing"
    } else {
        workspace.state.as_str()
    };
    let repo = if attached {
        checkout.expect("attached workspace checked above")
    } else {
        crate::core::scanner::resolve_host_path(&project.path)
    };
    if !repo.exists() {
        return Err(format!("Project path not found: {}", repo.display()));
    }

    let live_head = attached
        .then(|| crate::core::worktree::resolve_commit(&repo, "HEAD").ok())
        .flatten();
    let head = delivery
        .as_ref()
        .map(|item| item.head_sha.clone())
        .or(live_head)
        .or_else(|| workspace.head_sha.clone());
    let has_durable_range = workspace.base_sha.is_some() && head.is_some();
    let mut status = if has_durable_range {
        super::git_ops::run_git_status_without_commit_evidence(&repo)?
    } else {
        super::git_ops::run_git_status_page(&repo, commit_offset, commit_limit)?
    };
    status.workspace = Some(workspace_provenance(
        &workspace,
        execution.as_ref(),
        effective_state,
    ));
    if let Some(provenance) = status.workspace.as_mut() {
        provenance.head_sha = head.clone();
    }
    status.branch = workspace.branch.clone();
    status.is_default_branch = status.branch == status.default_branch;

    if let (Some(base), Some(head)) = (workspace.base_sha.as_deref(), head.as_deref()) {
        let (files, commits, total, truncated) =
            super::git_ops::run_git_range_page(&repo, base, head, commit_offset, commit_limit)?;
        status.committed_files = files;
        status.commits = commits;
        status.commits_total = total;
        status.commits_offset = commit_offset;
        status.commits_truncated = truncated;
        status.ahead = total;
    }
    if !attached {
        status.files.clear();
        status.behind = 0;
        status.has_upstream = false;
        status.upstream = None;
        status.pr_url = None;
    }
    if status.files.is_empty() && status.committed_files.is_empty() {
        status.empty_reason = Some(if effective_state == "attached" {
            "Workspace clean: no discussion-attributable file change.".to_string()
        } else {
            format!(
                "Workspace {effective_state}: the checkout is no longer available and its durable commit range contains no file change."
            )
        });
    }
    Ok(status)
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct DiscWorkspaceSelection {
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub commit_offset: u32,
    #[serde(default = "default_disc_git_commit_limit")]
    pub commit_limit: u32,
}

fn default_disc_git_commit_limit() -> u32 {
    super::git_ops::GIT_COMMIT_PAGE_DEFAULT
}

#[derive(Debug, serde::Deserialize)]
pub struct DiscGitDiffQuery {
    pub path: String,
    #[serde(default)]
    pub committed: Option<bool>,
    #[serde(default)]
    pub workspace_id: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct DiscGitCommitRequest {
    pub files: Vec<String>,
    pub message: String,
    #[serde(default)]
    pub amend: bool,
    #[serde(default)]
    pub sign: bool,
    #[serde(default)]
    pub workspace_id: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct DiscCreatePrRequest {
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default = "default_disc_pr_base")]
    pub base: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
}

fn default_disc_pr_base() -> String {
    "main".into()
}

#[derive(Debug, serde::Deserialize)]
pub struct DiscExecRequest {
    pub command: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
}

/// GET /api/discussions/:id/git-status
pub async fn disc_git_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DiscWorkspaceSelection>,
) -> Json<ApiResponse<GitStatusResponse>> {
    if let Some(workspace_id) = query.workspace_id.as_deref() {
        return match status_for_selected_workspace(
            &state,
            &id,
            workspace_id,
            query.commit_offset,
            query.commit_limit,
        )
        .await
        {
            Ok(status) => Json(ApiResponse::ok(status)),
            Err(error) => Json(ApiResponse::err(error)),
        };
    }
    let (work_dir, _) =
        match resolve_discussion_work_dir(&state, &id, query.workspace_id.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Json(ApiResponse::err(e)),
        };
    let display_path = work_dir.to_string_lossy().to_string();
    let status_dir = work_dir.clone();
    let commit_offset = query.commit_offset;
    let commit_limit = query.commit_limit;

    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_git_status_page(&status_dir, commit_offset, commit_limit)
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(mut status) => {
            let head = crate::core::worktree::resolve_commit(&work_dir, "HEAD").ok();
            status.workspace = Some(GitWorkspaceProvenance {
                workspace_id: None,
                ownership: "direct".to_string(),
                state: "attached".to_string(),
                path: Some(display_path),
                branch: status.branch.clone(),
                base_sha: None,
                head_sha: head,
                integrated_sha: None,
                task_execution_id: None,
                task_reference: None,
            });
            if status.files.is_empty() && status.committed_files.is_empty() {
                status.empty_reason = Some(
                    "Direct workspace clean. No declared baseline exists to attribute earlier commits to this discussion."
                        .to_string(),
                );
            }
            Json(ApiResponse::ok(status))
        }
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// GET /api/discussions/:id/git-diff?path=...
pub async fn disc_git_diff(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DiscGitDiffQuery>,
) -> Json<ApiResponse<GitDiffResponse>> {
    if query.path.contains("..") {
        return Json(ApiResponse::err("Invalid path"));
    }

    if query.committed.unwrap_or(false) {
        if let Some(workspace_id) = query.workspace_id.as_deref() {
            let context = selected_workspace_context(&state, &id, workspace_id).await;
            let (workspace, project, _execution, delivery) = match context {
                Ok(context) => context,
                Err(error) => return Json(ApiResponse::err(error)),
            };
            let head = delivery
                .as_ref()
                .map(|item| item.head_sha.as_str())
                .or(workspace.head_sha.as_deref());
            if let (Some(base), Some(head)) = (workspace.base_sha.as_deref(), head) {
                let repo = crate::core::scanner::resolve_host_path(&project.path);
                return match super::git_ops::run_git_diff_range(&repo, base, head, &query.path) {
                    Ok(diff) => Json(ApiResponse::ok(diff)),
                    Err(error) => Json(ApiResponse::err(error)),
                };
            }
        }
    }

    let (work_dir, _) =
        match resolve_discussion_work_dir(&state, &id, query.workspace_id.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Json(ApiResponse::err(e)),
        };

    let file_path = query.path.clone();
    let committed = query.committed.unwrap_or(false);
    let result = tokio::task::spawn_blocking(move || {
        if committed {
            super::git_ops::run_git_diff_committed(&work_dir, &file_path)
        } else {
            super::git_ops::run_git_diff(&work_dir, &file_path)
        }
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(diff) => Json(ApiResponse::ok(diff)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// POST /api/discussions/:id/git-commit
pub async fn disc_git_commit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<DiscGitCommitRequest>,
) -> Json<ApiResponse<GitCommitResponse>> {
    if req.files.is_empty() {
        return Json(ApiResponse::err("No files specified"));
    }
    if req.message.is_empty() {
        return Json(ApiResponse::err("Commit message is required"));
    }
    for f in &req.files {
        if f.contains("..") {
            return Json(ApiResponse::err(format!("Invalid file path: {}", f)));
        }
    }

    let (work_dir, _) =
        match resolve_discussion_work_dir(&state, &id, req.workspace_id.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Json(ApiResponse::err(e)),
        };

    let files = req.files.clone();
    let message = req.message.clone();
    let amend = req.amend;
    let sign = req.sign;
    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_git_commit(&work_dir, &files, &message, amend, sign)
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(resp) => Json(ApiResponse::ok(resp)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// POST /api/discussions/:id/git-push
pub async fn disc_git_push(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<DiscWorkspaceSelection>,
) -> Json<ApiResponse<GitPushResponse>> {
    let (work_dir, _) =
        match resolve_discussion_work_dir(&state, &id, req.workspace_id.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Json(ApiResponse::err(e)),
        };

    let github_token = resolve_github_token_from_state(&state).await;
    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_git_push(&work_dir, github_token.as_deref())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(resp) => Json(ApiResponse::ok(resp)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// POST /api/discussions/:id/exec
/// POST /api/discussions/:id/worktree-unlock
/// Removes the worktree to free the branch for user checkout/testing.
/// Keeps the branch and all commits intact.
pub async fn worktree_unlock(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<String>> {
    let disc = match state
        .db
        .with_conn({
            let did = id.clone();
            move |conn| crate::db::discussions::get_discussion(conn, &did)
        })
        .await
    {
        Ok(Some(d)) => d,
        Ok(None) => return Json(ApiResponse::err("Discussion not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let wp = match &disc.workspace_path {
        Some(p) if p.contains(".kronn/worktrees") || p.contains(".kronn-worktrees") => p.clone(),
        Some(_) => return Json(ApiResponse::err("Workspace is not a worktree")),
        None => return Json(ApiResponse::err("No worktree to unlock")),
    };

    let pid = match &disc.project_id {
        Some(p) => p.clone(),
        None => return Json(ApiResponse::err("No project associated")),
    };

    let project_path = state
        .db
        .with_conn(move |conn| {
            let p = crate::db::projects::get_project(conn, &pid)?;
            Ok(p.map(|p| p.path).unwrap_or_default())
        })
        .await
        .unwrap_or_default();

    if project_path.is_empty() {
        return Json(ApiResponse::err("Project not found"));
    }

    let resolved = crate::core::scanner::resolve_host_path(&project_path);
    let repo_path = std::path::Path::new(&resolved);

    // Remove worktree but keep the branch
    if let Err(e) = crate::core::worktree::remove_discussion_worktree(repo_path, &wp, false) {
        return Json(ApiResponse::err(format!("Failed to unlock: {}", e)));
    }

    // Clear workspace_path in DB (worktree_branch stays so we can re-lock later)
    let did = disc.id.clone();
    let _ = state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE discussions SET workspace_path = NULL WHERE id = ?1",
                rusqlite::params![did],
            )?;
            Ok(())
        })
        .await;

    let branch = disc.worktree_branch.unwrap_or_default();
    tracing::info!(
        "Unlocked worktree for discussion '{}', branch {} is free",
        disc.title,
        branch
    );
    Json(ApiResponse::ok(format!(
        "Branch {} unlocked — you can now checkout it in your repo",
        branch
    )))
}

/// POST /api/discussions/:id/worktree-lock
/// Re-creates the worktree for the discussion branch.
/// Fails if the branch is still checked out in the main repo.
pub async fn worktree_lock(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<String>> {
    let disc = match state
        .db
        .with_conn({
            let did = id.clone();
            move |conn| crate::db::discussions::get_discussion(conn, &did)
        })
        .await
    {
        Ok(Some(d)) => d,
        Ok(None) => return Json(ApiResponse::err("Discussion not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    if disc.workspace_path.is_some() {
        return Json(ApiResponse::err("Worktree already locked"));
    }

    let branch = match &disc.worktree_branch {
        Some(b) => b.clone(),
        None => {
            return Json(ApiResponse::err(
                "No branch associated with this discussion",
            ))
        }
    };

    let pid = match &disc.project_id {
        Some(p) => p.clone(),
        None => return Json(ApiResponse::err("No project associated")),
    };

    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
        .await
    {
        Ok(Some(p)) => p,
        _ => return Json(ApiResponse::err("Project not found")),
    };

    let resolved = crate::core::scanner::resolve_host_path(&project.path);
    let repo_path = std::path::Path::new(&resolved);

    match crate::core::worktree::reattach_worktree(repo_path, &project.name, &disc.title, &branch) {
        Ok(info) => {
            let did = disc.id.clone();
            let wp = info.path.clone();
            let wb = info.branch.clone();
            let _ = state
                .db
                .with_conn(move |conn| {
                    crate::db::discussions::update_discussion_workspace(conn, &did, &wp, &wb)
                })
                .await;
            tracing::info!(
                "Re-locked worktree for discussion '{}': {}",
                disc.title,
                info.path
            );
            Json(ApiResponse::ok(format!(
                "Worktree re-created at {}",
                info.path
            )))
        }
        Err(e) => Json(ApiResponse::err(format!("Failed to lock: {}", e))),
    }
}

// ── Test mode (user-facing wrapper around unlock/lock + main-repo checkout) ──
//
// The two endpoints below orchestrate the existing `worktree_unlock` /
// `worktree_lock` handlers with a `git checkout` in the main repo, so a
// non-dev user can "try the AI's version in my IDE" and "come back to
// where I was" in two clicks. Preflights:
//   1. worktree dirty  → block (would lose agent's changes if unlocked)
//   2. main repo dirty → require opt-in stash OR block
//   3. detached HEAD   → warn (no block — user confirmed via `force`)
// On error at any step we rollback (re-lock + pop stash) so the user is
// never left in a half-switched state.

#[derive(serde::Deserialize, Default)]
pub struct TestModeEnterRequest {
    /// If the main repo has uncommitted changes, stash them under
    /// `kronn:auto-<disc_id>` so the checkout can proceed. `exit` pops
    /// this stash back. Without this flag (default false) we refuse.
    #[serde(default)]
    pub stash_dirty: bool,
    /// Acknowledge the detached-HEAD warning and proceed anyway. Has no
    /// effect when the repo is on a named branch.
    #[serde(default)]
    pub force: bool,
}

#[derive(serde::Serialize)]
pub struct TestModeEnterResponse {
    pub previous_branch: String,
    pub tested_branch: String,
    pub stashed: bool,
    pub was_detached: bool,
}

/// Envelope wrapping either a successful enter or a structured preflight
/// blocker. Using a dedicated enum (rather than `ApiResponse::err(...)`)
/// lets the UI match on `kind` to show the right modal (commit CTA vs
/// stash-or-cancel dialog) instead of parsing free-form error strings.
#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TestModeEnterResult {
    Ok(TestModeEnterResponse),
    Blocked(TestModeBlocker),
}

#[derive(serde::Serialize)]
pub struct TestModeExitResponse {
    pub restored_branch: String,
    pub unstashed: bool,
    pub worktree_restored: bool,
    /// Non-fatal warning surfaced to the UI when something post-checkout
    /// went sideways (e.g. stash pop conflicted). The exit itself
    /// succeeded — the user is back on `restored_branch`, test_mode
    /// fields are cleared in the DB — but the operator may need to
    /// `git stash list` / `git stash pop` manually. `None` on the
    /// happy path. Pre-fix this travelled as `ApiResponse::err(...)`,
    /// which made the frontend think the entire exit had failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(serde::Serialize)]
pub struct TestModeBlocker {
    /// Machine-readable kind: "WorktreeDirty" | "MainDirty" | "Detached" |
    /// "AlreadyInTestMode" | "NotIsolated" | "NoBranch" | "NoProject".
    /// The UI maps this to the right modal / error toast.
    pub kind: String,
    /// Human-readable explanation, already localized? No — we keep English
    /// here for consistency with other API errors, UI translates via
    /// kind-based keys. This string is the fallback for unknown kinds.
    pub message: String,
    /// Optional per-kind details (dirty file list, current branch name…).
    /// Serialized as raw JSON so each kind can shape its own payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// POST /api/discussions/:id/test-mode/enter
pub async fn test_mode_enter(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<TestModeEnterRequest>,
) -> Json<ApiResponse<TestModeEnterResult>> {
    // Inline shortcut — preflight blockers travel inside `ApiResponse::ok`
    // because the request itself succeeded (we answered with a reason);
    // only infra failures use `ApiResponse::err(...)`. The UI matches on
    // `status: "blocked"` (tag) to show the right modal.
    let blocked = |kind: &str, message: String, details: Option<serde_json::Value>| {
        Json(ApiResponse::ok(TestModeEnterResult::Blocked(
            TestModeBlocker {
                kind: kind.into(),
                message,
                details,
            },
        )))
    };

    let did = id.clone();
    let disc = match state
        .db
        .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &did))
        .await
    {
        Ok(Some(d)) => d,
        Ok(None) => return Json(ApiResponse::err("Discussion not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    if disc.test_mode_restore_branch.is_some() {
        return blocked(
            "AlreadyInTestMode",
            "Already in test mode — call /test-mode/exit first".into(),
            None,
        );
    }

    let branch = match &disc.worktree_branch {
        Some(b) if !b.is_empty() => b.clone(),
        _ => {
            return blocked(
                "NoBranch",
                "Discussion has no worktree branch — switch to Isolated mode first".into(),
                None,
            )
        }
    };

    let pid = match &disc.project_id {
        Some(p) => p.clone(),
        None => return blocked("NoProject", "Discussion has no project".into(), None),
    };

    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
        .await
    {
        Ok(Some(p)) => p,
        _ => return Json(ApiResponse::err("Project not found")),
    };

    let repo_path = crate::core::scanner::resolve_host_path(&project.path);

    // ── Preflight #1: worktree must be clean ────────────────────────────
    if let Some(ref wp) = disc.workspace_path {
        let wt_resolved = crate::core::scanner::resolve_host_path(wp);
        match crate::core::worktree::worktree_dirty_files(&wt_resolved) {
            Ok(files) if !files.is_empty() => {
                let count = files.len();
                return blocked(
                    "WorktreeDirty",
                    format!(
                        "Worktree has {} uncommitted file(s) — commit them first",
                        count
                    ),
                    Some(serde_json::json!({ "files": files })),
                );
            }
            Err(e) => return Json(ApiResponse::err(format!("Failed to check worktree: {}", e))),
            _ => {}
        }
    }

    // ── Preflight #2 + #3: main repo state ──────────────────────────────
    let state_before = match crate::core::worktree::main_repo_state(&repo_path) {
        Ok(s) => s,
        Err(e) => {
            return Json(ApiResponse::err(format!(
                "Failed to check main repo: {}",
                e
            )))
        }
    };

    if state_before.is_detached && !req.force {
        return blocked(
            "Detached",
            "Main repo is in detached HEAD state — pass force=true to proceed".into(),
            None,
        );
    }

    let mut stashed = false;
    let stash_message = format!("kronn:auto-{}", disc.id);
    if !state_before.dirty_files.is_empty() {
        if !req.stash_dirty {
            return blocked(
                "MainDirty",
                format!(
                    "Main repo has {} uncommitted file(s) on `{}` — commit, stash, or re-run with stash_dirty=true",
                    state_before.dirty_files.len(),
                    if state_before.current_branch.is_empty() { "detached" } else { &state_before.current_branch }
                ),
                Some(serde_json::json!({
                    "files": state_before.dirty_files,
                    "current_branch": state_before.current_branch,
                })),
            );
        }
        match crate::core::worktree::stash_push(&repo_path, &stash_message) {
            Ok(true) => {
                stashed = true;
            }
            Ok(false) => {} // no-op, tree cleaned itself between checks
            Err(e) => {
                return Json(ApiResponse::err(format!(
                    "Failed to stash dirty files: {}",
                    e
                )))
            }
        }
    }

    // ── Unlock worktree (must happen BEFORE checkout, so the branch is
    //    free to be checked out in the main repo) ───────────────────────
    if let Some(ref wp) = disc.workspace_path {
        if let Err(e) = crate::core::worktree::remove_discussion_worktree(&repo_path, wp, false) {
            if stashed {
                let _ = crate::core::worktree::stash_pop_by_message(&repo_path, &stash_message);
            }
            return Json(ApiResponse::err(format!(
                "Failed to unlock worktree: {}",
                e
            )));
        }
        let did = disc.id.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "UPDATE discussions SET workspace_path = NULL WHERE id = ?1",
                    rusqlite::params![did],
                )?;
                Ok(())
            })
            .await
        {
            // Worktree is already gone on disk — a stale pointer is
            // recoverable, but say so instead of pretending it's clean.
            tracing::warn!("Test mode: worktree removed but clearing workspace_path failed: {e}");
        }
    }

    // ── Checkout the discussion branch in the main repo ─────────────────
    if let Err(e) = crate::core::worktree::checkout_branch(&repo_path, &branch) {
        // Full rollback: re-create worktree + pop stash.
        let _ = crate::core::worktree::reattach_worktree(
            &repo_path,
            &project.name,
            &disc.title,
            &branch,
        );
        if stashed {
            let _ = crate::core::worktree::stash_pop_by_message(&repo_path, &stash_message);
        }
        return Json(ApiResponse::err(format!(
            "Checkout failed, rolled back: {}",
            e
        )));
    }

    // ── Persist test-mode state in DB ────────────────────────────────────
    // This row is the ONLY record of how to undo the working-tree mutation we
    // just made (previous branch + the stash holding the user's dirty files).
    // If it doesn't persist, exiting test mode can't restore and the stash
    // pointer is lost — so a failure here rolls the checkout back instead of
    // reporting success.
    let previous_branch = state_before.current_branch.clone();
    let restore = previous_branch.clone();
    let stash_ref_clone = if stashed {
        Some(stash_message.clone())
    } else {
        None
    };
    let did = disc.id.clone();
    if let Err(e) = state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::update_discussion_test_mode(
                conn,
                &did,
                Some(&restore),
                stash_ref_clone.as_deref(),
            )
        })
        .await
    {
        let _ = crate::core::worktree::checkout_branch(&repo_path, &previous_branch);
        let _ = crate::core::worktree::reattach_worktree(
            &repo_path,
            &project.name,
            &disc.title,
            &branch,
        );
        if stashed {
            let _ = crate::core::worktree::stash_pop_by_message(&repo_path, &stash_message);
        }
        return Json(ApiResponse::err(format!(
            "Could not persist test-mode restore state ({e}) — checkout rolled back to `{previous_branch}`"
        )));
    }

    tracing::info!(
        "Test mode ON for disc '{}': main repo {} → {} (stashed={})",
        disc.title,
        previous_branch,
        branch,
        stashed
    );

    Json(ApiResponse::ok(TestModeEnterResult::Ok(
        TestModeEnterResponse {
            previous_branch,
            tested_branch: branch,
            stashed,
            was_detached: state_before.is_detached,
        },
    )))
}

/// POST /api/discussions/:id/test-mode/exit
pub async fn test_mode_exit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<TestModeExitResponse>> {
    let did = id.clone();
    let disc = match state
        .db
        .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &did))
        .await
    {
        Ok(Some(d)) => d,
        Ok(None) => return Json(ApiResponse::err("Discussion not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let restore_branch = match &disc.test_mode_restore_branch {
        Some(b) if !b.is_empty() => b.clone(),
        _ => return Json(ApiResponse::err("Not in test mode")),
    };
    let stash_ref = disc.test_mode_stash_ref.clone();

    let pid = match &disc.project_id {
        Some(p) => p.clone(),
        None => return Json(ApiResponse::err("Discussion has no project")),
    };
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
        .await
    {
        Ok(Some(p)) => p,
        _ => return Json(ApiResponse::err("Project not found")),
    };
    let repo_path = crate::core::scanner::resolve_host_path(&project.path);

    // Checkout the user's previous branch. If this fails we stop here —
    // the user needs to resolve whatever conflict is blocking it manually
    // (probably they committed to the branch during the test).
    if let Err(e) = crate::core::worktree::checkout_branch(&repo_path, &restore_branch) {
        return Json(ApiResponse::err(format!(
            "Failed to checkout back to `{}`: {}. Resolve manually, then call /test-mode/exit again.",
            restore_branch, e
        )));
    }

    // Pop the stash if we had one. On conflict we warn but leave the
    // stash intact — the user can pop it manually once they've sorted it.
    let mut unstashed = false;
    let mut stash_warn: Option<String> = None;
    if let Some(ref msg) = stash_ref {
        match crate::core::worktree::stash_pop_by_message(&repo_path, msg) {
            Ok(()) => {
                unstashed = true;
            }
            Err(e) => {
                stash_warn = Some(e);
            }
        }
    }

    // Re-create the worktree so the discussion can keep working.
    let worktree_branch = disc.worktree_branch.clone().unwrap_or_default();
    let mut worktree_restored = false;
    if !worktree_branch.is_empty() {
        match crate::core::worktree::reattach_worktree(
            &repo_path,
            &project.name,
            &disc.title,
            &worktree_branch,
        ) {
            Ok(info) => {
                worktree_restored = true;
                let did = disc.id.clone();
                let wp = info.path.clone();
                let wb = info.branch.clone();
                let _ = state
                    .db
                    .with_conn(move |conn| {
                        crate::db::discussions::update_discussion_workspace(conn, &did, &wp, &wb)
                    })
                    .await;
            }
            Err(e) => {
                tracing::warn!("Failed to restore worktree for '{}': {}", disc.title, e);
            }
        }
    }

    // Clear test-mode tracking fields.
    let did = disc.id.clone();
    let _ = state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::update_discussion_test_mode(conn, &did, None, None)
        })
        .await;

    tracing::info!(
        "Test mode OFF for disc '{}': restored `{}` (unstashed={}, worktree={})",
        disc.title,
        restore_branch,
        unstashed,
        worktree_restored
    );

    // Stash-pop failure is non-fatal here: we already cleared the
    // test_mode fields in DB and switched the working tree back to
    // `restore_branch`. Returning `Err` would make the UI think the
    // exit failed, when really the user is back on their branch and
    // just needs to recover the stash manually. Surface the issue as
    // a `warning` field on the success envelope instead.
    let warning = stash_warn.map(|w| {
        format!(
            "Stash pop failed: {}. Your work is safe — run `git stash list` to find it.",
            w
        )
    });

    Json(ApiResponse::ok(TestModeExitResponse {
        restored_branch: restore_branch,
        unstashed,
        worktree_restored,
        warning,
    }))
}

pub async fn disc_exec(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<DiscExecRequest>,
) -> Json<ApiResponse<ExecResponse>> {
    let cmd = req.command.trim().to_string();
    if cmd.is_empty() {
        return Json(ApiResponse::err("Empty command"));
    }

    // Require full_access on at least one agent (only enforced when agents are installed)
    {
        let config = state.config.read().await;
        if config.agents.any_installed() && !config.agents.any_full_access() {
            return Json(ApiResponse::err(
                "Terminal requires full_access enabled on at least one agent",
            ));
        }
    }

    // Validate command against strict allowlist
    if let Err(msg) = super::git_ops::validate_exec_command(&cmd) {
        return Json(ApiResponse::err(msg));
    }

    let (work_dir, _) =
        match resolve_discussion_work_dir(&state, &id, req.workspace_id.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Json(ApiResponse::err(e)),
        };

    // Rate-limit concurrent exec calls via the shared agent semaphore
    let _permit = match state.agent_semaphore.acquire().await {
        Ok(p) => p,
        Err(_) => return Json(ApiResponse::err("Server is shutting down")),
    };

    let result = tokio::task::spawn_blocking(move || super::git_ops::run_exec(&work_dir, &cmd))
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(resp) => Json(ApiResponse::ok(resp)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// POST /api/discussions/:id/git-pr
pub async fn disc_create_pr(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<DiscCreatePrRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let (work_dir, _) =
        match resolve_discussion_work_dir(&state, &id, req.workspace_id.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Json(ApiResponse::err(e)),
        };

    let title = req.title;
    let body = req.body;
    let base = req.base;
    let github_token = resolve_github_token_from_state(&state).await;
    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_create_pr(&work_dir, &title, &body, &base, github_token.as_deref())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(url) => Json(ApiResponse::ok(serde_json::json!({ "url": url }))),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// GET /api/discussions/:id/pr-template
pub async fn disc_pr_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DiscWorkspaceSelection>,
) -> Json<ApiResponse<serde_json::Value>> {
    let (work_dir, _) =
        match resolve_discussion_work_dir(&state, &id, query.workspace_id.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Json(ApiResponse::err(e)),
        };

    let branch = sync_cmd("git")
        .args(["branch", "--show-current"])
        .current_dir(&work_dir)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let template = super::git_ops::read_pr_template(&work_dir)
        .unwrap_or_else(|| super::git_ops::default_pr_template(&branch));

    let source = if super::git_ops::read_pr_template(&work_dir).is_some() {
        "project"
    } else {
        "kronn"
    };

    Json(ApiResponse::ok(serde_json::json!({
        "template": template,
        "source": source,
    })))
}

/// Build MCP context from global MCP configs for general discussions (no project).
/// Lists the server names so the agent knows which MCP tools are available.
async fn build_global_mcp_context(state: &AppState) -> Option<String> {
    let configs = state
        .db
        .with_conn(crate::db::mcps::list_configs)
        .await
        .ok()?;

    let global_configs: Vec<_> = configs.into_iter().filter(|c| c.include_general).collect();
    if global_configs.is_empty() {
        return None;
    }

    let servers = state
        .db
        .with_conn(crate::db::mcps::list_servers)
        .await
        .unwrap_or_default();
    let server_map: std::collections::HashMap<String, String> = servers
        .into_iter()
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();

    let mut result = String::from("## MCP Servers available\n\n");
    result.push_str("You have access to the following MCP servers (global). ");
    result.push_str(
        "Use their tools (prefixed `mcp__<server>__<tool>`) instead of Bash workarounds.\n\n",
    );
    result.push_str("Available servers:\n");
    for cfg in &global_configs {
        let name = server_map
            .get(&cfg.server_id)
            .cloned()
            .unwrap_or_else(|| cfg.label.clone());
        result.push_str(&format!("- **{}** ({})\n", cfg.label, name));
    }
    result.push('\n');
    let preference_plugins = state
        .db
        .with_conn(|conn| crate::core::mcp_scanner::collect_active_plugin_preferences(conn, None))
        .await
        .unwrap_or_default();
    result.push_str(
        &crate::core::mcp_scanner::build_plugin_invocation_preferences(&preference_plugins),
    );

    Some(result)
}

/// Build global MCP context AND write .mcp.json for general (no-project) discussions.
pub(crate) async fn prepare_general_mcp(
    state: &AppState,
    workspace_path: &Option<String>,
) -> Option<String> {
    let work_dir = workspace_path
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().to_string());
    {
        let db = state.db.clone();
        let cfg = state.config.read().await;
        if let Some(ref secret) = cfg.encryption_secret {
            let secret = secret.clone();
            let wd = work_dir;
            let _ = db
                .with_conn(move |conn| {
                    let _ = crate::core::mcp_scanner::write_general_mcp_json(conn, &secret, &wd);
                    Ok(())
                })
                .await;
        }
    }
    build_global_mcp_context(state).await
}

/// Format a rich log line from tool name + accumulated JSON input
pub(crate) fn format_tool_log(tool: &str, input_json: &str) -> String {
    // Try to parse the accumulated JSON and extract the most relevant field
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(input_json) {
        match tool {
            "Read" => {
                if let Some(path) = val.get("file_path").and_then(|v| v.as_str()) {
                    return format!("Read {}", path);
                }
            }
            "Bash" => {
                if let Some(cmd) = val.get("command").and_then(|v| v.as_str()) {
                    // Truncate by char count, not byte count. `&s[..80]`
                    // would panic if byte 80 falls in the middle of a UTF-8
                    // sequence (very real on French "été", emoji, accented
                    // package names like `pré-prod`).
                    let short: String = cmd.chars().take(80).collect();
                    return format!("$ {}", short.replace('\n', " "));
                }
            }
            "Edit" => {
                if let Some(path) = val.get("file_path").and_then(|v| v.as_str()) {
                    return format!("Edit {}", path);
                }
            }
            "Write" => {
                if let Some(path) = val.get("file_path").and_then(|v| v.as_str()) {
                    return format!("Write {}", path);
                }
            }
            "Grep" => {
                if let Some(pattern) = val.get("pattern").and_then(|v| v.as_str()) {
                    let path = val.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                    return format!("Grep '{}' in {}", pattern, path);
                }
            }
            "Glob" => {
                if let Some(pattern) = val.get("pattern").and_then(|v| v.as_str()) {
                    return format!("Glob {}", pattern);
                }
            }
            "WebFetch" => {
                if let Some(url) = val.get("url").and_then(|v| v.as_str()) {
                    return format!("Fetch {}", url);
                }
            }
            "Agent" => {
                if let Some(desc) = val.get("description").and_then(|v| v.as_str()) {
                    return format!("Agent: {}", desc);
                }
            }
            _ => {
                // MCP tools: mcp__server__tool
                if tool.starts_with("mcp__") {
                    let parts: Vec<&str> = tool.splitn(3, "__").collect();
                    if parts.len() == 3 {
                        return format!("MCP {}/{}", parts[1], parts[2]);
                    }
                }
            }
        }
    }
    // Fallback: just the tool name
    format!("Tool: {}", tool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::default_config;
    use crate::db::Database;
    use crate::DEFAULT_MAX_CONCURRENT_AGENTS;
    use std::process::Command;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn scoped_disc_commit_repo() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?} failed");
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Kronn Test"]);
        git(&["config", "user.email", "kronn@example.test"]);
        std::fs::write(repo.path().join("a.txt"), "base a\n").unwrap();
        std::fs::write(repo.path().join("b.txt"), "base b\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "initial"]);
        std::fs::write(repo.path().join("a.txt"), "changed a\n").unwrap();
        std::fs::write(repo.path().join("b.txt"), "changed b\n").unwrap();
        git(&["add", "b.txt"]);
        repo
    }

    fn disc_git_names(repo: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn discussion_commit_endpoint_commits_only_requested_paths() {
        let repo = scoped_disc_commit_repo();
        let state = AppState::new_defaults(
            Arc::new(RwLock::new(default_config())),
            Arc::new(Database::open_in_memory().expect("in-memory DB")),
            DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let path = repo.path().to_string_lossy().to_string();
        state
            .db
            .with_conn(move |connection| {
                connection.execute(
                    "INSERT INTO projects
                     (id, name, path, ai_config_json, created_at, updated_at)
                     VALUES ('project-disc-commit', 'Project', ?1, '{}', 'now', 'now')",
                    [&path],
                )?;
                connection.execute(
                    "INSERT INTO discussions
                     (id, project_id, title, agent, language, created_at, updated_at)
                     VALUES ('disc-scoped-commit', 'project-disc-commit', 'Disc',
                             'Codex', 'en', 'now', 'now')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let response = disc_git_commit(
            State(state),
            Path("disc-scoped-commit".into()),
            Json(DiscGitCommitRequest {
                files: vec!["a.txt".into()],
                message: "test: scoped discussion commit".into(),
                amend: false,
                sign: false,
                workspace_id: None,
            }),
        )
        .await
        .0;
        assert!(response.success, "discussion commit endpoint must succeed");
        assert_eq!(
            disc_git_names(repo.path(), &["diff", "HEAD^", "HEAD", "--name-only"]),
            "a.txt"
        );
        assert_eq!(
            disc_git_names(repo.path(), &["diff", "--cached", "--name-only"]),
            "b.txt",
            "an unrelated staged file must remain staged"
        );
    }

    #[tokio::test]
    async fn detached_child_workspace_keeps_commits_files_and_diff_visible_from_parent() {
        let repo = scoped_disc_commit_repo();
        // Start from a clean base, then create the exact one-commit range whose
        // checkout will be considered physically cleaned.
        Command::new("git")
            .args(["reset", "--hard", "HEAD"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let base = disc_git_names(repo.path(), &["rev-parse", "HEAD"]);
        std::fs::write(repo.path().join("agent.txt"), "durable evidence\n").unwrap();
        Command::new("git")
            .args(["add", "agent.txt"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "agent: durable change"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let head = disc_git_names(repo.path(), &["rev-parse", "HEAD"]);

        let state = AppState::new_defaults(
            Arc::new(RwLock::new(default_config())),
            Arc::new(Database::open_in_memory().expect("in-memory DB")),
            DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let path = repo.path().to_string_lossy().to_string();
        let base_db = base.clone();
        let head_db = head.clone();
        state
            .db
            .with_conn(move |connection| {
                connection.execute(
                    "INSERT INTO projects
                     (id, name, path, ai_config_json, created_at, updated_at)
                     VALUES ('p-code-history', 'Project', ?1, '{}', 'now', 'now')",
                    [&path],
                )?;
                for (id, title) in [("d-parent-code", "Parent"), ("d-child-code", "Child")] {
                    connection.execute(
                        "INSERT INTO discussions
                         (id, project_id, title, agent, language, created_at, updated_at)
                         VALUES (?1, 'p-code-history', ?2, 'Codex', 'en', 'now', 'now')",
                        rusqlite::params![id, title],
                    )?;
                }
                connection.execute(
                    "INSERT INTO discussion_workspaces
                     (id, disc_id, project_id, workspace_path, canonical_path,
                      branch, head_sha, ownership, state, created_at, updated_at,
                      parent_discussion_id, base_sha)
                     VALUES ('ws-cleaned', 'd-child-code', 'p-code-history', '/gone/worktree', NULL,
                             'kronn/task/KT-451-test', ?1, 'managed', 'detached', 'now', 'now',
                             'd-parent-code', ?2)",
                    rusqlite::params![head_db, base_db],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let response = disc_git_status(
            State(state.clone()),
            Path("d-parent-code".into()),
            Query(DiscWorkspaceSelection {
                workspace_id: Some("ws-cleaned".into()),
                ..DiscWorkspaceSelection::default()
            }),
        )
        .await
        .0;
        assert!(response.success, "historical status must succeed");
        let status = response.data.expect("status payload");
        assert_eq!(status.workspace.as_ref().unwrap().state, "detached");
        assert_eq!(
            status.workspace.as_ref().unwrap().head_sha.as_deref(),
            Some(head.as_str())
        );
        assert_eq!(status.commits.len(), 1);
        assert_eq!(status.commits[0].subject, "agent: durable change");
        assert_eq!(status.committed_files.len(), 1);
        assert_eq!(status.committed_files[0].path, "agent.txt");

        let diff = disc_git_diff(
            State(state),
            Path("d-parent-code".into()),
            Query(DiscGitDiffQuery {
                path: "agent.txt".into(),
                committed: Some(true),
                workspace_id: Some("ws-cleaned".into()),
            }),
        )
        .await
        .0;
        assert!(diff.success, "historical diff must succeed");
        assert!(diff.data.unwrap().diff.contains("durable evidence"));
    }

    #[test]
    fn format_tool_log_read() {
        let out = format_tool_log("Read", r#"{"file_path":"src/lib.rs"}"#);
        assert_eq!(out, "Read src/lib.rs");
    }

    #[test]
    fn format_tool_log_bash_short() {
        let out = format_tool_log("Bash", r#"{"command":"ls -la"}"#);
        assert_eq!(out, "$ ls -la");
    }

    #[test]
    fn format_tool_log_bash_truncates_at_80_chars() {
        // 100 ASCII chars → truncated to 80
        let cmd = "x".repeat(100);
        let json = format!(r#"{{"command":"{}"}}"#, cmd);
        let out = format_tool_log("Bash", &json);
        // "$ " prefix + 80 chars = 82 chars total
        assert_eq!(out.len(), 82);
    }

    #[test]
    fn format_tool_log_bash_replaces_newlines() {
        let out = format_tool_log("Bash", r#"{"command":"echo a\nb"}"#);
        assert_eq!(out, "$ echo a b");
    }

    #[test]
    fn format_tool_log_bash_does_not_panic_on_utf8_at_byte_80() {
        // Regression: pre-fix `&cmd[..80]` panicked when byte 80 fell in
        // the middle of a UTF-8 sequence. Build a string where byte 80 is
        // mid-sequence: 79 ASCII chars + an emoji (4 bytes) → byte 80 is
        // inside the emoji. Char-based truncation keeps the emoji whole.
        let cmd = format!("{}{}", "a".repeat(79), "😀");
        let json = format!(r#"{{"command":"{}"}}"#, cmd);
        let out = format_tool_log("Bash", &json);
        // Should not panic. Output keeps the 79 a's + the emoji = 80 chars.
        assert!(out.starts_with("$ "));
        assert!(out.contains("😀"));
    }

    #[test]
    fn format_tool_log_bash_truncates_french_chars_safely() {
        // French accented characters are 2 bytes each in UTF-8. A long
        // French-only string has byte length > char length and would
        // historically slip past the byte-80 panic depending on alignment.
        let cmd = "été ".repeat(40); // 160 chars, ~280 bytes
        let json = format!(r#"{{"command":"{}"}}"#, cmd);
        let out = format_tool_log("Bash", &json);
        // 80 chars + "$ " prefix; assert no panic and length is correct.
        assert!(out.starts_with("$ "));
        assert_eq!(out.chars().count(), 82);
    }

    #[test]
    fn format_tool_log_edit_write() {
        assert_eq!(
            format_tool_log("Edit", r#"{"file_path":"a.rs"}"#),
            "Edit a.rs"
        );
        assert_eq!(
            format_tool_log("Write", r#"{"file_path":"b.rs"}"#),
            "Write b.rs"
        );
    }

    #[test]
    fn format_tool_log_grep_with_default_path() {
        let out = format_tool_log("Grep", r#"{"pattern":"foo"}"#);
        assert_eq!(out, "Grep 'foo' in .");
    }

    #[test]
    fn format_tool_log_grep_with_path() {
        let out = format_tool_log("Grep", r#"{"pattern":"foo","path":"src/"}"#);
        assert_eq!(out, "Grep 'foo' in src/");
    }

    #[test]
    fn format_tool_log_mcp_tool_format() {
        let out = format_tool_log("mcp__github__create_pull_request", r#"{}"#);
        assert_eq!(out, "MCP github/create_pull_request");
    }

    #[test]
    fn format_tool_log_unknown_tool_falls_back() {
        let out = format_tool_log("Unknown", r#"{}"#);
        assert_eq!(out, "Tool: Unknown");
    }

    #[test]
    fn format_tool_log_invalid_json_falls_back() {
        // Non-JSON input shouldn't panic — falls through to the default.
        let out = format_tool_log("Bash", "not json");
        assert_eq!(out, "Tool: Bash");
    }
}
