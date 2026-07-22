// Git-flavoured endpoints under `/api/projects/:id/...`: status, diff,
// branch, commit, push, exec, create-pr, pr-template. The actual git
// invocations live in `api::git_ops`; this module is the HTTP-facing
// glue (path resolution, request validation, blocking pool dispatch).

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use std::time::{Duration, Instant};

use crate::core::cmd::sync_cmd;
use crate::core::scanner;
use crate::models::*;
use crate::AppState;

const LANGUAGE_CACHE_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Default, Deserialize)]
pub struct GitStatusQuery {
    /// Recompute source-language statistics while keeping Git status live.
    #[serde(default)]
    pub refresh: bool,
}

/// Resolve GitHub token from MCP configs for git operations (push, PR creation).
async fn resolve_github_token_from_state(state: &AppState) -> Option<String> {
    let cfg = state.config.read().await;
    let secret = cfg.encryption_secret.clone()?;
    drop(cfg);
    let db = state.db.clone();
    db.with_conn(move |conn| Ok(crate::api::git_ops::resolve_github_token(conn, &secret)))
        .await
        .ok()
        .flatten()
}

/// Helper: resolve a project's filesystem path from its DB id.
async fn resolve_project_path(state: &AppState, id: &str) -> Result<std::path::PathBuf, String> {
    let pid = id.to_string();
    let project = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
        .await
        .map_err(|e| format!("DB error: {}", e))?;
    let project = project.ok_or_else(|| "Project not found".to_string())?;
    let resolved = scanner::resolve_host_path(&project.path);
    if !resolved.exists() {
        return Err(format!("Project path not found: {}", resolved.display()));
    }
    Ok(resolved)
}

/// GET /api/projects/:id/git-status
pub async fn git_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<GitStatusQuery>,
) -> Json<ApiResponse<GitStatusResponse>> {
    let id_for_read = id.clone();
    let project_and_exclusions = match state
        .db
        .with_read_conn(move |conn| {
            let project = crate::db::projects::get_project(conn, &id_for_read)?;
            let exclusions = crate::db::projects::get_source_exclusions(conn, &id_for_read)?;
            Ok((project, exclusions))
        })
        .await
    {
        Ok((Some(project), exclusions)) => (project, exclusions),
        Ok((None, _)) => return Json(ApiResponse::err("Project not found")),
        Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
    };
    let (project, exclusions) = project_and_exclusions;
    let repo_path = scanner::resolve_host_path(&project.path);
    if !repo_path.exists() {
        return Json(ApiResponse::err(format!(
            "Project path not found: {}",
            repo_path.display()
        )));
    }

    let cached_languages = if query.refresh {
        None
    } else {
        state
            .git_language_cache
            .lock()
            .await
            .get(&id)
            .filter(|cached| {
                cached.inserted_at.elapsed() < LANGUAGE_CACHE_TTL && cached.exclusions == exclusions
            })
            .map(|cached| (cached.checked_at, cached.languages.clone()))
    };
    let exclusions_for_cache = exclusions.clone();
    let id_for_cache = id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut status = crate::api::git_ops::run_git_status(&repo_path)?;
        if let Some((checked_at, languages)) = cached_languages {
            status.languages = languages;
            status.languages_checked_at = Some(checked_at);
            status.languages_cached = true;
        } else {
            status.languages =
                crate::api::ai_docs::compute_source_language_stats(&repo_path, &exclusions);
            status.languages_checked_at = Some(chrono::Utc::now());
        }
        Ok::<_, String>(status)
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(status) => {
            if !status.languages_cached {
                if let Some(checked_at) = status.languages_checked_at {
                    state.git_language_cache.lock().await.insert(
                        id_for_cache,
                        CachedProjectLanguages {
                            inserted_at: Instant::now(),
                            checked_at,
                            exclusions: exclusions_for_cache,
                            languages: status.languages.clone(),
                        },
                    );
                }
            }
            Json(ApiResponse::ok(status))
        }
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// GET /api/projects/:id/git-diff?path=src/foo.rs
pub async fn git_diff(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<GitDiffQuery>,
) -> Json<ApiResponse<GitDiffResponse>> {
    // Path traversal protection
    if query.path.contains("..") {
        return Json(ApiResponse::err("Invalid path"));
    }

    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(p) => p,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let file_path = query.path.clone();
    let committed = query.committed.unwrap_or(false);
    let result = tokio::task::spawn_blocking(move || {
        if committed {
            crate::api::git_ops::run_git_diff_committed(&repo_path, &file_path)
        } else {
            crate::api::git_ops::run_git_diff(&repo_path, &file_path)
        }
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(diff) => Json(ApiResponse::ok(diff)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// GET /api/projects/:id/git-blame?path=src/foo.rs
pub async fn git_blame(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<GitBlameQuery>,
) -> Json<ApiResponse<GitBlameResponse>> {
    let relative = std::path::Path::new(&query.path);
    if query.path.is_empty()
        || query.path.starts_with('/')
        || query.path.starts_with('\\')
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Json(ApiResponse::err("Invalid source path"));
    }

    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(path) => path,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let file_path = query.path;
    let result = tokio::task::spawn_blocking(move || {
        crate::api::git_ops::run_git_blame(&repo_path, &file_path)
    })
    .await
    .unwrap_or_else(|error| Err(format!("Task failed: {error}")));

    match result {
        Ok(blame) => Json(ApiResponse::ok(blame)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

/// POST /api/projects/:id/git-branch
pub async fn git_branch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<GitBranchRequest>,
) -> Json<ApiResponse<GitBranchResponse>> {
    // Validate branch name (no spaces, no special chars)
    if req.name.is_empty() || req.name.contains(' ') || req.name.contains("..") {
        return Json(ApiResponse::err("Invalid branch name"));
    }

    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(p) => p,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let branch_name = req.name.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<GitBranchResponse, String> {
        let output = sync_cmd("git")
            .args(["checkout", "-b", &branch_name])
            .current_dir(&repo_path)
            .output()
            .map_err(|e| format!("Failed to run git: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git checkout -b failed: {}", stderr.trim()));
        }

        Ok(GitBranchResponse {
            branch: branch_name,
        })
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(resp) => Json(ApiResponse::ok(resp)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// POST /api/projects/:id/git-commit
pub async fn git_commit(
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

    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(p) => p,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let files = req.files.clone();
    let message = req.message.clone();
    let amend = req.amend;
    let sign = req.sign;
    let result = tokio::task::spawn_blocking(move || {
        crate::api::git_ops::run_git_commit(&repo_path, &files, &message, amend, sign)
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(resp) => Json(ApiResponse::ok(resp)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// POST /api/projects/:id/git-push
pub async fn git_push(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<GitPushResponse>> {
    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(p) => p,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let github_token = resolve_github_token_from_state(&state).await;
    let result = tokio::task::spawn_blocking(move || {
        crate::api::git_ops::run_git_push(&repo_path, github_token.as_deref())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(resp) => Json(ApiResponse::ok(resp)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// POST /api/projects/:id/exec
/// Execute a shell command in the project directory for verification.
pub async fn project_exec(
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
            return Json(ApiResponse::err(
                "Terminal requires full_access enabled on at least one agent",
            ));
        }
    }

    // Validate command against strict allowlist
    if let Err(msg) = crate::api::git_ops::validate_exec_command(&cmd) {
        return Json(ApiResponse::err(msg));
    }

    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(p) => p,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    // Rate-limit concurrent exec calls via the shared agent semaphore
    let _permit = match state.agent_semaphore.acquire().await {
        Ok(p) => p,
        Err(_) => return Json(ApiResponse::err("Server is shutting down")),
    };

    let result =
        tokio::task::spawn_blocking(move || crate::api::git_ops::run_exec(&repo_path, &cmd))
            .await
            .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(resp) => Json(ApiResponse::ok(resp)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// POST /api/projects/:id/git-pr
pub async fn create_pr(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreatePrRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(p) => p,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let title = req.title.clone();
    let body = req.body.clone();
    let base = req.base.clone();
    let github_token = resolve_github_token_from_state(&state).await;
    let result = tokio::task::spawn_blocking(move || {
        crate::api::git_ops::run_create_pr(
            &repo_path,
            &title,
            &body,
            &base,
            github_token.as_deref(),
        )
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(url) => Json(ApiResponse::ok(serde_json::json!({ "url": url }))),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// GET /api/projects/:id/pr-template
pub async fn pr_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<serde_json::Value>> {
    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(p) => p,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let branch = sync_cmd("git")
        .args(["branch", "--show-current"])
        .current_dir(&repo_path)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let template = crate::api::git_ops::read_pr_template(&repo_path)
        .unwrap_or_else(|| crate::api::git_ops::default_pr_template(&branch));

    let source = if crate::api::git_ops::read_pr_template(&repo_path).is_some() {
        "project"
    } else {
        "kronn"
    };

    Json(ApiResponse::ok(serde_json::json!({
        "template": template,
        "source": source,
    })))
}
