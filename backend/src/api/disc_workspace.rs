//! KT-140 — joined-CLI workspace declaration and compact state.

use std::path::{Path, PathBuf};

use axum::{
    extract::{Path as AxumPath, Query, State},
    Json,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::cmd::sync_cmd;
use crate::db::discussion_workspaces::{
    DiscussionWorkspace, HistoryLeaseAcquire, WorkspaceHistoryLease,
};
use crate::models::{ApiErrorCode, ApiResponse, Project};
use crate::AppState;

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscWorkspaceQuery {
    pub source_agent: String,
    pub source_session_id: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscWorkspaceSetRequest {
    pub source_agent: String,
    pub source_session_id: String,
    pub workspace_path: String,
    #[serde(default)]
    pub task_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscWorkspaceState {
    pub disc_id: String,
    pub session_pk: i64,
    pub current: Option<DiscussionWorkspace>,
    pub workspaces: Vec<DiscussionWorkspace>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscWorkspaceBlocker {
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscWorkspaceSetResponse {
    pub workspace: DiscussionWorkspace,
    pub blockers: Vec<DiscWorkspaceBlocker>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscWorkspaceHistoryLeaseRequest {
    pub source_agent: String,
    pub source_session_id: String,
    /// `acquire` (or idempotent renewal) / `release`.
    pub action: String,
    #[serde(default)]
    pub backup_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscWorkspaceHistoryLeaseResponse {
    pub acquired: bool,
    pub advisory: bool,
    pub lease: Option<WorkspaceHistoryLease>,
    pub blocker: Option<DiscWorkspaceBlocker>,
}

#[derive(Debug)]
struct ValidatedWorkspace {
    project_id: String,
    workspace_path: String,
    canonical_path: String,
    branch: String,
    head_sha: String,
    blockers: Vec<DiscWorkspaceBlocker>,
}

fn required_identity(
    source_agent: &str,
    source_session_id: &str,
) -> Result<(String, String), &'static str> {
    let source_agent = source_agent.trim();
    let source_session_id = source_session_id.trim();
    if source_agent.is_empty() || source_session_id.is_empty() {
        return Err("source_agent and source_session_id are required");
    }
    if source_agent.chars().count() > 80 || source_session_id.chars().count() > 512 {
        return Err("source_agent or source_session_id exceeds the supported length");
    }
    Ok((source_agent.to_string(), source_session_id.to_string()))
}

fn git_value(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = sync_cmd("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("git is unavailable: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("git {} failed", args.join(" "))
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_common_dir(cwd: &Path) -> Result<PathBuf, String> {
    let value = git_value(cwd, &["rev-parse", "--git-common-dir"])?;
    let path = PathBuf::from(value);
    let resolved = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    resolved
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize Git common dir: {error}"))
}

fn local_repo_roots(project: &Project) -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from(&project.path)];
    roots.extend(
        project
            .linked_repos
            .iter()
            .map(|repo| repo.location.trim())
            .filter(|location| {
                !location.is_empty()
                    && !location.starts_with("http://")
                    && !location.starts_with("https://")
                    && !location.starts_with("git@")
            })
            .map(PathBuf::from),
    );
    roots
}

fn registered_worktrees(repo_root: &Path) -> Result<Vec<PathBuf>, String> {
    let output = git_value(repo_root, &["worktree", "list", "--porcelain"])?;
    Ok(output
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .filter_map(|path| PathBuf::from(path).canonicalize().ok())
        .collect())
}

fn validate_workspace(
    project_id: &str,
    repo_roots: Vec<PathBuf>,
    requested_path: &str,
) -> Result<ValidatedWorkspace, String> {
    let requested_path = requested_path.trim();
    if requested_path.is_empty() {
        return Err("workspace_path is required".into());
    }
    let requested = crate::core::scanner::resolve_host_path(requested_path);
    let requested = requested
        .canonicalize()
        .map_err(|error| format!("workspace path does not exist: {error}"))?;
    if !requested.is_dir() {
        return Err("workspace path is not a directory".into());
    }

    let top_level = PathBuf::from(git_value(&requested, &["rev-parse", "--show-toplevel"])?)
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize Git worktree root: {error}"))?;
    let candidate_common = git_common_dir(&top_level)?;

    let mut matching_repo = None;
    for root in repo_roots {
        let root = crate::core::scanner::resolve_host_path(&root.to_string_lossy());
        let Ok(root) = root.canonicalize() else {
            continue;
        };
        let Ok(common) = git_common_dir(&root) else {
            continue;
        };
        if common == candidate_common {
            matching_repo = Some(root);
            break;
        }
    }
    let repo_root = matching_repo.ok_or_else(|| {
        "workspace does not belong to the discussion project repositories".to_string()
    })?;

    let is_registered = registered_worktrees(&repo_root)?
        .iter()
        .any(|path| path == &top_level);
    if !is_registered {
        return Err("workspace is not a registered Git worktree".into());
    }

    let branch = git_value(&top_level, &["branch", "--show-current"])?;
    if branch.is_empty() {
        return Err("detached HEAD worktrees cannot be declared".into());
    }
    let head_sha = git_value(&top_level, &["rev-parse", "HEAD"])?;
    let git_status = crate::api::git_ops::run_git_status(&top_level)?;
    let dirty_count = git_status.files.len();
    let blockers = if dirty_count == 0 {
        Vec::new()
    } else {
        vec![DiscWorkspaceBlocker {
            kind: "dirty".to_string(),
            message: format!("{dirty_count} uncommitted file(s) are present in this workspace"),
        }]
    };

    Ok(ValidatedWorkspace {
        project_id: project_id.to_string(),
        workspace_path: top_level.to_string_lossy().to_string(),
        canonical_path: top_level.to_string_lossy().to_string(),
        branch,
        head_sha,
        blockers,
    })
}

async fn session_context(
    state: &AppState,
    source_agent: String,
    source_session_id: String,
) -> anyhow::Result<(crate::db::discussion_sessions::DiscussionSession, Project)> {
    state
        .db
        .with_read_conn(move |conn| {
            let session = crate::db::discussion_sessions::find_active_session(
                conn,
                &source_agent,
                &source_session_id,
            )?
            .ok_or_else(|| anyhow::anyhow!("joined CLI session not found"))?;
            let discussion = crate::db::discussions::get_discussion(conn, &session.disc_id)?
                .ok_or_else(|| anyhow::anyhow!("discussion not found"))?;
            let project_id = discussion
                .project_id
                .ok_or_else(|| anyhow::anyhow!("discussion has no project"))?;
            let project = crate::db::projects::get_project(conn, &project_id)?
                .ok_or_else(|| anyhow::anyhow!("project not found"))?;
            Ok((session, project))
        })
        .await
}

/// `GET /api/disc/workspace` — compact current-session + room workspace state.
pub async fn disc_workspace_get(
    State(state): State<AppState>,
    Query(query): Query<DiscWorkspaceQuery>,
) -> Json<ApiResponse<DiscWorkspaceState>> {
    let (source_agent, source_session_id) =
        match required_identity(&query.source_agent, &query.source_session_id) {
            Ok(identity) => identity,
            Err(error) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, error)),
        };
    let (session, _) = match session_context(&state, source_agent, source_session_id).await {
        Ok(context) => context,
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                error.to_string(),
            ))
        }
    };
    let disc_id = session.disc_id.clone();
    let session_pk = session.id;
    let query_disc_id = disc_id.clone();
    let result = state
        .db
        .with_read_conn(move |conn| {
            let workspaces =
                crate::db::discussion_workspaces::list_for_discussion(conn, &query_disc_id)?;
            let current = workspaces
                .iter()
                .find(|workspace| workspace.session_pk == Some(session_pk))
                .cloned();
            Ok::<_, anyhow::Error>(DiscWorkspaceState {
                disc_id: query_disc_id,
                session_pk,
                current,
                workspaces,
            })
        })
        .await;
    match result {
        Ok(response) => Json(ApiResponse::ok(response)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            error.to_string(),
        )),
    }
}

/// `GET /api/discussions/:id/workspaces` — UI-facing room workspace list.
pub async fn discussion_workspaces(
    State(state): State<AppState>,
    AxumPath(disc_id): AxumPath<String>,
) -> Json<ApiResponse<Vec<DiscussionWorkspace>>> {
    let result = state
        .db
        .with_read_conn(move |conn| {
            if crate::db::discussions::get_discussion(conn, &disc_id)?.is_none() {
                anyhow::bail!("discussion not found");
            }
            crate::db::discussion_workspaces::list_visible_for_discussion(conn, &disc_id)
        })
        .await;
    match result {
        Ok(workspaces) => Json(ApiResponse::ok(workspaces)),
        Err(error) if error.to_string().contains("not found") => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            error.to_string(),
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            error.to_string(),
        )),
    }
}

/// `POST /api/disc/workspace` — validate Git facts and upsert this CLI's row.
pub async fn disc_workspace_set(
    State(state): State<AppState>,
    Json(request): Json<DiscWorkspaceSetRequest>,
) -> Json<ApiResponse<DiscWorkspaceSetResponse>> {
    let (source_agent, source_session_id) =
        match required_identity(&request.source_agent, &request.source_session_id) {
            Ok(identity) => identity,
            Err(error) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, error)),
        };
    let (session, project) = match session_context(&state, source_agent, source_session_id).await {
        Ok(context) => context,
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                error.to_string(),
            ))
        }
    };

    let requested_path = request.workspace_path.clone();
    let project_id = project.id.clone();
    let repo_roots = local_repo_roots(&project);
    let validated = match tokio::task::spawn_blocking(move || {
        validate_workspace(&project_id, repo_roots, &requested_path)
    })
    .await
    {
        Ok(Ok(workspace)) => workspace,
        Ok(Err(error)) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, error)),
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                format!("workspace validation task failed: {error}"),
            ))
        }
    };

    let task_ref = request
        .task_ref
        .as_deref()
        .map(str::trim)
        .filter(|reference| !reference.is_empty())
        .map(str::to_string);
    let disc_id = session.disc_id.clone();
    let session_pk = session.id;
    let result = state
        .db
        .with_conn(move |conn| {
            // KT-328: a room already served by a backend-owned `managed` worktree
            // must not be re-declared by a CLI — the accepting worker READS that row,
            // it does not overwrite it. Refuse cleanly (→ Conflict) rather than let
            // the UNIQUE canonical_path index throw an opaque constraint error, or
            // create a second external row with no designated teardown authority.
            if let Some(managed) =
                crate::db::discussion_workspaces::get_managed_for_discussion(conn, &disc_id)?
            {
                let owner = managed.task_execution_id.as_deref().unwrap_or("?");
                anyhow::bail!(
                    "workspace is backend-owned (managed) for execution {owner}; \
                     read it via the room workspace list, do not declare it"
                );
            }
            let task_id = if let Some(reference) = task_ref {
                let task = crate::db::planning::get_task(conn, &reference)?
                    .ok_or_else(|| anyhow::anyhow!("planning task not found"))?;
                let belongs_to_scope = task.summary.discussion_ids.contains(&disc_id)
                    || task.summary.project_ids.contains(&validated.project_id);
                if !belongs_to_scope {
                    anyhow::bail!("planning task is not linked to this discussion or project");
                }
                Some(task.summary.id)
            } else {
                None
            };
            let mut blockers = validated.blockers;
            let branch_owner = conn
                .query_row(
                    "SELECT ds.agent_type
                       FROM discussion_workspaces dw
                       LEFT JOIN discussion_sessions ds ON ds.id = dw.session_pk
                      WHERE dw.disc_id = ?1
                        AND dw.project_id = ?2
                        AND dw.branch = ?3
                        AND dw.session_pk IS NOT ?4
                        AND dw.state = 'attached'
                      LIMIT 1",
                    rusqlite::params![
                        &disc_id,
                        &validated.project_id,
                        &validated.branch,
                        session_pk
                    ],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();
            if let Some(agent_type) = branch_owner {
                blockers.push(DiscWorkspaceBlocker {
                    kind: "branch_checked_out".to_string(),
                    message: format!(
                        "branch {} is also declared by {agent_type} in this discussion",
                        validated.branch
                    ),
                });
            }
            let workspace = crate::db::discussion_workspaces::upsert_external(
                conn,
                &disc_id,
                session_pk,
                task_id.as_deref(),
                &validated.project_id,
                &validated.workspace_path,
                &validated.canonical_path,
                &validated.branch,
                &validated.head_sha,
            )?;
            Ok(DiscWorkspaceSetResponse {
                workspace,
                blockers,
            })
        })
        .await;

    match result {
        Ok(workspace) => Json(ApiResponse::ok(workspace)),
        Err(error) => {
            let message = error.to_string();
            let code = if message.contains("UNIQUE constraint failed")
                || message.contains("another discussion")
                || message.contains("backend-owned")
            {
                ApiErrorCode::Conflict
            } else {
                ApiErrorCode::Validation
            };
            Json(ApiResponse::err_coded(code, message))
        }
    }
}

/// `POST /api/disc/workspace/history-lease` — advisory Git rewrite guard.
pub async fn disc_workspace_history_lease(
    State(state): State<AppState>,
    Json(request): Json<DiscWorkspaceHistoryLeaseRequest>,
) -> Json<ApiResponse<DiscWorkspaceHistoryLeaseResponse>> {
    let (source_agent, source_session_id) =
        match required_identity(&request.source_agent, &request.source_session_id) {
            Ok(identity) => identity,
            Err(error) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, error)),
        };
    let (session, project) = match session_context(&state, source_agent, source_session_id).await {
        Ok(context) => context,
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                error.to_string(),
            ))
        }
    };
    let action = request.action.trim().to_ascii_lowercase();
    let disc_id = session.disc_id.clone();
    let session_pk = session.id;

    if action == "release" {
        let result = state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_workspaces::release_history_lease(conn, &disc_id, session_pk)
            })
            .await;
        return match result {
            Ok(released) => Json(ApiResponse::ok(DiscWorkspaceHistoryLeaseResponse {
                acquired: false,
                advisory: true,
                lease: None,
                blocker: (!released).then(|| DiscWorkspaceBlocker {
                    kind: "not_held".into(),
                    message: "this session did not hold an active history-rewrite lease".into(),
                }),
            })),
            Err(error) => Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                error.to_string(),
            )),
        };
    }
    if action != "acquire" {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "action must be `acquire` or `release`",
        ));
    }

    let backup_ref = match request
        .backup_ref
        .as_deref()
        .map(str::trim)
        .filter(|value| value.starts_with("refs/kronn-backup/") && value.len() <= 240)
    {
        Some(value) => value.to_string(),
        None => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                "acquire requires a backup_ref under refs/kronn-backup/",
            ))
        }
    };

    let lookup_disc_id = disc_id.clone();
    let workspace = match state
        .db
        .with_read_conn(move |conn| {
            crate::db::discussion_workspaces::get_for_session(conn, &lookup_disc_id, session_pk)?
                .ok_or_else(|| {
                    anyhow::anyhow!("declare this session workspace before acquiring a lease")
                })
        })
        .await
    {
        Ok(workspace) => workspace,
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                error.to_string(),
            ))
        }
    };
    let workspace_path = match workspace.workspace_path {
        Some(path) if workspace.state == "attached" => path,
        _ => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                "the declared workspace is not attached",
            ))
        }
    };
    let project_id = project.id.clone();
    let repo_roots = local_repo_roots(&project);
    let backup_ref_for_git = backup_ref.clone();
    let verified = tokio::task::spawn_blocking(move || {
        let workspace = validate_workspace(&project_id, repo_roots, &workspace_path)?;
        let backup_sha = git_value(
            Path::new(&workspace.canonical_path),
            &["rev-parse", "--verify", &format!("{backup_ref_for_git}^{{commit}}")],
        )?;
        if backup_sha != workspace.head_sha {
            return Err(format!(
                "backup_ref points to {backup_sha}, not the current HEAD {}; create or refresh it before acquiring the lease",
                workspace.head_sha
            ));
        }
        Ok::<_, String>(workspace)
    })
    .await;
    let verified = match verified {
        Ok(Ok(workspace)) => workspace,
        Ok(Err(error)) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, error)),
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                format!("history-lease validation task failed: {error}"),
            ))
        }
    };
    if workspace.branch != verified.branch
        || workspace.head_sha.as_deref() != Some(&verified.head_sha)
    {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            "the worktree branch or HEAD changed since disc_workspace_set; refresh the declaration and backup ref",
        ));
    }

    let result = state
        .db
        .with_conn(move |conn| {
            crate::db::discussion_workspaces::acquire_history_lease(
                conn,
                &disc_id,
                session_pk,
                &backup_ref,
            )
        })
        .await;
    match result {
        Ok(HistoryLeaseAcquire::Acquired(lease)) => {
            Json(ApiResponse::ok(DiscWorkspaceHistoryLeaseResponse {
                acquired: true,
                advisory: true,
                lease: Some(lease),
                blocker: None,
            }))
        }
        Ok(HistoryLeaseAcquire::Blocked(owner)) => {
            let message = format!(
                "history rewrite refused: {} ({}) already holds the advisory lease until {}",
                owner.session_agent_type,
                owner.session_id.as_deref().unwrap_or("unknown session"),
                owner.expires_at
            );
            Json(ApiResponse::ok(DiscWorkspaceHistoryLeaseResponse {
                acquired: false,
                advisory: true,
                lease: Some(owner),
                blocker: Some(DiscWorkspaceBlocker {
                    kind: "history_rewrite_locked".into(),
                    message,
                }),
            }))
        }
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            error.to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn git(cwd: &Path, args: &[&str]) {
        let output = sync_cmd("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository_with_worktree() -> (TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let worktree = temp.path().join("worktree");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init"]);
        git(&repo, &["config", "user.email", "tests@kronn.local"]);
        git(&repo, &["config", "user.name", "Kronn Tests"]);
        std::fs::write(repo.join("README.md"), "fixture").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "fixture"]);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "feature/workspace",
                worktree.to_str().unwrap(),
            ],
        );
        (temp, repo, worktree)
    }

    #[test]
    fn identity_rejects_half_empty_and_overlong_values() {
        assert!(required_identity("", "session").is_err());
        assert!(required_identity("Codex", "").is_err());
        assert!(required_identity(&"a".repeat(81), "session").is_err());
        assert_eq!(
            required_identity(" Codex ", " session ").unwrap(),
            ("Codex".into(), "session".into())
        );
    }

    #[test]
    fn validation_reads_registered_worktree_branch_and_head() {
        let (_temp, repo, worktree) = repository_with_worktree();
        let validated = validate_workspace("p1", vec![repo], worktree.to_str().unwrap()).unwrap();
        assert_eq!(validated.project_id, "p1");
        assert_eq!(validated.branch, "feature/workspace");
        assert_eq!(validated.head_sha.len(), 40);
        assert_eq!(
            PathBuf::from(validated.canonical_path),
            worktree.canonicalize().unwrap()
        );
        assert!(validated.blockers.is_empty());
    }

    #[test]
    fn validation_reports_dirty_workspace_as_a_structured_blocker() {
        let (_temp, repo, worktree) = repository_with_worktree();
        std::fs::write(worktree.join("dirty.txt"), "uncommitted").unwrap();

        let validated = validate_workspace("p1", vec![repo], worktree.to_str().unwrap()).unwrap();

        assert_eq!(validated.blockers.len(), 1);
        assert_eq!(validated.blockers[0].kind, "dirty");
        assert!(validated.blockers[0].message.contains("1 uncommitted"));
    }

    #[test]
    fn validation_rejects_a_worktree_from_an_unlinked_repository() {
        let (_temp, _repo, worktree) = repository_with_worktree();
        let other = tempfile::tempdir().unwrap();
        git(other.path(), &["init"]);
        let error = validate_workspace(
            "p1",
            vec![other.path().to_path_buf()],
            worktree.to_str().unwrap(),
        )
        .unwrap_err();
        assert!(error.contains("does not belong"));
    }
}
