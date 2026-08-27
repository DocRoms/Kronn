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
    /// Zero-based page offset for the bounded commit history.
    #[serde(default)]
    pub commit_offset: u32,
    /// Requested page size. The git layer clamps it to its hard maximum.
    #[serde(default = "default_git_commit_limit")]
    pub commit_limit: u32,
}

fn default_git_commit_limit() -> u32 {
    crate::api::git_ops::GIT_COMMIT_PAGE_DEFAULT
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

/// Return the active discussions that currently use the project's root
/// worktree.
///
/// The cancellation registry also contains workflow run ids, so its keys
/// cannot be treated as discussion ids blindly. Snapshot it without holding
/// the mutex across the database await, then resolve only real discussions.
///
/// NOTE (KT-89): this snapshot NARROWS the race, it is not mutual exclusion — a
/// run can still start between the check and the `git switch`. Closing it would
/// need a per-project lock held across a git invocation, which trades this defect
/// for a worse class of stall. The dirty-tree refusal inside
/// `run_git_switch_branch` remains the layer underneath.
async fn running_direct_discussions(
    state: &AppState,
    project_id: &str,
) -> Result<Vec<(String, String)>, String> {
    let running_ids: Vec<String> = match state.cancel_registry.lock() {
        Ok(registry) => registry.keys().cloned().collect(),
        Err(poisoned) => poisoned.into_inner().keys().cloned().collect(),
    };
    if running_ids.is_empty() {
        return Ok(Vec::new());
    }

    let project_id = project_id.to_string();
    state
        .db
        .with_read_conn(move |conn| {
            let mut discussions = Vec::new();
            for registry_id in running_ids {
                let discussion = match crate::db::discussions::get_discussion(conn, &registry_id)? {
                    Some(discussion) => discussion,
                    None => {
                        let Some(job) = crate::db::agent_dispatch::get(conn, &registry_id)? else {
                            continue;
                        };
                        let Some(discussion) =
                            crate::db::discussions::get_discussion(conn, &job.discussion_id)?
                        else {
                            continue;
                        };
                        discussion
                    }
                };
                if discussion.project_id.as_deref() == Some(project_id.as_str())
                    && discussion.workspace_mode == "Direct"
                    && !discussions.iter().any(|(id, _)| id == &discussion.id)
                {
                    discussions.push((discussion.id, discussion.title));
                }
            }
            discussions.sort_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)));
            Ok(discussions)
        })
        .await
        .map_err(|error| format!("DB error checking active discussions: {error}"))
}

/// Discussions of this project whose branch is currently swapped INTO the root
/// checkout by test mode.
///
/// KT-89 — a second kind of root occupancy that `workspace_mode` cannot see:
/// entering test mode REQUIRES a worktree branch ("switch to Isolated mode
/// first"), so such a discussion is always `Isolated` and is excluded by the
/// running-run filter by construction. Switching the branch underneath it moves
/// the current branch out from under `test_mode_restore_branch`, so leaving test
/// mode would restore the wrong state and pop its auto-stash onto the wrong
/// branch. Read from the durable column rather than a memory snapshot: no race,
/// and it also catches a test mode left active hours ago.
async fn test_mode_discussions(
    state: &AppState,
    project_id: &str,
) -> Result<Vec<(String, String)>, String> {
    let project_id = project_id.to_string();
    state
        .db
        .with_read_conn(move |conn| {
            let mut statement = conn.prepare(
                "SELECT id, title FROM discussions
                 WHERE project_id = ?1 AND test_mode_restore_branch IS NOT NULL
                 ORDER BY title, id",
            )?;
            let rows = statement.query_map([&project_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
        .map_err(|error| format!("DB error checking test mode: {error}"))
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

    // KT-94 — the language bar is decorative, yet computing it inline made this
    // endpoint take 50.8 s cold on a loaded repo, which the user reads as "the
    // Code tab is broken". Only `?refresh=true` (the explicit re-check button)
    // still blocks on it: the user asked for fresh numbers and gets them. The
    // default path serves the cache — fresh or STALE — and recomputes off the
    // response path; with no cache at all it returns the status immediately
    // with an honest `languages_checked_at: null`.
    let cached_languages = if query.refresh {
        None
    } else {
        state
            .git_language_cache
            .lock()
            .await
            .get(&id)
            .filter(|cached| cached.exclusions == exclusions)
            .map(|cached| {
                (
                    cached.checked_at,
                    cached.languages.clone(),
                    cached.inserted_at.elapsed() < LANGUAGE_CACHE_TTL,
                )
            })
    };

    if query.refresh {
        // Explicit re-check: compute inline, as before.
        let exclusions_for_compute = exclusions.clone();
        let commit_offset = query.commit_offset;
        let commit_limit = query.commit_limit;
        let result = tokio::task::spawn_blocking(move || {
            let mut status =
                crate::api::git_ops::run_git_status_page(&repo_path, commit_offset, commit_limit)?;
            status.languages = crate::api::ai_docs::compute_source_language_stats(
                &repo_path,
                &exclusions_for_compute,
            );
            status.languages_checked_at = Some(chrono::Utc::now());
            Ok::<_, String>(status)
        })
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));
        return match result {
            Ok(status) => {
                if let Some(checked_at) = status.languages_checked_at {
                    state.git_language_cache.lock().await.insert(
                        id,
                        CachedProjectLanguages {
                            inserted_at: Instant::now(),
                            checked_at,
                            exclusions,
                            languages: status.languages.clone(),
                        },
                    );
                }
                Json(ApiResponse::ok(status))
            }
            Err(e) => Json(ApiResponse::err(e)),
        };
    }

    let repo_path_for_status = repo_path.clone();
    let commit_offset = query.commit_offset;
    let commit_limit = query.commit_limit;
    let result = tokio::task::spawn_blocking(move || {
        crate::api::git_ops::run_git_status_page(&repo_path_for_status, commit_offset, commit_limit)
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(mut status) => {
            let fresh = match cached_languages {
                Some((checked_at, languages, is_fresh)) => {
                    status.languages = languages;
                    status.languages_checked_at = Some(checked_at);
                    status.languages_cached = true;
                    is_fresh
                }
                // No cache yet: an empty bar with a null timestamp, never a wait.
                None => false,
            };
            if !fresh {
                spawn_language_refresh(&state, id, repo_path, exclusions);
            }
            Json(ApiResponse::ok(status))
        }
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// Recompute a project's language stats off the response path (KT-94).
///
/// At most one computation per project at a time: three open tabs polling
/// git-status must not scan the same repository three times in parallel. The
/// in-flight set is module-local because this is the only spawner.
fn spawn_language_refresh(
    state: &AppState,
    project_id: String,
    repo_path: std::path::PathBuf,
    exclusions: Vec<String>,
) {
    static IN_FLIGHT: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

    {
        let mut in_flight = match IN_FLIGHT.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !in_flight.insert(project_id.clone()) {
            return; // already computing for this project
        }
    }

    let cache = state.git_language_cache.clone();
    tokio::spawn(async move {
        let repo_for_compute = repo_path.clone();
        let exclusions_for_compute = exclusions.clone();
        let languages = tokio::task::spawn_blocking(move || {
            crate::api::ai_docs::compute_source_language_stats(
                &repo_for_compute,
                &exclusions_for_compute,
            )
        })
        .await;
        if let Ok(languages) = languages {
            cache.lock().await.insert(
                project_id.clone(),
                CachedProjectLanguages {
                    inserted_at: Instant::now(),
                    checked_at: chrono::Utc::now(),
                    exclusions,
                    languages,
                },
            );
        }
        let mut in_flight = match IN_FLIGHT.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        in_flight.remove(&project_id);
    });
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

/// GET /api/projects/:id/git-commit?sha=abc1234
///
/// KT-67 — the commit behind an annotated line. Metadata + message + bounded
/// containing branches; never the patch (see `run_git_commit_detail`).
pub async fn git_commit_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::git::GitCommitQuery>,
) -> Json<ApiResponse<crate::models::git::GitCommitDetail>> {
    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(path) => path,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let sha = query.sha;
    let result = tokio::task::spawn_blocking(move || {
        crate::api::git_ops::run_git_commit_detail(&repo_path, &sha)
    })
    .await
    .unwrap_or_else(|error| Err(format!("Task failed: {error}")));

    match result {
        Ok(detail) => Json(ApiResponse::ok(detail)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

/// GET /api/projects/:id/git-commit-patch?sha=abc1234
///
/// KT-75 — the commit's own patch, for the temporary tab opened from an
/// annotated line. Separate from `git-commit-detail`, which stays cheap enough
/// to answer a hover.
pub async fn git_commit_patch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::git::GitCommitQuery>,
) -> Json<ApiResponse<crate::models::git::GitCommitPatch>> {
    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(path) => path,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let sha = query.sha;
    let result = tokio::task::spawn_blocking(move || {
        crate::api::git_ops::run_git_commit_patch(&repo_path, &sha)
    })
    .await
    .unwrap_or_else(|error| Err(format!("Task failed: {error}")));

    match result {
        Ok(patch) => Json(ApiResponse::ok(patch)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

/// GET /api/projects/:id/git-branches
pub async fn git_branches(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<GitBranchesResponse>> {
    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(path) => path,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let result =
        tokio::task::spawn_blocking(move || crate::api::git_ops::run_git_branches(&repo_path))
            .await
            .unwrap_or_else(|error| Err(format!("Task failed: {error}")));

    match result {
        Ok(branches) => Json(ApiResponse::ok(branches)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

/// POST /api/projects/:id/git-switch
pub async fn git_switch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<GitSwitchBranchRequest>,
) -> Json<ApiResponse<GitBranchResponse>> {
    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(path) => path,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    // Durable occupancy first: it cannot race, and its consequence (a broken test
    // mode restore point) is worse than an interrupted run.
    let in_test_mode = match test_mode_discussions(&state, &id).await {
        Ok(discussions) => discussions,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    if !in_test_mode.is_empty() {
        let discussions = in_test_mode
            .iter()
            .map(|(discussion_id, title)| format!("\"{title}\" ({discussion_id})"))
            .collect::<Vec<_>>()
            .join(", ");
        return Json(ApiResponse::err(format!(
            "Cannot switch branch while test mode is holding this project's root \
             checkout: {discussions}. Leave test mode from that discussion first — \
             it restores the previous branch and pops its stash for you."
        )));
    }

    let running = match running_direct_discussions(&state, &id).await {
        Ok(discussions) => discussions,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    if !running.is_empty() {
        let discussions = running
            .iter()
            .map(|(discussion_id, title)| format!("\"{title}\" ({discussion_id})"))
            .collect::<Vec<_>>()
            .join(", ");
        return Json(ApiResponse::err(format!(
            "Cannot switch branch while an agent run is using this project's root worktree: \
             {discussions}. Wait for it to finish or stop it explicitly first."
        )));
    }

    let branch = request.branch;
    let result = tokio::task::spawn_blocking(move || {
        crate::api::git_ops::run_git_switch_branch(&repo_path, &branch)
    })
    .await
    .unwrap_or_else(|error| Err(format!("Task failed: {error}")));

    match result {
        Ok(branch) => Json(ApiResponse::ok(branch)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::default_config;
    use crate::db::Database;
    use crate::{CancelGuard, DEFAULT_MAX_CONCURRENT_AGENTS};
    use std::sync::Arc;
    use std::{path::Path as FsPath, process::Command};
    use tokio::sync::RwLock;

    fn scoped_commit_repo() -> tempfile::TempDir {
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

    fn git_names(repo: &FsPath, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn project_commit_endpoint_commits_only_requested_paths() {
        let repo = scoped_commit_repo();
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
                     VALUES ('project-scoped-commit', 'Project', ?1, '{}', 'now', 'now')",
                    [&path],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let response = git_commit(
            State(state),
            Path("project-scoped-commit".into()),
            Json(GitCommitRequest {
                files: vec!["a.txt".into()],
                message: "test: scoped project commit".into(),
                amend: false,
                sign: false,
            }),
        )
        .await
        .0;
        assert!(response.success, "project commit endpoint must succeed");
        assert_eq!(
            git_names(repo.path(), &["diff", "HEAD^", "HEAD", "--name-only"]),
            "a.txt"
        );
        assert_eq!(
            git_names(repo.path(), &["diff", "--cached", "--name-only"]),
            "b.txt",
            "an unrelated staged file must remain staged"
        );
    }

    #[tokio::test]
    async fn running_direct_discussions_ignores_workflow_keys_isolated_and_other_projects() {
        let state = AppState::new_defaults(
            Arc::new(RwLock::new(default_config())),
            Arc::new(Database::open_in_memory().expect("in-memory DB")),
            DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        state
            .db
            .with_conn(|connection| {
                for (id, name, path) in [
                    ("project-a", "Project A", "/tmp/kronn-project-a"),
                    ("project-b", "Project B", "/tmp/kronn-project-b"),
                ] {
                    connection.execute(
                        "INSERT INTO projects
                         (id, name, path, ai_config_json, created_at, updated_at)
                         VALUES (?1, ?2, ?3, '{}', 'now', 'now')",
                        [id, name, path],
                    )?;
                }
                for (id, project_id, title, workspace_mode) in [
                    ("direct-a", "project-a", "Direct A", "Direct"),
                    ("isolated-a", "project-a", "Isolated A", "Isolated"),
                    ("direct-b", "project-b", "Direct B", "Direct"),
                ] {
                    connection.execute(
                        "INSERT INTO discussions
                         (id, project_id, title, agent, language, created_at, updated_at,
                          workspace_mode)
                         VALUES (?1, ?2, ?3, 'Codex', 'en', 'now', 'now', ?4)",
                        [id, project_id, title, workspace_mode],
                    )?;
                }
                connection.execute(
                    "INSERT INTO messages
                     (id, discussion_id, role, channel, content, timestamp, sort_order)
                     VALUES ('u-direct-a', 'direct-a', 'User', 'main', 'run', 'now', 1)",
                    [],
                )?;
                crate::db::agent_dispatch::enqueue(
                    connection,
                    crate::db::agent_dispatch::NewAgentDispatchJob {
                        id: "dispatch-direct-a",
                        discussion_id: "direct-a",
                        trigger_message_id: "u-direct-a",
                        trigger_sort_order: 1,
                        dedupe_key: "dispatch-direct-a",
                        agent_override: Some(&crate::models::AgentType::Codex),
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: None,
                        group_concurrency_limit: None,
                    },
                )?;
                Ok(())
            })
            .await
            .expect("seed projects and discussions");

        let _direct = CancelGuard::insert(&state.cancel_registry, "direct-a".to_string());
        let _durable = CancelGuard::insert(&state.cancel_registry, "dispatch-direct-a".to_string());
        let _isolated = CancelGuard::insert(&state.cancel_registry, "isolated-a".to_string());
        let _other = CancelGuard::insert(&state.cancel_registry, "direct-b".to_string());
        let _workflow = CancelGuard::insert(&state.cancel_registry, "workflow-run-123".to_string());

        assert_eq!(
            running_direct_discussions(&state, "project-a")
                .await
                .expect("active direct discussions"),
            vec![("direct-a".to_string(), "Direct A".to_string())]
        );
    }
}
