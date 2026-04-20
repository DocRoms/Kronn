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

async fn resolve_discussion_work_dir(state: &AppState, discussion_id: &str) -> Result<(std::path::PathBuf, String), String> {
    let did = discussion_id.to_string();
    let disc = state.db.with_conn(move |conn| crate::db::discussions::get_discussion(conn, &did))
        .await
        .map_err(|e| format!("DB error: {}", e))?;
    let disc = disc.ok_or_else(|| "Discussion not found".to_string())?;

    let project_id = disc.project_id.ok_or_else(|| "Discussion has no project".to_string())?;

    let pid = project_id.clone();
    let project = state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
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

/// GET /api/discussions/:id/git-status
pub async fn disc_git_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<GitStatusResponse>> {
    let (work_dir, _) = match resolve_discussion_work_dir(&state, &id).await {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_git_status(&work_dir)
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(status) => Json(ApiResponse::ok(status)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// GET /api/discussions/:id/git-diff?path=...
pub async fn disc_git_diff(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<GitDiffQuery>,
) -> Json<ApiResponse<GitDiffResponse>> {
    if query.path.contains("..") {
        return Json(ApiResponse::err("Invalid path"));
    }

    let (work_dir, _) = match resolve_discussion_work_dir(&state, &id).await {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let file_path = query.path.clone();
    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_git_diff(&work_dir, &file_path)
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(diff) => Json(ApiResponse::ok(diff)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// POST /api/discussions/:id/git-commit
pub async fn disc_git_commit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<GitCommitRequest>,
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

    let (work_dir, _) = match resolve_discussion_work_dir(&state, &id).await {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let files = req.files.clone();
    let message = req.message.clone();
    let amend = req.amend;
    let sign = req.sign;
    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_git_commit(&work_dir, &files, &message, amend, sign)
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(resp) => Json(ApiResponse::ok(resp)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// POST /api/discussions/:id/git-push
pub async fn disc_git_push(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<GitPushResponse>> {
    let (work_dir, _) = match resolve_discussion_work_dir(&state, &id).await {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let github_token = resolve_github_token_from_state(&state).await;
    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_git_push(&work_dir, github_token.as_deref())
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

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
    let disc = match state.db.with_conn({
        let did = id.clone();
        move |conn| crate::db::discussions::get_discussion(conn, &did)
    }).await {
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

    let project_path = state.db.with_conn(move |conn| {
        let p = crate::db::projects::get_project(conn, &pid)?;
        Ok(p.map(|p| p.path).unwrap_or_default())
    }).await.unwrap_or_default();

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
    let _ = state.db.with_conn(move |conn| {
        conn.execute(
            "UPDATE discussions SET workspace_path = NULL WHERE id = ?1",
            rusqlite::params![did],
        )?;
        Ok(())
    }).await;

    let branch = disc.worktree_branch.unwrap_or_default();
    tracing::info!("Unlocked worktree for discussion '{}', branch {} is free", disc.title, branch);
    Json(ApiResponse::ok(format!("Branch {} unlocked — you can now checkout it in your repo", branch)))
}

/// POST /api/discussions/:id/worktree-lock
/// Re-creates the worktree for the discussion branch.
/// Fails if the branch is still checked out in the main repo.
pub async fn worktree_lock(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<String>> {
    let disc = match state.db.with_conn({
        let did = id.clone();
        move |conn| crate::db::discussions::get_discussion(conn, &did)
    }).await {
        Ok(Some(d)) => d,
        Ok(None) => return Json(ApiResponse::err("Discussion not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    if disc.workspace_path.is_some() {
        return Json(ApiResponse::err("Worktree already locked"));
    }

    let branch = match &disc.worktree_branch {
        Some(b) => b.clone(),
        None => return Json(ApiResponse::err("No branch associated with this discussion")),
    };

    let pid = match &disc.project_id {
        Some(p) => p.clone(),
        None => return Json(ApiResponse::err("No project associated")),
    };

    let project = match state.db.with_conn(move |conn| {
        crate::db::projects::get_project(conn, &pid)
    }).await {
        Ok(Some(p)) => p,
        _ => return Json(ApiResponse::err("Project not found")),
    };

    let resolved = crate::core::scanner::resolve_host_path(&project.path);
    let repo_path = std::path::Path::new(&resolved);

    match crate::core::worktree::reattach_worktree(
        repo_path, &project.name, &disc.title, &branch,
    ) {
        Ok(info) => {
            let did = disc.id.clone();
            let wp = info.path.clone();
            let wb = info.branch.clone();
            let _ = state.db.with_conn(move |conn| {
                crate::db::discussions::update_discussion_workspace(conn, &did, &wp, &wb)
            }).await;
            tracing::info!("Re-locked worktree for discussion '{}': {}", disc.title, info.path);
            Json(ApiResponse::ok(format!("Worktree re-created at {}", info.path)))
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
        Json(ApiResponse::ok(TestModeEnterResult::Blocked(TestModeBlocker {
            kind: kind.into(), message, details,
        })))
    };

    let did = id.clone();
    let disc = match state.db.with_conn(move |conn| crate::db::discussions::get_discussion(conn, &did)).await {
        Ok(Some(d)) => d,
        Ok(None) => return Json(ApiResponse::err("Discussion not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    if disc.test_mode_restore_branch.is_some() {
        return blocked("AlreadyInTestMode", "Already in test mode — call /test-mode/exit first".into(), None);
    }

    let branch = match &disc.worktree_branch {
        Some(b) if !b.is_empty() => b.clone(),
        _ => return blocked("NoBranch", "Discussion has no worktree branch — switch to Isolated mode first".into(), None),
    };

    let pid = match &disc.project_id {
        Some(p) => p.clone(),
        None => return blocked("NoProject", "Discussion has no project".into(), None),
    };

    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &pid)).await {
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
                    format!("Worktree has {} uncommitted file(s) — commit them first", count),
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
        Err(e) => return Json(ApiResponse::err(format!("Failed to check main repo: {}", e))),
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
            Ok(true) => { stashed = true; }
            Ok(false) => {} // no-op, tree cleaned itself between checks
            Err(e) => return Json(ApiResponse::err(format!("Failed to stash dirty files: {}", e))),
        }
    }

    // ── Unlock worktree (must happen BEFORE checkout, so the branch is
    //    free to be checked out in the main repo) ───────────────────────
    if let Some(ref wp) = disc.workspace_path {
        if let Err(e) = crate::core::worktree::remove_discussion_worktree(&repo_path, wp, false) {
            if stashed {
                let _ = crate::core::worktree::stash_pop_by_message(&repo_path, &stash_message);
            }
            return Json(ApiResponse::err(format!("Failed to unlock worktree: {}", e)));
        }
        let did = disc.id.clone();
        let _ = state.db.with_conn(move |conn| {
            conn.execute(
                "UPDATE discussions SET workspace_path = NULL WHERE id = ?1",
                rusqlite::params![did],
            )?;
            Ok(())
        }).await;
    }

    // ── Checkout the discussion branch in the main repo ─────────────────
    if let Err(e) = crate::core::worktree::checkout_branch(&repo_path, &branch) {
        // Full rollback: re-create worktree + pop stash.
        let _ = crate::core::worktree::reattach_worktree(&repo_path, &project.name, &disc.title, &branch);
        if stashed {
            let _ = crate::core::worktree::stash_pop_by_message(&repo_path, &stash_message);
        }
        return Json(ApiResponse::err(format!("Checkout failed, rolled back: {}", e)));
    }

    // ── Persist test-mode state in DB ────────────────────────────────────
    let previous_branch = state_before.current_branch.clone();
    let restore = previous_branch.clone();
    let stash_ref_clone = if stashed { Some(stash_message.clone()) } else { None };
    let did = disc.id.clone();
    let _ = state.db.with_conn(move |conn| {
        crate::db::discussions::update_discussion_test_mode(
            conn, &did, Some(&restore), stash_ref_clone.as_deref(),
        )
    }).await;

    tracing::info!(
        "Test mode ON for disc '{}': main repo {} → {} (stashed={})",
        disc.title, previous_branch, branch, stashed
    );

    Json(ApiResponse::ok(TestModeEnterResult::Ok(TestModeEnterResponse {
        previous_branch,
        tested_branch: branch,
        stashed,
        was_detached: state_before.is_detached,
    })))
}

/// POST /api/discussions/:id/test-mode/exit
pub async fn test_mode_exit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<TestModeExitResponse>> {
    let did = id.clone();
    let disc = match state.db.with_conn(move |conn| crate::db::discussions::get_discussion(conn, &did)).await {
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
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &pid)).await {
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
            Ok(()) => { unstashed = true; }
            Err(e) => { stash_warn = Some(e); }
        }
    }

    // Re-create the worktree so the discussion can keep working.
    let worktree_branch = disc.worktree_branch.clone().unwrap_or_default();
    let mut worktree_restored = false;
    if !worktree_branch.is_empty() {
        match crate::core::worktree::reattach_worktree(&repo_path, &project.name, &disc.title, &worktree_branch) {
            Ok(info) => {
                worktree_restored = true;
                let did = disc.id.clone();
                let wp = info.path.clone();
                let wb = info.branch.clone();
                let _ = state.db.with_conn(move |conn| {
                    crate::db::discussions::update_discussion_workspace(conn, &did, &wp, &wb)
                }).await;
            }
            Err(e) => {
                tracing::warn!("Failed to restore worktree for '{}': {}", disc.title, e);
            }
        }
    }

    // Clear test-mode tracking fields.
    let did = disc.id.clone();
    let _ = state.db.with_conn(move |conn| {
        crate::db::discussions::update_discussion_test_mode(conn, &did, None, None)
    }).await;

    tracing::info!(
        "Test mode OFF for disc '{}': restored `{}` (unstashed={}, worktree={})",
        disc.title, restore_branch, unstashed, worktree_restored
    );

    if let Some(warn) = stash_warn {
        return Json(ApiResponse::err(format!(
            "Exited test mode (back on `{}`) but stash pop failed: {}. Your work is safe — run `git stash list` to find it.",
            restore_branch, warn
        )));
    }

    Json(ApiResponse::ok(TestModeExitResponse {
        restored_branch: restore_branch,
        unstashed,
        worktree_restored,
    }))
}

pub async fn disc_exec(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ExecRequest>,
) -> Json<ApiResponse<ExecResponse>> {
    let cmd = req.command.trim().to_string();
    if cmd.is_empty() {
        return Json(ApiResponse::err("Empty command"));
    }

    // Require full_access on at least one agent (only enforced when agents are installed)
    {
        let config = state.config.read().await;
        if config.agents.any_installed() && !config.agents.any_full_access() {
            return Json(ApiResponse::err("Terminal requires full_access enabled on at least one agent"));
        }
    }

    // Validate command against strict allowlist
    if let Err(msg) = super::git_ops::validate_exec_command(&cmd) {
        return Json(ApiResponse::err(msg));
    }

    let (work_dir, _) = match resolve_discussion_work_dir(&state, &id).await {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    // Rate-limit concurrent exec calls via the shared agent semaphore
    let _permit = match state.agent_semaphore.acquire().await {
        Ok(p) => p,
        Err(_) => return Json(ApiResponse::err("Server is shutting down")),
    };

    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_exec(&work_dir, &cmd)
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(resp) => Json(ApiResponse::ok(resp)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// POST /api/discussions/:id/git-pr
pub async fn disc_create_pr(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreatePrRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let (work_dir, _) = match resolve_discussion_work_dir(&state, &id).await {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let title = req.title;
    let body = req.body;
    let base = req.base;
    let github_token = resolve_github_token_from_state(&state).await;
    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_create_pr(&work_dir, &title, &body, &base, github_token.as_deref())
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(url) => Json(ApiResponse::ok(serde_json::json!({ "url": url }))),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// GET /api/discussions/:id/pr-template
pub async fn disc_pr_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<serde_json::Value>> {
    let (work_dir, _) = match resolve_discussion_work_dir(&state, &id).await {
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
    let configs = state.db.with_conn(|conn| {
        crate::db::mcps::list_configs(conn)
    }).await.ok()?;

    let global_configs: Vec<_> = configs.into_iter().filter(|c| c.include_general).collect();
    if global_configs.is_empty() {
        return None;
    }

    let servers = state.db.with_conn(|conn| {
        crate::db::mcps::list_servers(conn)
    }).await.unwrap_or_default();
    let server_map: std::collections::HashMap<String, String> = servers.into_iter()
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();

    let mut result = String::from("## MCP Servers available\n\n");
    result.push_str("You have access to the following MCP servers (global). ");
    result.push_str("Use their tools (prefixed `mcp__<server>__<tool>`) instead of Bash workarounds.\n\n");
    result.push_str("Available servers:\n");
    for cfg in &global_configs {
        let name = server_map.get(&cfg.server_id)
            .cloned()
            .unwrap_or_else(|| cfg.label.clone());
        result.push_str(&format!("- **{}** ({})\n", cfg.label, name));
    }
    result.push('\n');

    Some(result)
}

/// Build global MCP context AND write .mcp.json for general (no-project) discussions.
pub(crate) async fn prepare_general_mcp(state: &AppState, workspace_path: &Option<String>) -> Option<String> {
    let work_dir = workspace_path.clone()
        .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().to_string());
    {
        let db = state.db.clone();
        let cfg = state.config.read().await;
        if let Some(ref secret) = cfg.encryption_secret {
            let secret = secret.clone();
            let wd = work_dir;
            let _ = db.with_conn(move |conn| {
                let _ = crate::core::mcp_scanner::write_general_mcp_json(conn, &secret, &wd);
                Ok(())
            }).await;
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
                    let short = if cmd.len() > 80 { &cmd[..80] } else { cmd };
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

