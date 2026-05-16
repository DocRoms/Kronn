// Project CRUD: list / get / scan / create / add-folder / delete +
// the small mutation endpoints (default skills/profile, path remap).
// Bootstrap, clone, template, git ops, and migrate live in sibling files.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::core::cmd::sync_cmd;
use crate::core::scanner;
use crate::models::*;
use crate::AppState;

use super::{enrich_audit_status, find_common_parent};

/// GET /api/projects
pub async fn list(State(state): State<AppState>) -> Json<ApiResponse<Vec<Project>>> {
    match state.db.with_conn(crate::db::projects::list_projects).await {
        Ok(mut projects) => {
            let projects = tokio::task::spawn_blocking(move || {
                for p in &mut projects {
                    enrich_audit_status(p);
                }
                projects
            }).await.unwrap_or_else(|e| {
                tracing::error!("Failed to enrich audit status: {e}");
                vec![]
            });
            Json(ApiResponse::ok(projects))
        }
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// GET /api/projects/:id
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Project>> {
    let pid = id.clone();
    match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &pid)).await {
        Ok(Some(mut project)) => {
            tokio::task::spawn_blocking(move || {
                enrich_audit_status(&mut project);
                project
            })
            .await
            .map(|p| Json(ApiResponse::ok(p)))
            .unwrap_or_else(|e| Json(ApiResponse::err(format!("Enrich failed: {e}"))))
        }
        Ok(None) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// Discover WSL home directories on Windows hosts. Returns UNC paths
/// (`\\wsl.localhost\<distro>\home\<user>`) so subsequent project scans
/// can reach into WSL filesystems where most dev repos actually live.
///
/// Hardened: every `wsl.exe` invocation runs through a per-distro timeout
/// (`WSL_DISTRO_TIMEOUT`). A corrupted or stopped distro can otherwise hang
/// `wsl.exe -d <distro>` indefinitely and freeze project scanning.
pub fn discover_wsl_homes() -> Vec<String> {
    let mut homes = Vec::new();
    #[cfg(target_os = "windows")]
    {
        use crate::core::cmd::sync_cmd;
        use std::time::{Duration, Instant};

        /// Max wait per `wsl.exe` invocation. wsl.exe -l -q normally answers
        /// in <100 ms; we give it generous slack but still cap so a hung
        /// distro can't lock out the scan endpoint.
        const WSL_DISTRO_TIMEOUT: Duration = Duration::from_secs(5);

        /// Spawn a wsl.exe child, kill it if it overruns `timeout`, return its output.
        fn run_with_timeout(
            args: &[&str],
            timeout: Duration,
        ) -> Option<std::process::Output> {
            let mut child = sync_cmd("wsl.exe")
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .ok()?;
            let deadline = Instant::now() + timeout;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => return child.wait_with_output().ok(),
                    Ok(None) => {
                        if Instant::now() >= deadline {
                            tracing::warn!(
                                "wsl.exe {:?} did not return within {:?}; killing",
                                args,
                                timeout
                            );
                            let _ = child.kill();
                            let _ = child.wait();
                            return None;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) => {
                        tracing::warn!("wsl.exe poll failed: {}", e);
                        let _ = child.kill();
                        return None;
                    }
                }
            }
        }

        // List installed distros
        let distros: Vec<String> = match run_with_timeout(&["-l", "-q"], WSL_DISTRO_TIMEOUT) {
            Some(out) if out.status.success() => {
                // wsl.exe outputs UTF-16 LE on Windows
                let stdout = String::from_utf8_lossy(&out.stdout)
                    .replace('\u{0}', "")
                    .replace('\r', "");
                stdout.lines()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty() && !s.contains("docker-desktop"))
                    .collect()
            }
            _ => return homes,
        };

        for distro in &distros {
            // List /home directory inside the distro
            let args = ["-d", distro.as_str(), "--", "ls", "/home"];
            if let Some(out) = run_with_timeout(&args, WSL_DISTRO_TIMEOUT) {
                if out.status.success() {
                    let users = String::from_utf8_lossy(&out.stdout);
                    for user in users.lines() {
                        let user = user.trim();
                        if !user.is_empty() {
                            // UNC path: \\wsl.localhost\<distro>\home\<user>
                            homes.push(format!("\\\\wsl.localhost\\{}\\home\\{}", distro, user));
                        }
                    }
                }
            }
        }

        // Fallback: try direct \\wsl.localhost\ filesystem read if wsl.exe didn't return anything
        if homes.is_empty() {
            for distro_path in &["\\\\wsl.localhost", "\\\\wsl$"] {
                let wsl_root = std::path::Path::new(distro_path);
                if let Ok(entries) = std::fs::read_dir(wsl_root) {
                    for entry in entries.flatten() {
                        let home = entry.path().join("home");
                        if let Ok(users) = std::fs::read_dir(&home) {
                            for user in users.flatten() {
                                if user.path().is_dir() {
                                    homes.push(user.path().to_string_lossy().to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = &mut homes;
    }
    homes
}

pub async fn scan(State(state): State<AppState>) -> Json<ApiResponse<Vec<DetectedRepo>>> {
    let config = state.config.read().await;

    let mut scan_paths = if config.scan.paths.is_empty() {
        // Fallback: Docker host home, or user home
        let mut paths: Vec<String> = Vec::new();
        if let Ok(host_home) = std::env::var("KRONN_HOST_HOME") {
            paths.push(host_home);
        }
        if paths.is_empty() {
            if let Some(home) = directories::UserDirs::new().map(|d| d.home_dir().to_string_lossy().to_string()) {
                paths.push(home);
            }
        }
        paths
    } else {
        config.scan.paths.clone()
    };

    // On Windows: always include WSL home directories (most dev repos live there)
    let wsl_homes = discover_wsl_homes();
    for wsl_home in &wsl_homes {
        if !scan_paths.contains(wsl_home) {
            scan_paths.push(wsl_home.clone());
        }
    }

    let ignore = config.scan.ignore.clone();
    let depth = config.scan.scan_depth;
    drop(config);

    let existing_paths: Vec<String> = state.db.with_conn(|conn| {
        let projects = crate::db::projects::list_projects(conn)?;
        Ok(projects.into_iter().map(|p| p.path).collect())
    }).await.unwrap_or_default();

    match scanner::scan_paths_with_depth(&scan_paths, &ignore, depth).await {
        Ok(mut repos) => {
            for repo in &mut repos {
                repo.has_project = existing_paths.contains(&repo.path);
            }
            Json(ApiResponse::ok(repos))
        }
        Err(e) => Json(ApiResponse::err(format!("Scan failed: {}", e))),
    }
}

/// POST /api/projects
pub async fn create(
    State(state): State<AppState>,
    Json(repo): Json<DetectedRepo>,
) -> Json<ApiResponse<Project>> {
    // Reject path traversal attempts before they reach the DB. A registered
    // project path is later passed to scanner/mcp_scanner/file readers, so a
    // `..` component would let a remote caller (peer / future multi-user mode)
    // anchor reads outside the intended scan roots.
    if scanner::contains_parent_dir(&repo.path) {
        return Json(ApiResponse::err("Project path may not contain '..' components"));
    }

    let now = Utc::now();

    let mut project = Project {
        id: Uuid::new_v4().to_string(),
        name: repo.name.clone(),
        path: repo.path.clone(),
        repo_url: repo.remote_url.clone(),
        token_override: None,
        ai_config: AiConfigStatus {
            detected: !repo.ai_configs.is_empty(),
            configs: repo.ai_configs.clone(),
        },
        audit_status: AiAuditStatus::NoTemplate,
        ai_todo_count: 0,
            tech_debt_count: 0,
        needs_docs_migration: false,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        linked_repos: vec![],
        created_at: now,
        updated_at: now,
    };
    enrich_audit_status(&mut project);

    let p = project.clone();
    match state.db.with_conn(move |conn| {
        crate::db::projects::insert_project(conn, &p)?;
        Ok(())
    }).await {
        Ok(()) => Json(ApiResponse::ok(project)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct AddFolderRequest {
    pub path: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// POST /api/projects/add-folder
///
/// Register a local directory as a project — no git required. Useful for
/// documentation repos, design folders, config directories, or any workspace
/// that doesn't have a `.git` root. The path must exist on disk.
///
/// If the folder happens to contain a `.git`, we auto-detect the remote URL
/// and branch so the project still benefits from worktree isolation and PR
/// features. Otherwise `repo_url` is set to None and git features are
/// gracefully disabled.
pub async fn add_folder(
    State(state): State<AppState>,
    Json(req): Json<AddFolderRequest>,
) -> Json<ApiResponse<Project>> {
    if scanner::contains_parent_dir(&req.path) {
        return Json(ApiResponse::err("Project path may not contain '..' components"));
    }

    let resolved = crate::core::scanner::resolve_host_path(&req.path);
    if !resolved.is_dir() {
        return Json(ApiResponse::err(format!(
            "Directory does not exist: {}",
            resolved.display()
        )));
    }

    // Auto-detect name from last path component if not provided.
    let name = req.name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| {
            resolved.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project")
                .to_string()
        });

    // Check for duplicate path.
    let path_check = req.path.clone();
    let duplicate = state.db.with_conn(move |conn| {
        let projects = crate::db::projects::list_projects(conn)?;
        Ok(projects.iter().any(|p| p.path == path_check))
    }).await.unwrap_or(false);
    if duplicate {
        return Json(ApiResponse::err("A project with this path already exists"));
    }

    // If there's a .git, detect remote + branch.
    // Uses sync_cmd to suppress console flash on Windows/Tauri.
    let (repo_url, _branch) = if resolved.join(".git").exists() {
        let path_for_git = req.path.clone();
        let detected = tokio::task::spawn_blocking(move || {
            let remote = sync_cmd("git")
                .args(["remote", "get-url", "origin"])
                .current_dir(&path_for_git)
                .output()
                .ok()
                .and_then(|o| if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else { None });
            let branch = sync_cmd("git")
                .args(["branch", "--show-current"])
                .current_dir(&path_for_git)
                .output()
                .ok()
                .and_then(|o| if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else { None });
            (remote, branch.unwrap_or_else(|| "main".to_string()))
        }).await.unwrap_or((None, "main".to_string()));
        detected
    } else {
        (None, String::new())
    };

    // Detect ai/ configs if present (same patterns as the scan pipeline
    // in core/scanner.rs — kept simple here since we only need the check
    // once and the pattern list is short).
    let ai_configs = {
        use crate::models::AiConfigType;
        let mut found = Vec::new();
        if resolved.join("CLAUDE.md").exists() { found.push(AiConfigType::ClaudeMd); }
        if resolved.join(".claude").is_dir() { found.push(AiConfigType::ClauseDir); }
        if resolved.join(".ai").is_dir() { found.push(AiConfigType::AiDir); }
        if resolved.join(".cursorrules").exists() { found.push(AiConfigType::CursorRules); }
        if resolved.join(".continue").is_dir() { found.push(AiConfigType::ContinueDev); }
        if resolved.join(".mcp.json").exists() { found.push(AiConfigType::McpJson); }
        found
    };

    let now = Utc::now();
    let mut project = Project {
        id: Uuid::new_v4().to_string(),
        name,
        path: req.path.clone(),
        repo_url,
        token_override: None,
        ai_config: AiConfigStatus {
            detected: !ai_configs.is_empty(),
            configs: ai_configs,
        },
        audit_status: AiAuditStatus::NoTemplate,
        ai_todo_count: 0,
            tech_debt_count: 0,
        needs_docs_migration: false,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        linked_repos: vec![],
        created_at: now,
        updated_at: now,
    };
    enrich_audit_status(&mut project);

    let p = project.clone();
    match state.db.with_conn(move |conn| {
        crate::db::projects::insert_project(conn, &p)?;
        Ok(())
    }).await {
        Ok(()) => {
            tracing::info!("Project '{}' added from folder: {}", project.name, project.path);
            Json(ApiResponse::ok(project))
        }
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct DeleteProjectQuery {
    #[serde(default)]
    pub hard: bool,
}

/// DELETE /api/projects/:id
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DeleteProjectQuery>,
) -> Json<ApiResponse<()>> {
    // Fetch project first (needed for hard delete path check)
    let project = if query.hard {
        let pid = id.clone();
        match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &pid)).await {
            Ok(Some(p)) => Some(p),
            Ok(None) => return Json(ApiResponse::err("Project not found")),
            Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
        }
    } else {
        None
    };

    // Hard delete: remove filesystem directory
    if let Some(ref proj) = project {
        let path = scanner::resolve_host_path(&proj.path);

        // Safety guards
        let path_str = path.to_string_lossy();
        if path_str == "/" || path_str == std::env::var("HOME").unwrap_or_default() {
            return Json(ApiResponse::err("Refusing to delete root or home directory"));
        }
        if proj.path.contains("..") {
            return Json(ApiResponse::err("Path contains '..' — refusing to delete"));
        }

        // Verify path is under a known scan path or existing projects' common parent
        let config = state.config.read().await;
        let scan_paths = config.scan.paths.clone();
        drop(config);
        let existing = state.db.with_conn(crate::db::projects::list_projects).await.unwrap_or_default();
        let common_parent = find_common_parent(&existing);

        let path_allowed = scan_paths.iter().any(|sp| proj.path.starts_with(sp))
            || common_parent.as_ref().map(|cp| proj.path.starts_with(cp)).unwrap_or(false);

        if !path_allowed {
            return Json(ApiResponse::err("Project path is not under any scan path or common parent — refusing hard delete"));
        }

        if path.exists() {
            if let Err(e) = std::fs::remove_dir_all(&path) {
                return Json(ApiResponse::err(format!("Failed to remove directory: {}", e)));
            }
        }
    }

    // Delete discussions linked to this project
    if query.hard {
        let pid = id.clone();
        if let Err(e) = state.db.with_conn(move |conn| crate::db::projects::delete_project_discussions(conn, &pid)).await {
            tracing::warn!("Failed to delete project discussions: {}", e);
        }
    }

    // Delete project from DB
    match state.db.with_conn(move |conn| crate::db::projects::delete_project(conn, &id)).await {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PUT /api/projects/:id/default-skills
pub async fn set_default_skills(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(skill_ids): Json<Vec<String>>,
) -> Json<ApiResponse<bool>> {
    let pid = id.clone();
    let sids = skill_ids.clone();
    match state.db.with_conn(move |conn| {
        crate::db::projects::update_project_default_skills(conn, &pid, &sids)
    }).await {
        Ok(true) => {
            // Sync native SKILL.md files to disk (full sync with cleanup)
            let sids2 = skill_ids;
            let pid2 = id;
            let _ = state.db.with_conn(move |conn| {
                if let Ok(Some(project)) = crate::db::projects::get_project(conn, &pid2) {
                    let profile_ids: Vec<String> = project.default_profile_id.iter().cloned().collect();
                    let _ = crate::core::native_files::sync_project_native_files_full(
                        &project.path, &sids2, &profile_ids,
                    );
                }
                Ok::<(), anyhow::Error>(())
            }).await;
            Json(ApiResponse::ok(true))
        }
        Ok(false) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PUT /api/projects/:id/linked-repos — 0.8.3.
/// Replaces the project's `linked_repos` list with the body payload.
/// Validates basic shape (non-empty `name` + `location`, recognized
/// `kind` value). The frontend Settings UI on ProjectCard sends the
/// full list on every edit; we don't expose partial CRUD endpoints
/// for individual links because the list is small (rarely > 5 items)
/// and atomic-replace is simpler than per-row reconciliation.
pub async fn set_linked_repos(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<Vec<LinkedRepo>>,
) -> Json<ApiResponse<bool>> {
    // Allowed kinds — kept loose with a fallback to "other" so a
    // future kind doesn't break existing rows. The frontend picker
    // surfaces these as the canonical set.
    const ALLOWED_KINDS: &[&str] = &["api", "iac", "design", "shared-lib", "docs", "other"];

    for (idx, repo) in payload.iter().enumerate() {
        if repo.name.trim().is_empty() {
            return Json(ApiResponse::err(format!(
                "linked_repos[{idx}] requires a non-empty `name`"
            )));
        }
        if repo.location.trim().is_empty() {
            return Json(ApiResponse::err(format!(
                "linked_repos[{idx}] requires a non-empty `location`"
            )));
        }
        if !ALLOWED_KINDS.contains(&repo.kind.as_str()) {
            return Json(ApiResponse::err(format!(
                "linked_repos[{idx}] has unknown kind `{}` — expected one of: {}",
                repo.kind,
                ALLOWED_KINDS.join(", ")
            )));
        }
    }

    // Cap the list at 20 to keep the prompt prelude bounded.
    if payload.len() > 20 {
        return Json(ApiResponse::err(format!(
            "Too many linked repos ({} ; max 20). A project usually needs 1-5 companions; if you need more, consider grouping them.",
            payload.len()
        )));
    }

    let pid = id.clone();
    let list = payload.clone();
    match state.db.with_conn(move |conn| {
        crate::db::projects::update_project_linked_repos(conn, &pid, &list)
    }).await {
        Ok(true) => {
            // 0.8.4 (#295) — push→pull migration. Auto-write
            // `docs/linked-repos.md` from the canonical list so the
            // agent reads it on-demand instead of receiving the block
            // inlined into EVERY disc/WF system prompt (saves 500-2000
            // tokens/message on chatty sessions). If `docs/` doesn't
            // exist yet (project pre-bootstrap), `sync_linked_repos_doc`
            // is a no-op — the audit Phase 1 will recall it. Log on
            // failure; never block the CRUD response.
            let project_for_doc = state.db.with_conn({
                let pid2 = id.clone();
                move |conn| crate::db::projects::get_project(conn, &pid2)
            }).await;
            if let Ok(Some(project)) = project_for_doc {
                let project_path = crate::core::scanner::resolve_host_path(&project.path);
                if let Err(e) = super::sync_linked_repos_doc(&project_path, &payload) {
                    tracing::warn!(
                        "Failed to sync docs/linked-repos.md for project {} ({}): {} — agent will rely on stale doc until next audit",
                        project.name, id, e
                    );
                }
            }
            Json(ApiResponse::ok(true))
        }
        Ok(false) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PUT /api/projects/:id/default-profile
pub async fn set_default_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Json<ApiResponse<bool>> {
    let profile_id = body.get("profile_id").and_then(|v| v.as_str()).map(|s| s.to_string());
    let pid = id.clone();
    let prof = profile_id.clone();
    match state.db.with_conn(move |conn| {
        crate::db::projects::update_project_default_profile(conn, &pid, prof.as_deref())
    }).await {
        Ok(true) => {
            // Sync native agent files to disk (full sync with cleanup)
            let _ = state.db.with_conn(move |conn| {
                if let Ok(Some(project)) = crate::db::projects::get_project(conn, &id) {
                    let profile_ids: Vec<String> = profile_id.into_iter().collect();
                    let _ = crate::core::native_files::sync_project_native_files_full(
                        &project.path, &project.default_skill_ids, &profile_ids,
                    );
                }
                Ok::<(), anyhow::Error>(())
            }).await;
            Json(ApiResponse::ok(true))
        }
        Ok(false) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/projects/:id/remap-path
pub async fn remap_path(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> Json<ApiResponse<()>> {
    let new_path = match req.get("path").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => return Json(ApiResponse::err("Missing 'path' field".to_string())),
    };

    // Reject traversal first — same reasoning as POST /api/projects.
    if scanner::contains_parent_dir(&new_path) {
        return Json(ApiResponse::err("Path may not contain '..' components".to_string()));
    }

    // Validate path exists
    if !std::path::Path::new(&new_path).exists() {
        return Json(ApiResponse::err("Path does not exist".to_string()));
    }

    let pid = id.clone();
    let np = new_path.clone();
    match state.db.with_conn(move |conn| crate::db::projects::update_project_path(conn, &pid, &np)).await {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err("Project not found".to_string())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}
