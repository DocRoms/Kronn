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
            })
            .await
            .unwrap_or_else(|e| {
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
    match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
        .await
    {
        Ok(Some(mut project)) => tokio::task::spawn_blocking(move || {
            enrich_audit_status(&mut project);
            project
        })
        .await
        .map(|p| Json(ApiResponse::ok(p)))
        .unwrap_or_else(|e| Json(ApiResponse::err(format!("Enrich failed: {e}")))),
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
        fn run_with_timeout(args: &[&str], timeout: Duration) -> Option<std::process::Output> {
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
                stdout
                    .lines()
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
            if let Some(home) =
                directories::UserDirs::new().map(|d| d.home_dir().to_string_lossy().to_string())
            {
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

    let existing_paths: Vec<String> = state
        .db
        .with_conn(|conn| {
            let projects = crate::db::projects::list_projects(conn)?;
            Ok(projects.into_iter().map(|p| p.path).collect())
        })
        .await
        .unwrap_or_default();

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
        return Json(ApiResponse::err(
            "Project path may not contain '..' components",
        ));
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
        path_exists: true,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        linked_repos: vec![],
        created_at: now,
        updated_at: now,
    };
    enrich_audit_status(&mut project);

    let p = project.clone();
    match state
        .db
        .with_conn(move |conn| {
            crate::db::projects::insert_project(conn, &p)?;
            Ok(())
        })
        .await
    {
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
        return Json(ApiResponse::err(
            "Project path may not contain '..' components",
        ));
    }

    let resolved = crate::core::scanner::resolve_host_path(&req.path);
    if !resolved.is_dir() {
        return Json(ApiResponse::err(format!(
            "Directory does not exist: {}",
            resolved.display()
        )));
    }

    // Auto-detect name from last path component if not provided.
    let name = req
        .name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| {
            resolved
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project")
                .to_string()
        });

    // Check for duplicate path.
    let path_check = req.path.clone();
    let duplicate = state
        .db
        .with_conn(move |conn| {
            let projects = crate::db::projects::list_projects(conn)?;
            Ok(projects.iter().any(|p| p.path == path_check))
        })
        .await
        .unwrap_or(false);
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
                .and_then(|o| {
                    if o.status.success() {
                        Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                    } else {
                        None
                    }
                });
            let branch = sync_cmd("git")
                .args(["branch", "--show-current"])
                .current_dir(&path_for_git)
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                    } else {
                        None
                    }
                });
            (remote, branch.unwrap_or_else(|| "main".to_string()))
        })
        .await
        .unwrap_or((None, "main".to_string()));
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
        if resolved.join("CLAUDE.md").exists() {
            found.push(AiConfigType::ClaudeMd);
        }
        if resolved.join(".claude").is_dir() {
            found.push(AiConfigType::ClauseDir);
        }
        if resolved.join(".ai").is_dir() {
            found.push(AiConfigType::AiDir);
        }
        if resolved.join(".cursorrules").exists() {
            found.push(AiConfigType::CursorRules);
        }
        if resolved.join(".continue").is_dir() {
            found.push(AiConfigType::ContinueDev);
        }
        if resolved.join(".mcp.json").exists() {
            found.push(AiConfigType::McpJson);
        }
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
        path_exists: true,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        linked_repos: vec![],
        created_at: now,
        updated_at: now,
    };
    enrich_audit_status(&mut project);

    let p = project.clone();
    match state
        .db
        .with_conn(move |conn| {
            crate::db::projects::insert_project(conn, &p)?;
            Ok(())
        })
        .await
    {
        Ok(()) => {
            tracing::info!(
                "Project '{}' added from folder: {}",
                project.name,
                project.path
            );
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
        match state
            .db
            .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
            .await
        {
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
            return Json(ApiResponse::err(
                "Refusing to delete root or home directory",
            ));
        }
        if proj.path.contains("..") {
            return Json(ApiResponse::err("Path contains '..' — refusing to delete"));
        }

        // Verify path is under a known scan path or existing projects' common parent
        let config = state.config.read().await;
        let scan_paths = config.scan.paths.clone();
        drop(config);
        let existing = state
            .db
            .with_conn(crate::db::projects::list_projects)
            .await
            .unwrap_or_default();
        let common_parent = find_common_parent(&existing);

        let path_allowed = scan_paths.iter().any(|sp| proj.path.starts_with(sp))
            || common_parent
                .as_ref()
                .map(|cp| proj.path.starts_with(cp))
                .unwrap_or(false);

        if !path_allowed {
            return Json(ApiResponse::err(
                "Project path is not under any scan path or common parent — refusing hard delete",
            ));
        }

        if path.exists() {
            if let Err(e) = std::fs::remove_dir_all(&path) {
                return Json(ApiResponse::err(format!(
                    "Failed to remove directory: {}",
                    e
                )));
            }
        }
    }

    // Delete discussions linked to this project
    if query.hard {
        let pid = id.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| crate::db::projects::delete_project_discussions(conn, &pid))
            .await
        {
            tracing::warn!("Failed to delete project discussions: {}", e);
        }
    }

    // Delete project from DB
    match state
        .db
        .with_conn(move |conn| crate::db::projects::delete_project(conn, &id))
        .await
    {
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
    match state
        .db
        .with_conn(move |conn| {
            crate::db::projects::update_project_default_skills(conn, &pid, &sids)
        })
        .await
    {
        Ok(true) => {
            // Sync native SKILL.md files to disk (full sync with cleanup)
            let sids2 = skill_ids;
            let pid2 = id;
            let _ = state
                .db
                .with_conn(move |conn| {
                    if let Ok(Some(project)) = crate::db::projects::get_project(conn, &pid2) {
                        let profile_ids: Vec<String> =
                            project.default_profile_id.iter().cloned().collect();
                        let _ = crate::core::native_files::sync_project_native_files_full(
                            &project.path,
                            &sids2,
                            &profile_ids,
                        );
                    }
                    Ok::<(), anyhow::Error>(())
                })
                .await;
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
    match state
        .db
        .with_conn(move |conn| crate::db::projects::update_project_linked_repos(conn, &pid, &list))
        .await
    {
        Ok(true) => {
            // 0.8.4 (#295) — push→pull migration. Auto-write
            // `docs/linked-repos.md` from the canonical list so the
            // agent reads it on-demand instead of receiving the block
            // inlined into EVERY disc/WF system prompt (saves 500-2000
            // tokens/message on chatty sessions). If `docs/` doesn't
            // exist yet (project pre-bootstrap), `sync_linked_repos_doc`
            // is a no-op — the audit Phase 1 will recall it. Log on
            // failure; never block the CRUD response.
            let project_for_doc = state
                .db
                .with_conn({
                    let pid2 = id.clone();
                    move |conn| crate::db::projects::get_project(conn, &pid2)
                })
                .await;
            if let Ok(Some(project)) = project_for_doc {
                let project_path = crate::core::scanner::resolve_host_path(&project.path);
                if let Err(e) = super::sync_linked_repos_doc(&project_path, &payload) {
                    tracing::warn!(
                        "Failed to sync docs/linked-repos.md for project {} ({}): {} — agent will rely on stale doc until next audit",
                        project.name, id, e
                    );
                }

                // 0.8.6 phase 4 — bidirectional linking. Pre-fix, linking
                // A → B only created the forward link ; B stayed unaware
                // of the relationship until the user manually added the
                // reverse. Now we compute + apply the reverse updates
                // automatically. Idempotent (no-op when B already links
                // back), so no infinite loop even when this triggers a
                // re-sync on B's side.
                let payload_clone = payload.clone();
                let project_for_bidir = project.clone();
                let bidir_result = state
                    .db
                    .with_conn(move |conn| {
                        let all = crate::db::projects::list_projects(conn)?;
                        let updates = compute_bidirectional_link_updates(
                            &project_for_bidir,
                            &payload_clone,
                            &all,
                        );
                        for upd in &updates {
                            crate::db::projects::update_project_linked_repos(
                                conn,
                                &upd.target_project_id,
                                &upd.new_linked_repos,
                            )?;
                        }
                        Ok::<_, anyhow::Error>(updates)
                    })
                    .await;
                if let Ok(updates) = bidir_result {
                    // Push→pull : also refresh docs/linked-repos.md on
                    // each touched reverse-target so the agent there
                    // sees the new sister-project reference on its
                    // next audit / disc.
                    for upd in &updates {
                        let target_id = upd.target_project_id.clone();
                        let target_proj = state
                            .db
                            .with_conn(move |conn| {
                                crate::db::projects::get_project(conn, &target_id)
                            })
                            .await;
                        if let Ok(Some(target)) = target_proj {
                            let target_path = crate::core::scanner::resolve_host_path(&target.path);
                            if let Err(e) =
                                super::sync_linked_repos_doc(&target_path, &upd.new_linked_repos)
                            {
                                tracing::warn!(
                                    "Failed to sync docs/linked-repos.md on reverse target {} ({}): {}",
                                    target.name, upd.target_project_id, e,
                                );
                            }
                        }
                    }
                    if !updates.is_empty() {
                        tracing::info!(
                            "Bidirectional link : {} reverse update(s) applied for source project {}",
                            updates.len(), project.id,
                        );
                    }
                } else if let Err(e) = bidir_result {
                    tracing::warn!(
                        "Bidirectional link compute failed for project {} : {} — forward link still saved, reverse skipped",
                        project.id, e,
                    );
                }
            }
            Json(ApiResponse::ok(true))
        }
        Ok(false) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// 0.8.6 phase 4 — one reverse-link update to apply after a primary
/// `set_linked_repos` write. Returned by [`compute_bidirectional_link_updates`]
/// so the handler can batch them via DB calls.
#[derive(Debug, Clone, PartialEq)]
pub struct BidirectionalLinkUpdate {
    pub target_project_id: String,
    pub new_linked_repos: Vec<LinkedRepo>,
}

/// 0.8.6 phase 4 — compute the reverse-link updates that should be
/// applied after `set_linked_repos` writes project A's new list. For
/// every LinkedRepo in A's payload whose `location` matches another
/// Kronn project B's `path` or `repo_url`, this returns an update
/// adding A as a LinkedRepo on B (if not already present).
///
/// Pure (no DB, no IO) — caller provides the snapshot of all projects.
/// Idempotent : a LinkedRepo from A to B that already has its reverse
/// in B produces NO update (avoids duplicates + breaks the A→B→A loop).
///
/// Semantic decisions :
///   - Reverse-link `kind` defaults to `"other"` (neutral). The user
///     can rename it later in the picker. Source kind isn't copied
///     because the semantic isn't symmetric (an "api" companion for
///     a frontend is a "frontend" companion for the API, not "api").
///   - Reverse-link `description` is empty by default. Same rationale.
///   - Reverse-link `location` is A's `path` if available, else A's
///     `repo_url`. We prefer the path because it works for local-only
///     projects + agents can `cd` to it.
///   - The match in B's existing list checks both `path` and `repo_url`
///     of A — covers projects linked via either form.
pub fn compute_bidirectional_link_updates(
    source_project: &Project,
    source_payload: &[LinkedRepo],
    all_projects: &[Project],
) -> Vec<BidirectionalLinkUpdate> {
    let source_locations: Vec<String> = [
        Some(source_project.path.clone()),
        source_project.repo_url.clone(),
    ]
    .into_iter()
    .flatten()
    .filter(|s| !s.is_empty())
    .collect();
    if source_locations.is_empty() {
        return vec![];
    }

    let mut updates = Vec::new();
    for link in source_payload {
        // Find the Kronn project this link points to (if any).
        let target = all_projects.iter().find(|p| {
            if p.id == source_project.id {
                // A linking to itself : ignore (defensive guard).
                return false;
            }
            // Match against the project's path OR its repo_url. Either
            // form is what the user might have typed in the link picker.
            (!p.path.is_empty() && p.path == link.location)
                || p.repo_url.as_ref().is_some_and(|u| u == &link.location)
        });
        let Some(target) = target else { continue };

        // Already linked back ? Skip — avoids duplicates AND avoids
        // any chance of an infinite ping-pong on subsequent saves.
        let already_linked = target
            .linked_repos
            .iter()
            .any(|existing| source_locations.iter().any(|loc| loc == &existing.location));
        if already_linked {
            continue;
        }

        // Build the reverse link.
        let reverse_location = source_locations[0].clone(); // path-first preference
                                                            // 0.8.6 phase 4 audit feedback (2026-05-22) : populate the
                                                            // reverse-link description with provenance so the user / agent
                                                            // on B's side can tell :
                                                            //   1. This link wasn't typed by hand (`↩ Auto-linked from`)
                                                            //   2. Where it came from (source project name)
                                                            //   3. What semantic role B plays in A's universe (source kind
                                                            //      — "api" / "iac" / "design" / etc.). B's user sees
                                                            //      "Backend is the api for Frontend" without ambiguity.
                                                            //   4. The original description if any (kept verbatim under
                                                            //      "Original:" so context isn't lost).
        let mut description = format!(
            "↩ Auto-linked from {} (original kind: {})",
            source_project.name, link.kind,
        );
        if !link.description.is_empty() {
            description.push_str(&format!(" — original note: \"{}\"", link.description));
        }
        let reverse_link = LinkedRepo {
            id: uuid::Uuid::new_v4().to_string(),
            name: source_project.name.clone(),
            kind: "other".into(),
            location: reverse_location,
            description,
        };
        let mut new_list = target.linked_repos.clone();
        new_list.push(reverse_link);

        // If we already accumulated an update for this same target
        // (e.g. A's payload referenced B twice via path AND repo_url),
        // we'd double-add. Avoid by merging.
        if let Some(existing) = updates
            .iter_mut()
            .find(|u: &&mut BidirectionalLinkUpdate| u.target_project_id == target.id)
        {
            existing.new_linked_repos = new_list;
        } else {
            updates.push(BidirectionalLinkUpdate {
                target_project_id: target.id.clone(),
                new_linked_repos: new_list,
            });
        }
    }
    updates
}

/// 0.8.6 (#27) — One row in the linked-repos picker. Surfaces the
/// minimum needed for autocomplete : project id (for stable React
/// keys), human name, path, and a `proximity_hint` (`'same-parent'` |
/// `'other'`) so the UI can render a "Companion projects" group at
/// the top of the dropdown vs an "Other projects" group at the bottom.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct LinkedRepoCandidate {
    pub id: String,
    pub name: String,
    pub path: String,
    pub proximity_hint: String,
}

/// 0.8.6 (#27) — extract the proximity-sort logic so the test
/// module can lock it independently of the DB layer. Takes a slice
/// of `(id, name, path)` tuples + the current project's path ;
/// returns `LinkedRepoCandidate[]` sorted same-parent-first, then
/// alphabetical (case-insensitive). The current project is filtered
/// out of the output regardless of its position.
pub fn rank_linked_repos_candidates(
    projects: &[(String, String, String)], // (id, name, path)
    current_project_id: &str,
    current_project_path: &str,
) -> Vec<LinkedRepoCandidate> {
    let current_parent = std::path::Path::new(current_project_path)
        .parent()
        .map(|x| x.to_path_buf());

    let mut candidates: Vec<LinkedRepoCandidate> = projects
        .iter()
        .filter(|(id, _, _)| id != current_project_id)
        .map(|(id, name, path)| {
            let parent = std::path::Path::new(path).parent().map(|x| x.to_path_buf());
            let proximity = match (&current_parent, &parent) {
                (Some(a), Some(b)) if a == b => "same-parent",
                _ => "other",
            };
            LinkedRepoCandidate {
                id: id.clone(),
                name: name.clone(),
                path: path.clone(),
                proximity_hint: proximity.to_string(),
            }
        })
        .collect();

    candidates.sort_by(|a, b| {
        // bool < bool : false < true → "not same-parent" goes last.
        let prox_order =
            (a.proximity_hint != "same-parent").cmp(&(b.proximity_hint != "same-parent"));
        prox_order.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    candidates
}

/// `GET /api/projects/:id/linked-repos/candidates`
///
/// 0.8.6 (#27) — returns the list of OTHER Kronn-known projects the
/// user could plausibly link to the current one as a companion. Sorted
/// by proximity : same-parent dir first (the typical "monorepo of
/// repos" pattern : `~/Repositories/front_apollo` linking to
/// `~/Repositories/front_euronews`), then alphabetical fallback.
///
/// The frontend's linked-repos drawer uses this for an autocomplete
/// picker. Manual path entry (off-Kronn repos, remote URLs) stays
/// supported via a free-text fallback.
pub async fn linked_repos_candidates(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<LinkedRepoCandidate>>> {
    let pid = id.clone();
    let res = state
        .db
        .with_conn(move |conn| {
            let all = crate::db::projects::list_projects(conn)?;
            let current = all.iter().find(|p| p.id == pid).cloned();
            let current_path = current.map(|p| p.path).unwrap_or_default();
            let tuples: Vec<(String, String, String)> =
                all.into_iter().map(|p| (p.id, p.name, p.path)).collect();
            Ok::<_, anyhow::Error>(rank_linked_repos_candidates(&tuples, &pid, &current_path))
        })
        .await;

    match res {
        Ok(list) => Json(ApiResponse::ok(list)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {e}"))),
    }
}

/// PUT /api/projects/:id/default-profile
pub async fn set_default_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Json<ApiResponse<bool>> {
    let profile_id = body
        .get("profile_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let pid = id.clone();
    let prof = profile_id.clone();
    match state
        .db
        .with_conn(move |conn| {
            crate::db::projects::update_project_default_profile(conn, &pid, prof.as_deref())
        })
        .await
    {
        Ok(true) => {
            // Sync native agent files to disk (full sync with cleanup)
            let _ = state
                .db
                .with_conn(move |conn| {
                    if let Ok(Some(project)) = crate::db::projects::get_project(conn, &id) {
                        let profile_ids: Vec<String> = profile_id.into_iter().collect();
                        let _ = crate::core::native_files::sync_project_native_files_full(
                            &project.path,
                            &project.default_skill_ids,
                            &profile_ids,
                        );
                    }
                    Ok::<(), anyhow::Error>(())
                })
                .await;
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
        return Json(ApiResponse::err(
            "Path may not contain '..' components".to_string(),
        ));
    }

    // Validate path exists
    if !std::path::Path::new(&new_path).exists() {
        return Json(ApiResponse::err("Path does not exist".to_string()));
    }

    let pid = id.clone();
    let np = new_path.clone();
    match state
        .db
        .with_conn(move |conn| crate::db::projects::update_project_path(conn, &pid, &np))
        .await
    {
        Ok(true) => {
            // Re-sync the project's plugins (MCP configs) + native skill/profile
            // files to the new directory. Pre-fix, a remap only moved the DB
            // pointer — the freshly mapped folder never received the `.mcp.json`
            // / SKILL.md files the project is configured for, so an agent running
            // there saw none of its plugins. Best-effort (logs on failure).
            super::resync_project_assets(&state, &id).await;
            Json(ApiResponse::ok(()))
        }
        Ok(false) => Json(ApiResponse::err("Project not found".to_string())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

#[cfg(test)]
mod linked_repos_candidates_tests {
    use super::*;

    fn proj(id: &str, name: &str, path: &str) -> (String, String, String) {
        (id.to_string(), name.to_string(), path.to_string())
    }

    #[test]
    fn filters_out_the_current_project() {
        let projects = vec![
            proj("p1", "ProjAlpha", "/repos/alpha"),
            proj("p2", "ProjBeta", "/repos/beta"),
        ];
        let result = rank_linked_repos_candidates(&projects, "p1", "/repos/alpha");
        assert_eq!(result.len(), 1, "current project must be excluded");
        assert_eq!(result[0].id, "p2");
    }

    #[test]
    fn ranks_same_parent_companions_first() {
        let projects = vec![
            proj("p-far", "FarRepo", "/elsewhere/far"),
            proj("p-near", "NearRepo", "/repos/near"),
            proj("p-current", "Current", "/repos/current"),
        ];
        let result = rank_linked_repos_candidates(&projects, "p-current", "/repos/current");
        assert_eq!(result.len(), 2);
        // /repos/near shares the parent /repos with /repos/current → first.
        assert_eq!(result[0].id, "p-near");
        assert_eq!(result[0].proximity_hint, "same-parent");
        // /elsewhere/far is unrelated → second, tagged "other".
        assert_eq!(result[1].id, "p-far");
        assert_eq!(result[1].proximity_hint, "other");
    }

    #[test]
    fn alphabetical_tiebreaker_within_same_proximity_bucket() {
        // 3 same-parent companions — sorted case-insensitive alpha.
        let projects = vec![
            proj("p1", "zeta-app", "/repos/zeta-app"),
            proj("p2", "Alpha-API", "/repos/alpha-api"),
            proj("p3", "mid-tier", "/repos/mid-tier"),
            proj("p-cur", "Current", "/repos/current"),
        ];
        let result = rank_linked_repos_candidates(&projects, "p-cur", "/repos/current");
        let names: Vec<&str> = result.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha-API", "mid-tier", "zeta-app"]);
    }

    #[test]
    fn empty_input_returns_empty_list() {
        let result = rank_linked_repos_candidates(&[], "p1", "/repos/p1");
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn current_project_with_no_parent_dir_treats_all_as_other() {
        // Pathological case : a project at "/" — parent is None.
        // Everything else falls in the "other" bucket.
        let projects = vec![
            proj("p1", "Alpha", "/repos/alpha"),
            proj("p-root", "Root", "/"),
        ];
        let result = rank_linked_repos_candidates(&projects, "p-root", "/");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "p1");
        assert_eq!(result[0].proximity_hint, "other");
    }
}

#[cfg(test)]
mod bidirectional_link_tests {
    use super::*;
    use crate::models::{AiAuditStatus, AiConfigStatus};

    fn make_project(id: &str, name: &str, path: &str, repo_url: Option<&str>) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            path: path.into(),
            repo_url: repo_url.map(String::from),
            token_override: None,
            ai_config: AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: AiAuditStatus::NoTemplate,
            ai_todo_count: 0,
            tech_debt_count: 0,
            needs_docs_migration: false,
            path_exists: true,
            default_skill_ids: vec![],
            default_profile_id: None,
            briefing_notes: None,
            linked_repos: vec![],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn make_link(name: &str, location: &str, kind: &str) -> LinkedRepo {
        LinkedRepo {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            kind: kind.into(),
            location: location.into(),
            description: String::new(),
        }
    }

    #[test]
    fn no_update_when_link_does_not_match_any_kronn_project() {
        // Linking to an external repo (Github URL, off-disk path) :
        // the helper finds no Kronn project to update → empty.
        let a = make_project("a", "Frontend", "/repos/frontend", None);
        let payload = vec![make_link(
            "Vendor API",
            "https://github.com/vendor/api",
            "api",
        )];
        let updates = compute_bidirectional_link_updates(&a, &payload, std::slice::from_ref(&a));
        assert_eq!(updates, vec![]);
    }

    #[test]
    fn creates_reverse_link_when_target_is_a_kronn_project_matched_by_path() {
        // Canonical scenario : A's link.location matches B's path.
        // → B gains a reverse link back to A.
        let a = make_project("a", "Frontend", "/repos/frontend", None);
        let b = make_project("b", "Backend", "/repos/backend", None);
        let payload = vec![make_link("Backend API", "/repos/backend", "api")];
        let updates = compute_bidirectional_link_updates(&a, &payload, &[a.clone(), b.clone()]);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].target_project_id, "b");
        // Reverse link points back to A's path with neutral kind.
        let reverse = &updates[0].new_linked_repos[0];
        assert_eq!(reverse.location, "/repos/frontend");
        assert_eq!(reverse.name, "Frontend");
        assert_eq!(reverse.kind, "other");
        // Description encodes provenance so the user/agent on B's side
        // can tell it's auto-created + what role they play for A.
        assert!(reverse.description.contains("Auto-linked from"));
        assert!(reverse.description.contains("Frontend"));
        assert!(
            reverse.description.contains("api"),
            "reverse description must surface the original kind so B's user knows their role for A"
        );
    }

    #[test]
    fn reverse_description_includes_source_link_note_when_provided() {
        // User wrote a meaningful description on A's side
        // ("GraphQL schema lives here") ; the reverse link on B's
        // side preserves it under "original note" so context isn't
        // lost. Without this, B's user sees just "Auto-linked from A".
        let a = make_project("a", "Frontend", "/repos/frontend", None);
        let b = make_project("b", "Backend", "/repos/backend", None);
        let mut link = make_link("Backend API", "/repos/backend", "api");
        link.description = "GraphQL schema lives here".into();
        let payload = vec![link];
        let updates = compute_bidirectional_link_updates(&a, &payload, &[a.clone(), b.clone()]);
        let reverse = &updates[0].new_linked_repos[0];
        assert!(
            reverse.description.contains("GraphQL schema lives here"),
            "original description must be preserved verbatim on the reverse, got: {}",
            reverse.description
        );
        assert!(
            reverse.description.contains("original note"),
            "reverse description must call out the preserved note explicitly"
        );
    }

    #[test]
    fn reverse_description_omits_note_section_when_source_link_has_none() {
        // No description on A's side → the reverse description stays
        // compact (no dangling "original note:" with empty content).
        let a = make_project("a", "Frontend", "/repos/frontend", None);
        let b = make_project("b", "Backend", "/repos/backend", None);
        let payload = vec![make_link("Backend", "/repos/backend", "api")];
        let updates = compute_bidirectional_link_updates(&a, &payload, &[a.clone(), b.clone()]);
        let reverse = &updates[0].new_linked_repos[0];
        assert!(
            !reverse.description.contains("original note"),
            "should not show empty 'original note' when source had no description"
        );
    }

    #[test]
    fn matches_target_by_repo_url_when_path_does_not_match() {
        // User types a Github URL in the picker ; B is registered in
        // Kronn with that same repo_url. The helper matches and
        // creates the reverse.
        let a = make_project("a", "Frontend", "/repos/frontend", None);
        let mut b = make_project(
            "b",
            "Backend",
            "/repos/backend",
            Some("git@github.com:org/backend.git"),
        );
        b.path = "/different/path/backend".into(); // path doesn't match the link
        let payload = vec![make_link(
            "Backend",
            "git@github.com:org/backend.git",
            "api",
        )];
        let updates = compute_bidirectional_link_updates(&a, &payload, &[a.clone(), b.clone()]);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].target_project_id, "b");
    }

    #[test]
    fn idempotent_when_reverse_link_already_exists() {
        // Critical : if B already links back to A (manual add by user
        // OR result of a previous bidirectional pass), the helper
        // MUST skip — otherwise every save would duplicate the entry,
        // and the A→B→A→B chain could ping-pong forever.
        let a = make_project("a", "Frontend", "/repos/frontend", None);
        let mut b = make_project("b", "Backend", "/repos/backend", None);
        b.linked_repos = vec![make_link("Frontend", "/repos/frontend", "other")];
        let payload = vec![make_link("Backend", "/repos/backend", "api")];
        let updates = compute_bidirectional_link_updates(&a, &payload, &[a.clone(), b.clone()]);
        assert!(
            updates.is_empty(),
            "MUST not duplicate when reverse already present — got: {:?}",
            updates
        );
    }

    #[test]
    fn idempotent_when_reverse_link_uses_repo_url_form() {
        // Variant of the above : B links back via A's repo_url
        // instead of path. Still no duplicate.
        let a = make_project(
            "a",
            "Frontend",
            "/repos/frontend",
            Some("git@github.com:org/front.git"),
        );
        let mut b = make_project("b", "Backend", "/repos/backend", None);
        b.linked_repos = vec![make_link(
            "Frontend",
            "git@github.com:org/front.git",
            "other",
        )];
        let payload = vec![make_link("Backend", "/repos/backend", "api")];
        let updates = compute_bidirectional_link_updates(&a, &payload, &[a.clone(), b.clone()]);
        assert!(updates.is_empty());
    }

    #[test]
    fn skips_self_reference_defensively() {
        // If a user types their OWN path in the picker, the link
        // would match "themselves". Defensive guard refuses to
        // create a self-reverse (silly + would infinite-loop on save).
        let a = make_project("a", "Self", "/repos/me", None);
        let payload = vec![make_link("Me", "/repos/me", "other")];
        let updates = compute_bidirectional_link_updates(&a, &payload, std::slice::from_ref(&a));
        assert_eq!(updates, vec![]);
    }

    #[test]
    fn multi_targets_create_multi_updates() {
        // A links to B AND C — both Kronn projects → 2 separate
        // updates, each adding A on the respective target.
        let a = make_project("a", "Frontend", "/repos/frontend", None);
        let b = make_project("b", "Backend", "/repos/backend", None);
        let c = make_project("c", "Designs", "/repos/designs", None);
        let payload = vec![
            make_link("Backend", "/repos/backend", "api"),
            make_link("Designs", "/repos/designs", "design"),
        ];
        let updates =
            compute_bidirectional_link_updates(&a, &payload, &[a.clone(), b.clone(), c.clone()]);
        assert_eq!(updates.len(), 2);
        let target_ids: Vec<&str> = updates
            .iter()
            .map(|u| u.target_project_id.as_str())
            .collect();
        assert!(target_ids.contains(&"b"));
        assert!(target_ids.contains(&"c"));
    }

    #[test]
    fn source_path_preferred_over_repo_url_for_reverse_location() {
        // When A has both a path AND repo_url, the reverse link uses
        // the path (works for local + remote ; agents can `cd` to it).
        let a = make_project(
            "a",
            "Frontend",
            "/repos/frontend",
            Some("git@github.com:org/front.git"),
        );
        let b = make_project("b", "Backend", "/repos/backend", None);
        let payload = vec![make_link("Backend", "/repos/backend", "api")];
        let updates = compute_bidirectional_link_updates(&a, &payload, &[a.clone(), b.clone()]);
        let reverse = &updates[0].new_linked_repos[0];
        assert_eq!(
            reverse.location, "/repos/frontend",
            "path-first preference broken — reverse link should use path when available"
        );
    }

    #[test]
    fn fallbacks_to_repo_url_when_source_has_no_path() {
        // Edge : A is a virtual / cloud-only project with no on-disk
        // path. Reverse uses the repo_url so the link still points
        // somewhere meaningful.
        let mut a = make_project("a", "Cloud", "", Some("git@github.com:org/cloud.git"));
        a.path = String::new();
        let b = make_project("b", "Backend", "/repos/backend", None);
        let payload = vec![make_link("Backend", "/repos/backend", "api")];
        let updates = compute_bidirectional_link_updates(&a, &payload, &[a.clone(), b.clone()]);
        assert_eq!(updates.len(), 1);
        assert_eq!(
            updates[0].new_linked_repos[0].location,
            "git@github.com:org/cloud.git"
        );
    }

    #[test]
    fn no_update_when_source_has_neither_path_nor_repo_url() {
        // Degenerate case : nothing to link back TO. Returns empty
        // rather than creating a link with an empty location (which
        // would be invalid against the picker's allowlist).
        let mut a = make_project("a", "Ghost", "", None);
        a.path = String::new();
        let b = make_project("b", "Backend", "/repos/backend", None);
        let payload = vec![make_link("Backend", "/repos/backend", "api")];
        let updates = compute_bidirectional_link_updates(&a, &payload, &[a.clone(), b.clone()]);
        assert!(updates.is_empty());
    }

    #[test]
    fn preserves_existing_links_on_the_target() {
        // B already has a link to C unrelated to A. When A links to
        // B, the reverse update PRESERVES B's existing link to C +
        // appends A. Never wipes.
        let a = make_project("a", "Frontend", "/repos/frontend", None);
        let mut b = make_project("b", "Backend", "/repos/backend", None);
        b.linked_repos = vec![make_link("Designs", "/repos/designs", "design")];
        let payload = vec![make_link("Backend", "/repos/backend", "api")];
        let updates = compute_bidirectional_link_updates(&a, &payload, &[a.clone(), b.clone()]);
        assert_eq!(updates.len(), 1);
        let new_list = &updates[0].new_linked_repos;
        assert_eq!(new_list.len(), 2);
        assert!(
            new_list.iter().any(|l| l.location == "/repos/designs"),
            "existing C link must be preserved"
        );
        assert!(
            new_list.iter().any(|l| l.location == "/repos/frontend"),
            "new A link must be appended"
        );
    }
}
