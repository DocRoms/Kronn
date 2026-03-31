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

