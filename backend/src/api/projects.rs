use std::convert::Infallible;
use std::pin::Pin;

use axum::{
    extract::{Path, Query, State},
    response::sse::{Event, Sse},
    Json,
};
use chrono::Utc;
use futures::Stream;
use uuid::Uuid;

use crate::agents::runner;
use crate::core::scanner;
use crate::models::*;
use crate::AppState;

type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

/// Populate audit_status and ai_todo_count on a project (computed from filesystem)
fn enrich_audit_status(project: &mut Project) {
    project.audit_status = scanner::detect_audit_status(&project.path);
    project.ai_todo_count = scanner::count_ai_todos(&project.path);
}

/// GET /api/projects
pub async fn list(State(state): State<AppState>) -> Json<ApiResponse<Vec<Project>>> {
    match state.db.with_conn(crate::db::projects::list_projects).await {
        Ok(mut projects) => {
            for p in &mut projects {
                enrich_audit_status(p);
            }
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
    match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(mut p)) => {
            enrich_audit_status(&mut p);
            Json(ApiResponse::ok(p))
        }
        Ok(None) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/projects/scan
pub async fn scan(State(state): State<AppState>) -> Json<ApiResponse<Vec<DetectedRepo>>> {
    let config = state.config.read().await;

    let scan_paths = if config.scan.paths.is_empty() {
        std::env::var("KRONN_HOST_HOME")
            .into_iter()
            .collect::<Vec<_>>()
    } else {
        config.scan.paths.clone()
    };
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
        default_skill_ids: vec![],
        default_profile_id: None,
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

/// Determine the parent directory for new projects (shared between bootstrap and clone).
async fn determine_parent_dir(state: &AppState) -> Result<String, String> {
    let existing = state.db.with_conn(crate::db::projects::list_projects).await.unwrap_or_default();
    if let Some(common) = find_common_parent(&existing) {
        Ok(common)
    } else if let Ok(repos_dir) = std::env::var("KRONN_REPOS_DIR") {
        Ok(repos_dir)
    } else {
        let config = state.config.read().await;
        match config.scan.paths.first().cloned() {
            Some(p) => Ok(p),
            None => Err("No scan path configured and no existing projects.".to_string()),
        }
    }
}

/// POST /api/projects/bootstrap
/// Create a new project from scratch: create dir, git init, install template, create bootstrap discussion.
pub async fn bootstrap(
    State(state): State<AppState>,
    Json(req): Json<BootstrapProjectRequest>,
) -> Json<ApiResponse<BootstrapProjectResponse>> {
    // 1. Determine parent directory: use the common parent of existing projects,
    // or fall back to KRONN_REPOS_DIR env var, or first scan path.
    // We need a writable directory — KRONN_HOST_HOME is mounted read-only.
    let parent_dir = match determine_parent_dir(&state).await {
        Ok(d) => d,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let project_name = req.name.trim().to_string();
    if project_name.is_empty() {
        return Json(ApiResponse::err("Project name is required"));
    }

    // Sanitize name for directory (kebab-case)
    let dir_name: String = project_name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if dir_name.is_empty() {
        return Json(ApiResponse::err("Invalid project name"));
    }

    let project_path_str = format!("{}/{}", parent_dir.trim_end_matches('/'), dir_name);
    let description = req.description.clone();
    let agent_type = req.agent;

    // 2. Create directory + git init on blocking thread
    let setup_result = tokio::task::spawn_blocking({
        let parent = parent_dir.clone();
        let dirname = dir_name.clone();
        move || -> Result<(), String> {
            // Resolve the parent dir (which exists) then append the new dir name
            let parent_resolved = scanner::resolve_host_path(&parent);
            if !parent_resolved.exists() {
                return Err(format!("Parent directory not found: {}", parent_resolved.display()));
            }
            let project_path = parent_resolved.join(&dirname);
            if project_path.exists() {
                return Err(format!("Directory already exists: {}", project_path.display()));
            }
            std::fs::create_dir_all(&project_path)
                .map_err(|e| format!("Failed to create directory: {}", e))?;

            // git init
            let status = std::process::Command::new("git")
                .arg("init")
                .current_dir(&project_path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map_err(|e| format!("git init failed: {}", e))?;
            if !status.success() {
                return Err("git init failed".into());
            }

            // Install template
            let template_dir = resolve_templates_dir();
            if template_dir.exists() {
                let ai_template = template_dir.join("ai");
                let ai_target = project_path.join("ai");
                if ai_template.is_dir() {
                    copy_dir_nondestructive(&ai_template, &ai_target)?;
                }
                for filename in &["CLAUDE.md", ".cursorrules", ".windsurfrules", ".clinerules"] {
                    let src = template_dir.join(filename);
                    let dst = project_path.join(filename);
                    if src.exists() && !dst.exists() {
                        let _ = std::fs::copy(&src, &dst);
                    }
                }
            }

            runner::fix_file_ownership(&project_path);
            Ok(())
        }
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = setup_result {
        return Json(ApiResponse::err(e));
    }

    // 3. Create project in DB
    let now = Utc::now();
    let project_id = Uuid::new_v4().to_string();
    let mut project = Project {
        id: project_id.clone(),
        name: project_name.clone(),
        path: project_path_str.clone(),
        repo_url: None,
        token_override: None,
        ai_config: AiConfigStatus {
            detected: false,
            configs: vec![],
        },
        audit_status: AiAuditStatus::NoTemplate,
        ai_todo_count: 0,
        default_skill_ids: vec![],
        default_profile_id: None,
        created_at: now,
        updated_at: now,
    };
    enrich_audit_status(&mut project);

    let p = project.clone();
    let mcp_ids = req.mcp_config_ids.clone();
    if let Err(e) = state.db.with_conn(move |conn| {
        crate::db::projects::insert_project(conn, &p)?;
        // Link selected MCP configs to the new project
        for mcp_id in &mcp_ids {
            crate::db::mcps::link_config_project(conn, mcp_id, &p.id)?;
        }
        Ok(())
    }).await {
        return Json(ApiResponse::err(format!("DB error: {}", e)));
    }

    // Sync .mcp.json for the new project if MCPs were linked
    if !req.mcp_config_ids.is_empty() {
        let config = state.config.read().await;
        if let Some(ref secret) = config.encryption_secret {
            let secret = secret.clone();
            let pid = project_id.clone();
            let _ = state.db.with_conn(move |conn| {
                crate::core::mcp_scanner::sync_affected_projects(conn, &[pid], &secret);
                Ok::<_, anyhow::Error>(())
            }).await;
        }
    }

    // 4. Get language
    let language = {
        let config = state.config.read().await;
        config.language.clone()
    };

    // 5. Build bootstrap discussion prompt
    let bootstrap_prompt = build_bootstrap_prompt(&language, &project_name, &description);

    let discussion_id = Uuid::new_v4().to_string();
    let initial_message = DiscussionMessage {
        id: Uuid::new_v4().to_string(),
        role: MessageRole::User,
        content: bootstrap_prompt,
        agent_type: None,
        timestamp: now,
        tokens_used: 0,
        auth_mode: None,
    };

    let discussion = Discussion {
        id: discussion_id.clone(),
        project_id: Some(project_id.clone()),
        title: format!("Bootstrap: {}", project_name),
        agent: agent_type.clone(),
        language: language.clone(),
        participants: vec![agent_type],
        messages: vec![initial_message.clone()],
        message_count: 1,
        skill_ids: vec![],
        profile_ids: vec![
            "architect".into(),
            "product-owner".into(),
        ],
        directive_ids: vec![],
        archived: false,
        created_at: now,
        updated_at: now,
    };

    let disc = discussion.clone();
    let msg = initial_message;
    if let Err(e) = state.db.with_conn(move |conn| {
        crate::db::discussions::insert_discussion(conn, &disc)?;
        crate::db::discussions::insert_message(conn, &disc.id, &msg)?;
        Ok(())
    }).await {
        return Json(ApiResponse::err(format!("Failed to create discussion: {}", e)));
    }

    Json(ApiResponse::ok(BootstrapProjectResponse {
        project_id,
        discussion_id,
    }))
}

/// Build the bootstrap discussion prompt
fn build_bootstrap_prompt(language: &str, project_name: &str, description: &str) -> String {
    let lang_instruction = match language {
        "fr" => "Réponds en français.",
        "es" => "Responde en español.",
        _ => "Respond in English.",
    };

    format!(
r#"# Bootstrap du projet "{project_name}"

{lang_instruction}

## Description du projet
{description}

## Ta mission
Tu es un architecte logiciel et product owner. Tu dois m'aider à construire ce projet de zéro, étape par étape.

Commence par analyser la description ci-dessus, puis guide-moi à travers les étapes suivantes :

### 1. Vision & Objectifs
- Reformule la vision du projet en 2-3 phrases claires
- Identifie les utilisateurs cibles
- Liste les 3-5 objectifs principaux

### 2. Architecture technique
- Propose un stack technique adapté (frontend, backend, DB, infra)
- Justifie chaque choix
- Dessine l'architecture en ASCII si pertinent

### 3. Structure du projet
- Propose une arborescence de fichiers/dossiers
- Explique les conventions de nommage

### 4. MVP — Features prioritaires
- Liste les features pour un MVP fonctionnel
- Priorise-les (P0 = indispensable, P1 = important, P2 = nice-to-have)
- Estime la complexité relative de chaque feature

### 5. Plan d'action
- Propose un plan de développement séquentiel
- Identifie les dépendances entre features
- Suggère les premiers fichiers à créer

### 6. Finalisation
- Quand tu as terminé toutes les étapes, écris exactement `KRONN:BOOTSTRAP_COMPLETE` dans ton dernier message.

Commence maintenant par l'étape 1. Pose-moi des questions si la description manque de détails."#
    )
}

/// POST /api/projects/clone
pub async fn clone_project(
    State(state): State<AppState>,
    Json(req): Json<CloneProjectRequest>,
) -> Json<ApiResponse<CloneProjectResponse>> {
    let url = req.url.trim().to_string();
    if url.is_empty() {
        return Json(ApiResponse::err("Repository URL is required"));
    }

    // Extract name from URL: last segment, remove .git suffix
    let repo_name = req.name.as_deref()
        .filter(|n| !n.trim().is_empty())
        .map(|n| n.trim().to_string())
        .unwrap_or_else(|| {
            url.trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("repo")
                .trim_end_matches(".git")
                .to_string()
        });

    if repo_name.is_empty() {
        return Json(ApiResponse::err("Could not determine repository name from URL"));
    }

    // Determine parent directory (same logic as bootstrap)
    let parent_dir = determine_parent_dir(&state).await;
    let parent_dir = match parent_dir {
        Ok(d) => d,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    // Sanitize name for directory (kebab-case)
    let dir_name: String = repo_name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    let project_path = format!("{}/{}", parent_dir, dir_name);
    // Resolve the *parent* directory (which exists) to get the correct Docker mount path,
    // then append the dir name. resolve_host_path on the full path would fail because
    // the target doesn't exist yet and the exists() check would fall through to the raw host path.
    let resolved_parent = scanner::resolve_host_path(&parent_dir);
    let host_path = resolved_parent.join(&dir_name);

    if host_path.exists() {
        return Json(ApiResponse::err(format!("Directory already exists: {}", project_path)));
    }

    // Git clone
    let clone_url = url.clone();
    let clone_path = host_path.clone();
    let clone_result = tokio::task::spawn_blocking(move || {
        std::process::Command::new("git")
            .args(["clone", &clone_url, &clone_path.to_string_lossy()])
            .output()
    }).await;

    match clone_result {
        Ok(Ok(output)) if !output.status.success() => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Json(ApiResponse::err(format!("git clone failed: {}", stderr.trim())));
        }
        Ok(Err(e)) => return Json(ApiResponse::err(format!("Failed to run git: {}", e))),
        Err(e) => return Json(ApiResponse::err(format!("Task failed: {}", e))),
        _ => {} // success
    }

    // Create project in DB
    let project_id = Uuid::new_v4().to_string();
    let now = Utc::now();
    let mut project = Project {
        id: project_id.clone(),
        name: repo_name.clone(),
        path: project_path.clone(),
        repo_url: Some(url),
        token_override: None,
        ai_config: AiConfigStatus {
            detected: false,
            configs: vec![],
        },
        audit_status: crate::models::AiAuditStatus::default(),
        ai_todo_count: 0,
        default_skill_ids: vec![],
        default_profile_id: None,
        created_at: now,
        updated_at: now,
    };
    enrich_audit_status(&mut project);

    let p = project.clone();
    if let Err(e) = state.db.with_conn(move |conn| crate::db::projects::insert_project(conn, &p)).await {
        return Json(ApiResponse::err(format!("DB error: {}", e)));
    }

    // Auto-detect skills
    let detected = detect_project_skills(&host_path);
    if !detected.is_empty() {
        let pid = project_id.clone();
        let skills = detected.clone();
        let _ = state.db.with_conn(move |conn| {
            crate::db::projects::update_project_default_skills(conn, &pid, &skills)
        }).await;
    }

    Json(ApiResponse::ok(CloneProjectResponse {
        project_id,
        discussion_id: None,
    }))
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

// ─── Template installation ──────────────────────────────────────────────────

/// POST /api/projects/:id/install-template
/// Copies the AI template files into the project directory.
pub async fn install_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    // Run filesystem I/O on blocking thread pool
    let install_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        if !project_path.exists() {
            return Err(format!("Project path not found: {}", project_path.display()));
        }

        let ai_target = project_path.join("ai");
        if ai_target.exists() {
            if let Err(e) = check_ai_dir_permissions(&ai_target) {
                return Err(format!(
                    "ai/ directory exists but has permission issues that could not be fixed: {}. \
                     Run: sudo chown -R $(id -u):$(id -g) {}/ai/",
                    e, project_path.display()
                ));
            }
        }

        let template_dir = resolve_templates_dir();
        if !template_dir.exists() {
            return Err(format!("Templates directory not found: {}", template_dir.display()));
        }

        let ai_template = template_dir.join("ai");
        if ai_template.is_dir() {
            copy_dir_nondestructive(&ai_template, &ai_target)?;
        }

        for filename in &["CLAUDE.md", ".cursorrules", ".windsurfrules", ".clinerules"] {
            let src = template_dir.join(filename);
            let dst = project_path.join(filename);
            if src.exists() && !dst.exists() {
                if let Err(e) = std::fs::copy(&src, &dst) {
                    tracing::warn!("Failed to copy {}: {}", filename, e);
                }
            }
        }

        let index_file = project_path.join("ai/index.md");
        if index_file.exists() {
            inject_bootstrap_prompt(&index_file);
        }

        runner::fix_file_ownership(&project_path);

        Ok(())
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = install_result {
        return Json(ApiResponse::err(e));
    }

    // Ensure ai/ files are in .gitignore
    crate::core::mcp_scanner::ensure_gitignore_public(&project.path, "ai/var/");

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

/// Resolve the templates directory (Docker mount or local)
fn resolve_templates_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("KRONN_TEMPLATES_DIR") {
        return std::path::PathBuf::from(dir);
    }
    // Docker default
    let docker_path = std::path::PathBuf::from("/app/templates");
    if docker_path.exists() {
        return docker_path;
    }
    // Local dev fallback: relative to binary
    std::path::PathBuf::from("templates")
}

/// Recursively copy a directory, skipping files that already exist at the destination.
fn copy_dir_nondestructive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {}", dst.display(), e))?;

    let entries = std::fs::read_dir(src)
        .map_err(|e| format!("read_dir {}: {}", src.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("entry: {}", e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_nondestructive(&src_path, &dst_path)?;
        } else if !dst_path.exists() {
            std::fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("copy {} -> {}: {}", src_path.display(), dst_path.display(), e))?;
        }
    }
    Ok(())
}

/// Inject the bootstrap prompt at the top of ai/index.md
fn inject_bootstrap_prompt(index_file: &std::path::Path) {
    let content = match std::fs::read_to_string(index_file) {
        Ok(c) => c,
        Err(_) => return,
    };

    // Already has bootstrap prompt?
    if content.contains("KRONN:BOOTSTRAP:START") {
        return;
    }

    let prompt = r#"<!-- KRONN:BOOTSTRAP:START -->
<!-- ================================================================
     BOOTSTRAP INSTRUCTIONS — AUTO-GENERATED BY KRONN
     ================================================================
     This block is for AI agents only. It instructs you to analyze
     this repository and fill in the ai/ documentation skeleton.

     After completing the analysis, you MUST delete this entire block
     (from KRONN:BOOTSTRAP:START to KRONN:BOOTSTRAP:END).
     ================================================================ -->

> **FIRST-RUN TASK — Bootstrap ai/ documentation**
>
> This is a fresh `ai/` skeleton. You must analyze the repository and fill in all files.
>
> **Rules:**
> - All `ai/` files MUST be in **English**
> - Content is **AI context** (factual, concise), NOT human documentation
> - Do NOT invent information — mark unknowns with `<!-- TODO: verify -->`
> - Replace ALL `{{PLACEHOLDERS}}` and `<!-- ... -->` comment placeholders with real content
> - Keep the existing file structure and headings — fill the blanks, do NOT rewrite from scratch
>
> **Steps (in order):**
>
> 1. **`ai/index.md`** — Analyze the project (README, configs, CI). Fill: project name, stack,
>    common tasks table, prerequisites, DO NOT rules, source of truth, code placement, stack table, date.
>
> 2. **`ai/glossary.md`** — Extract domain terms, abbreviations, internal names.
>    Organize by category (Architecture, Domain, Business, Third Parties). 30-60 terms.
>    Mark unknown terms with `<!-- TODO: ask user -->` for validation phase.
>
> 3. **`ai/repo-map.md`** — Map folder structure (2-3 levels), key files, entry points.
>
> 4. **`ai/coding-rules.md`** — One section per language. Linters, formatters, conventions, commands.
>
> 5. **`ai/testing-quality.md`** — Test frameworks, commands, CI gates, test file list, coverage.
>
> 6. **`ai/architecture/overview.md`** — Services table, key patterns, data flow, separation of concerns.
>
> 7. **`ai/operations/debug-operations.md`** — Common commands, Docker services, troubleshooting.
>
> 8. **`ai/operations/mcp-servers.md`** — MCP servers if .mcp.json exists.
>    Only create `ai/operations/mcp-servers/<slug>.md` if there are project-specific rules to document.
>
> 9. **`ai/inconsistencies-tech-debt.md`** — Real issues found during analysis.
>    Create `ai/tech-debt/TD-*.md` detail files for each entry.
>    Flag outdated prerequisites (old runtimes, deprecated dependencies, obsolete tooling).
>
> 10. **Review** — Check all files for consistency, completeness, no remaining placeholders.
>
> 11. **DELETE THIS ENTIRE BLOCK** (from `KRONN:BOOTSTRAP:START` to `KRONN:BOOTSTRAP:END`).
>
> 12. **Signal completion** — Write exactly `KRONN:BOOTSTRAP_COMPLETE` in your final message.
>
> When done, summarize: files filled, items needing human input, suggested deep-dives.

<!-- KRONN:BOOTSTRAP:END -->

"#;

    let new_content = format!("{}{}", prompt, content);
    if let Err(e) = std::fs::write(index_file, new_content) {
        tracing::warn!("Failed to inject bootstrap prompt: {}", e);
    }
}

// ─── AI Audit (SSE streaming) ───────────────────────────────────────────────

const PROMPT_PREAMBLE: &str = "\
Rules: Write in English. Be factual and concise — this is AI context for coding agents, NOT human documentation.\n\
- Do NOT invent information — mark unknowns with `<!-- TODO: verify -->`.\n\
- Replace ALL `{{PLACEHOLDERS}}` and `<!-- ... -->` comment placeholders with real content.\n\
- Keep the existing file structure and section headings — fill in the blanks, do NOT rewrite the file from scratch.\n\
- Write plain facts, not opinions or recommendations. No debate, no trade-offs analysis.\n\
- Each section should be self-contained: another AI agent reading just that section should get the full picture.";

const ANALYSIS_STEPS: &[(&str, &str)] = &[
    // Step 1: Project analysis + index
    ("ai/index.md", "\
Read README.md, package.json (or composer.json, Cargo.toml, go.mod), Makefile, Dockerfile, docker-compose.yml, \
CI configs (.github/workflows, .gitlab-ci.yml), and main config files.\n\
Determine: tech stack, project structure, build/dev/test commands, key patterns, third-party services, CI/CD pipeline.\n\n\
Then fill ai/index.md — replace ALL {{PLACEHOLDERS}} in each section:\n\
- {{PROJECT_NAME}} and {{STACK_SUMMARY}}: project name and one-line stack description\n\
- Common tasks table: replace {{TASK_EXAMPLE_*}} with 5-7 real task→file mappings\n\
- Prerequisites table: replace {{PREREQ_*}} with Docker, language versions, build commands\n\
- DO NOT rules: replace {{DO_NOT_*}} with 3+ project-specific rules\n\
- Source of truth table: replace {{SOURCE_*}} with key config files (models, routes, DB schema, types)\n\
- Code placement table: replace {{CODE_TYPE_*}} with where to put new endpoints, pages, tests\n\
- Stack table: replace {{TECH_*}} with all major technologies, versions, and roles\n\
- Workflow constraints: replace {{WORKFLOW_CONSTRAINT_*}} with project-specific rules\n\
- {{DATE}}: set to today's date (YYYY-MM-DD)"),

    // Step 2: Glossary (early — defines vocabulary for subsequent steps)
    ("ai/glossary.md", "\
Read ai/index.md for project context. Search the codebase for domain-specific terms, abbreviations, \
internal naming conventions, and project jargon.\n\n\
Fill ai/glossary.md — replace ALL {{PLACEHOLDERS}} in the tables:\n\
- Organize terms by category: Architecture/Stack, Domain/Business, Environments, Third Parties, Abbreviations\n\
- Each term: one-line definition + optional link to relevant ai/ file\n\
- Include: framework-specific terms, model names, service names, acronyms used in code\n\
- Aim for 30-60 terms total across all categories\n\
- IMPORTANT: if you encounter a domain term you don't understand, add it with `<!-- TODO: ask user -->` \
after the definition. Example: `| Widget | Some kind of business entity <!-- TODO: ask user --> | |`\n\
- These TODO markers will be reviewed during the validation phase so the user can clarify them"),

    // Step 3: Repo map
    ("ai/repo-map.md", "\
Read ai/index.md and ai/glossary.md for context. Explore the directory structure (2-3 levels deep).\n\n\
Fill ai/repo-map.md — replace ALL {{PLACEHOLDERS}}:\n\
- {{STACK_OVERVIEW}}: one paragraph summarizing the architecture\n\
- Key folders tree: replace {{FOLDER_*}} and {{SUBFOLDER_*}} with every major directory (2-3 levels deep)\n\
  Use the tree-like format already in the file with inline annotations\n\
- Primary entrypoints table: replace {{ENTRYPOINT_*}} with 5-7 key files (main config, routes, models, etc.)\n\
- Auto-generated files table: replace {{FILE_PATTERN}} with files that should NOT be edited manually\n\
- Note any auto-generated files that should NOT be edited manually"),

    // Step 4: Coding rules
    ("ai/coding-rules.md", "\
Read ai/index.md for context. Find ALL linter, formatter, and type-checker configs in the repo \
(e.g. .eslintrc, eslint.config.js, prettier, rustfmt.toml, tsconfig.json, phpcs.xml, etc.).\n\n\
Fill ai/coding-rules.md — replace ALL {{PLACEHOLDERS}}:\n\
- Replace {{LANGUAGE_*}} with one section per language/framework used in the project\n\
- For each language, fill the Tools table: {{CONFIG}} and {{COMMAND}} for linter, formatter, type checker\n\
- Replace {{CONVENTION_*}} with 5-10 coding conventions per language (naming, error handling, imports)\n\
- Replace {{MISTAKE_*}} with common mistakes to avoid (linter patterns, framework gotchas)\n\
- Add or remove language sections as needed to match the actual project stack"),

    // Step 5: Testing & quality
    ("ai/testing-quality.md", "\
Read ai/index.md for context. Find test framework configs (jest, vitest, phpunit, pytest, cargo test, bats, etc.) \
and CI quality gates.\n\n\
Fill ai/testing-quality.md — replace ALL {{PLACEHOLDERS}}:\n\
- Build & quality checks table: replace {{CHECK_*}} and {{COMMAND}} with all quality checks (compile, lint, format, test, build)\n\
- Test infrastructure table: replace {{LANG_*}}, {{RUNNER}}, {{CONFIG}} for each language\n\
- Test suites table: replace {{SUITE_*}} with test files/suites and approximate counts\n\
- Coverage: replace {{COVERAGE_STATUS}} and {{COVERAGE_TARGET}} with current status and targets\n\
- Replace {{UNTESTED_*}} with components that have NO tests\n\
- Fast smoke checks table: replace {{COMMAND_*}} with 3-5 quick pre-commit commands"),

    // Step 6: Architecture overview
    ("ai/architecture/overview.md", "\
Read ai/index.md and ai/repo-map.md for context. Analyze the high-level architecture.\n\n\
Fill ai/architecture/overview.md — replace ALL {{PLACEHOLDERS}}:\n\
- Apps/services table: replace {{SERVICE_*}}, {{PORT}}, {{TECH}}, {{ROLE}} for each service\n\
- Key patterns: replace {{PATTERN_*_NAME}} and {{PATTERN_*_DESCRIPTION}} with 3-5 architectural patterns \
  (API pattern, state management, auth, data flow, caching, etc.) — 2-3 sentences each\n\
- {{SEPARATION_DESCRIPTION}}: how the codebase is organized (by feature, by layer, etc.)\n\
- Data flow: replace {{DATA_FLOW_DIAGRAM}} with ASCII flow diagram and {{DATA_FLOW_DESCRIPTION}}\n\
- Legacy table: replace {{AREA}}, {{CURRENT}}, {{TARGET}} for any legacy patterns or planned migrations"),

    // Step 7: Debug operations
    ("ai/operations/debug-operations.md", "\
Read ai/index.md for context. Find operational commands from Makefile, package.json scripts, \
docker-compose commands, and any run/build/debug procedures.\n\n\
Fill ai/operations/debug-operations.md — replace ALL {{PLACEHOLDERS}}:\n\
- Common commands table: replace {{ACTION_*}} and {{COMMAND_*}} for start, stop, logs, test, build, deploy\n\
- Docker services table: replace {{SERVICE_*}}, {{PORT}}, {{ROLE}}, {{HEALTH}} for each container\n\
- Troubleshooting: replace {{ISSUE_*_TITLE}}, {{SYMPTOM}}, {{CAUSE}}, {{FIX}} with 3-5 common issues"),

    // Step 8: MCP servers overview
    ("ai/operations/mcp-servers.md", "\
Read ai/index.md for context. Check if .mcp.json or .mcp.json.example or .env.mcp.example exists in the repo.\n\n\
If MCP config exists:\n\
- Document each configured MCP server in ai/operations/mcp-servers.md\n\
- For each server: name, transport type, what it's used for, required env vars\n\
- ONLY create a context file at ai/operations/mcp-servers/<slug>.md if you have \
project-specific rules, constraints, or usage patterns to document for that MCP.\n\
  Do NOT create empty or boilerplate context files — they add no value.\n\
  A context file should contain: purpose in this project, specific rules, and usage examples.\n\n\
If no MCP config exists: replace ai/operations/mcp-servers.md content with:\n\
'# MCP Servers\\n\\nNo MCP servers configured for this project.'"),

    // Step 9: Inconsistencies & tech debt
    ("ai/inconsistencies-tech-debt.md", "\
Read ai/index.md and browse ALL the other ai/ files you filled in previous steps.\n\
Based on everything you observed during analysis, note any inconsistencies, legacy patterns, \
outdated configs, or technical debt.\n\n\
Fill ai/inconsistencies-tech-debt.md — replace ALL {{PLACEHOLDERS}} and <!-- ... --> placeholders:\n\
1. Fill the 'Outdated prerequisites' table: check language runtime versions (PHP, Node, Python, Ruby, etc.), \
   framework versions, deprecated bundles/packages, obsolete CSS/SASS tooling. \
   Flag anything that is EOL, deprecated, or significantly behind latest stable.\n\
2. Add concrete, factual entries to the 'Current list' table (ID, Problem, Area, Severity)\n\
3. For EACH entry in the table, also create a detail file at `ai/tech-debt/TD-YYYYMMDD-short-slug.md` \
   using the entry template format from the index file. Include: ID, Area, Severity, Problem, \
   Why we can't fix now, Impact, Where (file paths), Suggested direction, Next step.\n\
4. If no issues found, add a single row: 'None identified during initial audit'\n\
- Each entry should describe a real issue found, not a hypothetical one"),

    // Step 10: Final review
    ("REVIEW", "\
Read ALL ai/ files one by one. This is the final quality pass.\n\n\
Check each file for:\n\
1. No remaining {{PLACEHOLDERS}} — ALL must be replaced with real content (search for `{{` in all files)\n\
2. No remaining <!-- fill --> or <!-- ... --> placeholder comments — replace or remove them \
   (except `<!-- TODO: ask user -->` and `<!-- TODO: verify -->` which are intentional)\n\
3. No duplicate information across files (same fact documented in two places)\n\
4. Terminology consistency: terms in ai/glossary.md used consistently everywhere\n\
5. Cross-references: file paths mentioned in one file exist and match other files\n\
6. No contradictions: numbers, service names, commands agree across all files\n\
7. Completeness: no critical section left empty (stack table, prerequisites, test list, etc.)\n\
8. Format: markdown is clean, tables render correctly, headings are consistent\n\
9. Tech debt files: verify each entry in ai/inconsistencies-tech-debt.md has a matching \
   detail file in ai/tech-debt/\n\
10. Glossary TODO markers: verify all `<!-- TODO: ask user -->` terms are genuine unknowns\n\n\
Fix any issues found directly in the files. If a section is genuinely empty because the project \
doesn't have that feature, add a note like 'N/A — this project does not use X'."),
];

/// POST /api/projects/:id/ai-audit
/// Runs a 10-step AI audit, streaming progress via SSE.
pub async fn run_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<LaunchAuditRequest>,
) -> Sse<SseStream> {
    // Look up project
    let project = state.db.with_conn({
        let id = id.clone();
        move |conn| crate::db::projects::get_project(conn, &id)
    }).await.ok().flatten();

    if project.is_none() {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(
                Event::default().event("error").data("{\"error\":\"Project not found\"}")
            )
        }));
        return Sse::new(stream);
    }

    let project = project.unwrap();
    let project_path_str = project.path.clone();
    let project_path = scanner::resolve_host_path(&project.path);

    // Remove bootstrap prompt before running audit
    let index_file = project_path.join("ai/index.md");
    if index_file.exists() {
        remove_bootstrap_block(&index_file);
    }

    let tokens = {
        let config = state.config.read().await;
        config.tokens.clone()
    };

    let agent_type = req.agent;
    let total_steps = ANALYSIS_STEPS.len();

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        let start = serde_json::json!({ "total_steps": total_steps });
        yield Event::default().event("start").data(start.to_string());

        for (step_num, (target_file, step_prompt)) in ANALYSIS_STEPS.iter().enumerate() {
            let step = step_num + 1;
            let file_label = if *target_file == "REVIEW" { "Relecture finale" } else { target_file };

            let step_start = serde_json::json!({
                "step": step,
                "total": total_steps,
                "file": file_label
            });
            yield Event::default().event("step_start").data(step_start.to_string());

            let full_prompt = format!("{}\n\n{}", PROMPT_PREAMBLE, step_prompt);

            // No profiles for audit — solo agent mode produces clean factual documentation.
            // Multi-profile debate format would pollute ai/ files with discussion artifacts.

            // Always use full_access for audit (agent needs to write files)
            match runner::start_agent_with_config(runner::AgentStartConfig {
                agent_type: &agent_type, project_path: &project_path_str, work_dir: None,
                prompt: &full_prompt, tokens: &tokens, full_access: true,
                skill_ids: &[], directive_ids: &[], profile_ids: &[],
                mcp_context_override: None,
            }).await {
                Ok(mut process) => {
                    while let Some(line) = process.next_line().await {
                        let chunk = serde_json::json!({ "text": line, "step": step });
                        yield Event::default().event("chunk").data(chunk.to_string());
                    }

                    let status = process.child.wait().await;
                    process.fix_ownership();
                    tracing::debug!("Audit step {}: fix_ownership applied for {}", step, file_label);
                    let success = status.map(|s| s.success()).unwrap_or(false);

                    let step_done = serde_json::json!({
                        "step": step,
                        "success": success,
                        "file": file_label
                    });
                    yield Event::default().event("step_done").data(step_done.to_string());
                }
                Err(e) => {
                    tracing::error!("Audit step {} failed to start: {}", step, e);
                    let err = serde_json::json!({
                        "error": format!("Step {} ({}): {}", step, file_label, e),
                        "step": step
                    });
                    yield Event::default().event("step_error").data(err.to_string());
                    // Continue to next step (same behavior as CLI)
                }
            }
        }

        let done = serde_json::json!({ "status": "complete", "total_steps": total_steps });
        yield Event::default().event("done").data(done.to_string());
    });

    Sse::new(stream)
}

/// GET /api/projects/:id/audit-info
/// Returns the list of filled AI files and remaining TODOs for the validation prompt.
pub async fn audit_info(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AuditInfo>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    // Run filesystem I/O on blocking thread pool to avoid blocking the async runtime
    let result = tokio::task::spawn_blocking(move || {
        compute_audit_info_sync(&project_path_str)
    }).await.unwrap_or_else(|_| AuditInfo { files: vec![], todos: vec![], tech_debt_items: vec![] });

    Json(ApiResponse::ok(result))
}

/// POST /api/projects/:id/validate-audit
/// Marks the audit as validated by injecting a KRONN:VALIDATED marker into ai/index.md.
pub async fn validate_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    // Run filesystem I/O on blocking thread pool
    let validate_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let index_file = project_path.join("ai/index.md");

        if !index_file.exists() {
            return Err("ai/index.md not found — run the audit first".into());
        }

        let content = std::fs::read_to_string(&index_file)
            .map_err(|e| format!("Cannot read ai/index.md: {}", e))?;

        if content.contains("KRONN:VALIDATED") {
            return Ok(());
        }

        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let marker = format!("\n<!-- KRONN:VALIDATED:{} -->\n", today);
        let new_content = format!("{}{}", content.trim_end(), marker);

        std::fs::write(&index_file, new_content)
            .map_err(|e| format!("Failed to write marker: {}", e))?;

        Ok(())
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = validate_result {
        return Json(ApiResponse::err(e));
    }

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

/// POST /api/projects/:id/mark-bootstrapped
/// Marks the project as bootstrapped by injecting a KRONN:BOOTSTRAPPED marker into ai/index.md.
pub async fn mark_bootstrapped(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let index_file = project_path.join("ai/index.md");

        if !index_file.exists() {
            return Err("ai/index.md not found".into());
        }

        let content = std::fs::read_to_string(&index_file)
            .map_err(|e| format!("Cannot read ai/index.md: {}", e))?;

        if content.contains("KRONN:BOOTSTRAPPED") {
            return Ok(());
        }

        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let marker = format!("\n<!-- KRONN:BOOTSTRAPPED:{} -->\n", today);
        let new_content = format!("{}{}", content.trim_end(), marker);

        std::fs::write(&index_file, new_content)
            .map_err(|e| format!("Failed to write marker: {}", e))?;

        Ok(())
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = result {
        return Json(ApiResponse::err(e));
    }

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

// ─── Shared helpers ─────────────────────────────────────────────────────────

/// Compute audit info (files + TODOs) from the filesystem.
fn compute_audit_info_sync(project_path_str: &str) -> AuditInfo {
    let project_path = scanner::resolve_host_path(project_path_str);
    let ai_dir = project_path.join("ai");

    if !ai_dir.is_dir() {
        return AuditInfo { files: vec![], todos: vec![], tech_debt_items: vec![] };
    }

    let mut files = Vec::new();
    let mut todos = Vec::new();

    for entry in walkdir::WalkDir::new(&ai_dir).max_depth(4).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let rel = entry.path().strip_prefix(&project_path).unwrap_or(entry.path());
        let rel_str = rel.to_string_lossy().to_string();

        if let Ok(content) = std::fs::read_to_string(entry.path()) {
            let is_empty = content.lines()
                .filter(|l| !l.trim().is_empty() && !l.starts_with('#') && !l.starts_with('>') && !l.starts_with("---") && !l.starts_with('|'))
                .count() < 3;

            files.push(AuditFileInfo {
                path: rel_str.clone(),
                filled: !is_empty && !content.contains("{{"),
            });

            for (line_num, line) in content.lines().enumerate() {
                if line.contains("<!-- TODO") {
                    todos.push(AuditTodo {
                        file: rel_str.clone(),
                        line: (line_num + 1) as u32,
                        text: line.trim().to_string(),
                    });
                }
            }
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    // Parse tech-debt items from the "Current list" table in inconsistencies-tech-debt.md
    let mut tech_debt_items = Vec::new();
    let tech_debt_file = ai_dir.join("inconsistencies-tech-debt.md");
    if let Ok(content) = std::fs::read_to_string(&tech_debt_file) {
        // Parse markdown table rows: | ID | Problem | Area | Severity |
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with('|') || trimmed.starts_with("| ID") || trimmed.starts_with("|--") || trimmed.contains("{{") {
                continue;
            }
            let cols: Vec<&str> = trimmed.split('|').map(|c| c.trim()).collect();
            // cols[0] is empty (before first |), cols[1]=ID, cols[2]=Problem, cols[3]=Area, cols[4]=Severity
            if cols.len() >= 5 && cols[1].starts_with("TD-") {
                tech_debt_items.push(TechDebtItem {
                    id: cols[1].to_string(),
                    problem: cols[2].to_string(),
                    area: cols[3].to_string(),
                    severity: cols[4].to_string(),
                });
            }
        }
    }

    AuditInfo { files, todos, tech_debt_items }
}

/// Build the validation discussion prompt with file/TODO/tech-debt enrichment.
/// The prompt follows a strict 4-phase protocol to ensure thorough validation.
fn build_validation_prompt(language: &str, info: &AuditInfo, has_issue_tracker_mcp: bool) -> String {
    let base = match language {
        "en" => {
            let mut s = String::from(concat!(
                "Here is the AI context for the project (ai/ folder). You must follow a **strict 4-phase validation protocol**. ",
                "Do NOT emit KRONN:VALIDATION_COMPLETE until ALL 4 phases are done.\n\n",
                "## Phase 1 — Auto-fix (autonomous)\n",
                "Read the project source code and resolve all issues you can handle autonomously:\n",
                "- Orphan <!-- TODO --> markers that reference nonexistent content\n",
                "- Empty/skeleton files where you can infer the content from the actual codebase\n",
                "- Outdated or incorrect information you can verify from the code\n",
                "Update the ai/ files directly. Report what you fixed.\n\n",
                "## Phase 2 — Ambiguity questions (interactive)\n",
                "For remaining ambiguities or decisions only a human can make, ask your questions **one by one**.\n",
                "- After each of my answers, **immediately update** the relevant ai/ files before asking the next question.\n",
                "- If my answer reveals new unknowns or contradictions, ask follow-up questions — do NOT skip them.\n",
                "- Do NOT move to Phase 3 until all ambiguities are resolved.\n\n",
                "## Phase 3 — Tech debt & inconsistencies review (interactive)\n",
                "Review EVERY entry in `ai/inconsistencies-tech-debt.md` and the detail files in `ai/tech-debt/`.\n",
                "For each tech-debt item, present it to me and ask:\n",
                "- Is this assessment accurate? Should the severity be adjusted?\n",
                "- What is the priority for fixing this?\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str("- Should I create a ticket/issue for this item? (I have access to your issue tracker via MCP)\n");
            }
            s.push_str(concat!(
                "Update entries based on my feedback. Remove false positives. Add any issues I mention that were missed.\n",
                "If there are many items, you may group them by area (e.g. \"Here are the 3 Backend items...\") ",
                "but still require explicit confirmation for each.\n\n",
                "## Phase 4 — Documentation challenge (interactive)\n",
                "Ask me 2-3 practical questions that a developer joining the project might ask, for example:\n",
                "- \"How do I add a new API endpoint?\" or \"How do I run the tests?\"\n",
                "- Then check if the ai/ documentation answers them correctly and completely.\n",
                "- If the documentation is insufficient or wrong, update it and ask another question.\n",
                "- If all questions are answered correctly by the docs, this phase is complete.\n\n",
                "## Completion\n",
                "Only when ALL 4 phases are fully complete (no remaining ambiguities, all tech-debt items reviewed, ",
                "documentation validated), end your message with the exact phrase: \"KRONN:VALIDATION_COMPLETE\".\n",
                "NEVER emit this phrase early. If in doubt, ask one more question.",
            ));
            s
        },
        "es" => {
            let mut s = String::from(concat!(
                "Aqui esta el contexto AI del proyecto (carpeta ai/). Debes seguir un **protocolo estricto de validacion en 4 fases**. ",
                "NO emitas KRONN:VALIDATION_COMPLETE hasta que las 4 fases esten completas.\n\n",
                "## Fase 1 — Auto-correccion (autonoma)\n",
                "Lee el codigo fuente del proyecto y resuelve todo lo que puedas de forma autonoma:\n",
                "- Marcadores <!-- TODO --> huerfanos que referencian contenido inexistente\n",
                "- Archivos vacios/esqueleto donde puedas inferir el contenido del codigo real\n",
                "- Informacion desactualizada o incorrecta que puedas verificar del codigo\n",
                "Actualiza los archivos ai/ directamente. Reporta lo que corregiste.\n\n",
                "## Fase 2 — Preguntas de ambiguedad (interactiva)\n",
                "Para ambiguedades restantes o decisiones que solo un humano puede tomar, haz tus preguntas **una por una**.\n",
                "- Con cada respuesta mia, **actualiza inmediatamente** los archivos ai/ correspondientes antes de la siguiente pregunta.\n",
                "- Si mi respuesta revela nuevas incognitas o contradicciones, haz preguntas de seguimiento — NO las omitas.\n",
                "- NO pases a la Fase 3 hasta que todas las ambiguedades esten resueltas.\n\n",
                "## Fase 3 — Revision de deuda tecnica e inconsistencias (interactiva)\n",
                "Revisa CADA entrada en `ai/inconsistencies-tech-debt.md` y los archivos detalle en `ai/tech-debt/`.\n",
                "Para cada item de deuda tecnica, presentamelo y pregunta:\n",
                "- ¿Es precisa esta evaluacion? ¿Debe ajustarse la severidad?\n",
                "- ¿Cual es la prioridad para corregir esto?\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str("- ¿Debo crear un ticket/issue para este item? (Tengo acceso a tu gestor de issues via MCP)\n");
            }
            s.push_str(concat!(
                "Actualiza las entradas segun mi feedback. Elimina falsos positivos. Agrega problemas que mencione y se hayan omitido.\n",
                "Si hay muchos items, puedes agruparlos por area pero requiere confirmacion explicita para cada uno.\n\n",
                "## Fase 4 — Desafio de documentacion (interactiva)\n",
                "Hazme 2-3 preguntas practicas que un desarrollador nuevo en el proyecto podria hacer, por ejemplo:\n",
                "- \"¿Como agrego un nuevo endpoint API?\" o \"¿Como ejecuto los tests?\"\n",
                "- Luego verifica si la documentacion ai/ las responde correcta y completamente.\n",
                "- Si la documentacion es insuficiente o incorrecta, actualizala y haz otra pregunta.\n",
                "- Si todas las preguntas son respondidas correctamente por la doc, esta fase esta completa.\n\n",
                "## Finalizacion\n",
                "Solo cuando las 4 fases esten completamente terminadas (sin ambiguedades, todos los items de deuda tecnica revisados, ",
                "documentacion validada), termina tu mensaje con la frase exacta: \"KRONN:VALIDATION_COMPLETE\".\n",
                "NUNCA emitas esta frase antes de tiempo. Si tienes dudas, haz una pregunta mas.",
            ));
            s
        },
        _ => {
            let mut s = String::from(concat!(
                "Voici le contexte AI du projet (dossier ai/). Tu dois suivre un **protocole strict de validation en 4 phases**. ",
                "NE PAS emettre KRONN:VALIDATION_COMPLETE tant que les 4 phases ne sont pas terminees.\n\n",
                "## Phase 1 — Auto-correction (autonome)\n",
                "Lis le code source du projet et resous tout ce que tu peux gerer de facon autonome :\n",
                "- Marqueurs <!-- TODO --> orphelins qui referencent du contenu inexistant\n",
                "- Fichiers vides/squelettes ou tu peux inferer le contenu depuis le code reel\n",
                "- Informations obsoletes ou incorrectes verifiables depuis le code\n",
                "Mets a jour les fichiers ai/ directement. Indique ce que tu as corrige.\n\n",
                "## Phase 2 — Questions d'ambiguite (interactif)\n",
                "Pour les ambiguites restantes ou les decisions que seul un humain peut prendre, pose tes questions **une par une**.\n",
                "- A chaque reponse de ma part, **mets immediatement a jour** les fichiers ai/ concernes avant la question suivante.\n",
                "- Si ma reponse revele de nouvelles zones d'ombre ou des contradictions, pose des questions de suivi — ne les ignore PAS.\n",
                "- NE passe PAS a la Phase 3 tant que toutes les ambiguites ne sont pas resolues.\n\n",
                "## Phase 3 — Revue de la dette technique et des inconsistances (interactif)\n",
                "Passe en revue CHAQUE entree dans `ai/inconsistencies-tech-debt.md` et les fichiers detail dans `ai/tech-debt/`.\n",
                "Pour chaque item de dette technique, presente-le-moi et demande :\n",
                "- Cette evaluation est-elle correcte ? Faut-il ajuster la severite ?\n",
                "- Quelle est la priorite pour corriger ce point ?\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str("- Dois-je creer un ticket/issue pour cet item ? (J'ai acces a ton gestionnaire d'issues via MCP)\n");
            }
            s.push_str(concat!(
                "Mets a jour les entrees selon mon retour. Supprime les faux positifs. Ajoute les problemes que je signale et qui ont ete oublies.\n",
                "S'il y a beaucoup d'items, tu peux les regrouper par domaine (ex : \"Voici les 3 items Backend...\") ",
                "mais exige une confirmation explicite pour chacun.\n\n",
                "## Phase 4 — Challenge de la documentation (interactif)\n",
                "Pose-moi 2-3 questions pratiques qu'un developpeur rejoignant le projet pourrait poser, par exemple :\n",
                "- \"Comment ajouter un nouvel endpoint API ?\" ou \"Comment lancer les tests ?\"\n",
                "- Puis verifie si la documentation ai/ y repond correctement et completement.\n",
                "- Si la documentation est insuffisante ou incorrecte, mets-la a jour et pose une autre question.\n",
                "- Si toutes les questions sont correctement couvertes par la doc, cette phase est terminee.\n\n",
                "## Fin\n",
                "Uniquement quand les 4 phases sont entierement terminees (plus aucune ambiguite, tous les items de dette technique revus, ",
                "documentation validee), termine ton message par la phrase exacte : \"KRONN:VALIDATION_COMPLETE\".\n",
                "NE JAMAIS emettre cette phrase en avance. En cas de doute, pose encore une question.",
            ));
            s
        },
    };

    let mut prompt = base;

    // Summary counts only — the agent has filesystem access to read the actual files
    let unfilled_count = info.files.iter().filter(|f| !f.filled).count();
    let total_files = info.files.len();
    if total_files > 0 {
        let summary = match language {
            "en" => format!("{} AI files detected ({} still incomplete). Read `ai/index.md` for the full tree.", total_files, unfilled_count),
            "es" => format!("{} archivos AI detectados ({} aun incompletos). Lee `ai/index.md` para el arbol completo.", total_files, unfilled_count),
            _ => format!("{} fichiers AI detectes ({} encore incomplets). Lis `ai/index.md` pour l'arbre complet.", total_files, unfilled_count),
        };
        prompt.push_str(&format!("\n\n{}", summary));
    }

    if !info.todos.is_empty() {
        let hint = match language {
            "en" => format!("{} remaining TODO markers across AI files. Scan `ai/` for `<!-- TODO` to find them all.", info.todos.len()),
            "es" => format!("{} marcadores TODO restantes en archivos AI. Busca `<!-- TODO` en `ai/` para encontrarlos.", info.todos.len()),
            _ => format!("{} marqueurs TODO restants dans les fichiers AI. Cherche `<!-- TODO` dans `ai/` pour les trouver.", info.todos.len()),
        };
        prompt.push_str(&format!("\n\n{}", hint));
    }

    if !info.tech_debt_items.is_empty() {
        let hint = match language {
            "en" => format!("{} tech debt items to review in Phase 3. Read `ai/inconsistencies-tech-debt.md` and `ai/tech-debt/` for details.", info.tech_debt_items.len()),
            "es" => format!("{} items de deuda tecnica a revisar en Fase 3. Lee `ai/inconsistencies-tech-debt.md` y `ai/tech-debt/` para detalles.", info.tech_debt_items.len()),
            _ => format!("{} items de dette technique a revoir en Phase 3. Lis `ai/inconsistencies-tech-debt.md` et `ai/tech-debt/` pour les details.", info.tech_debt_items.len()),
        };
        prompt.push_str(&format!("\n\n{}", hint));
    }

    prompt
}

// ─── Full audit (unified flow) ──────────────────────────────────────────────

/// POST /api/projects/:id/full-audit
/// Unified endpoint: install template + run 10-step audit + create validation discussion.
pub async fn full_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<LaunchAuditRequest>,
) -> Sse<SseStream> {
    // Look up project
    let project = state.db.with_conn({
        let id = id.clone();
        move |conn| crate::db::projects::get_project(conn, &id)
    }).await.ok().flatten();

    if project.is_none() {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(
                Event::default().event("error").data("{\"error\":\"Project not found\"}")
            )
        }));
        return Sse::new(stream);
    }

    let project = project.unwrap();
    let project_id = project.id.clone();
    let project_path_str = project.path.clone();
    let project_path = scanner::resolve_host_path(&project.path);
    let project_default_skill_ids = project.default_skill_ids.clone();
    let agent_type = req.agent;

    let tokens = {
        let config = state.config.read().await;
        config.tokens.clone()
    };

    let language = {
        let config = state.config.read().await;
        config.language.clone()
    };

    let total_steps = ANALYSIS_STEPS.len();
    let db = state.db.clone();
    let audit_tracker = state.audit_tracker.clone();

    // Clear any stale cancellation flag for this project
    audit_tracker.lock().unwrap().cancelled.remove(&project_id);

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        // ── Phase 1: Install template if needed ──
        let status = scanner::detect_audit_status(&project_path_str);
        let template_installed = matches!(status, AiAuditStatus::NoTemplate);

        if template_installed {
            let pp = project_path_str.clone();
            let install_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
                let project_path = scanner::resolve_host_path(&pp);
                if !project_path.exists() {
                    return Err(format!("Project path not found: {}", project_path.display()));
                }

                let ai_target = project_path.join("ai");
                if ai_target.exists() {
                    if let Err(e) = check_ai_dir_permissions(&ai_target) {
                        return Err(format!("ai/ permission error: {}", e));
                    }
                }

                let template_dir = resolve_templates_dir();
                if !template_dir.exists() {
                    return Err(format!("Templates directory not found: {}", template_dir.display()));
                }

                let ai_template = template_dir.join("ai");
                if ai_template.is_dir() {
                    copy_dir_nondestructive(&ai_template, &ai_target)?;
                }

                for filename in &["CLAUDE.md", ".cursorrules", ".windsurfrules", ".clinerules"] {
                    let src = template_dir.join(filename);
                    let dst = project_path.join(filename);
                    if src.exists() && !dst.exists() {
                        if let Err(e) = std::fs::copy(&src, &dst) {
                            tracing::warn!("Failed to copy {}: {}", filename, e);
                        }
                    }
                }

                let index_file = project_path.join("ai/index.md");
                if index_file.exists() {
                    inject_bootstrap_prompt(&index_file);
                }

                runner::fix_file_ownership(&project_path);
                Ok(())
            }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

            if let Err(e) = install_result {
                let err = serde_json::json!({ "error": e });
                yield Event::default().event("error").data(err.to_string());
                return;
            }

            crate::core::mcp_scanner::ensure_gitignore_public(&project_path_str, "ai/var/");
        }

        let tmpl_event = serde_json::json!({ "installed": template_installed });
        yield Event::default().event("template_installed").data(tmpl_event.to_string());

        // ── Phase 2: Run 10-step audit ──
        // Remove bootstrap prompt
        let index_file = project_path.join("ai/index.md");
        if index_file.exists() {
            remove_bootstrap_block(&index_file);
        }

        let start = serde_json::json!({ "total_steps": total_steps });
        yield Event::default().event("start").data(start.to_string());

        for (step_num, (target_file, step_prompt)) in ANALYSIS_STEPS.iter().enumerate() {
            // Check for cancellation before each step
            if audit_tracker.lock().unwrap().cancelled.contains(&project_id) {
                let cancelled = serde_json::json!({ "status": "cancelled" });
                yield Event::default().event("cancelled").data(cancelled.to_string());
                return;
            }

            let step = step_num + 1;
            let file_label = if *target_file == "REVIEW" { "Relecture finale" } else { target_file };

            let step_start = serde_json::json!({
                "step": step, "total": total_steps, "file": file_label
            });
            yield Event::default().event("step_start").data(step_start.to_string());

            let full_prompt = format!("{}\n\n{}", PROMPT_PREAMBLE, step_prompt);

            match runner::start_agent_with_config(runner::AgentStartConfig {
                agent_type: &agent_type, project_path: &project_path_str, work_dir: None,
                prompt: &full_prompt, tokens: &tokens, full_access: true,
                skill_ids: &[], directive_ids: &[], profile_ids: &[],
                mcp_context_override: None,
            }).await {
                Ok(mut process) => {
                    // Register the child PID for cancellation
                    if let Some(pid) = process.child.id() {
                        audit_tracker.lock().unwrap().running_pids.insert(project_id.clone(), pid);
                    }

                    while let Some(line) = process.next_line().await {
                        let chunk = serde_json::json!({ "text": line, "step": step });
                        yield Event::default().event("chunk").data(chunk.to_string());
                    }
                    let status = process.child.wait().await;
                    process.fix_ownership();

                    // Unregister PID
                    audit_tracker.lock().unwrap().running_pids.remove(&project_id);

                    // Check if cancelled during this step
                    if audit_tracker.lock().unwrap().cancelled.contains(&project_id) {
                        let cancelled = serde_json::json!({ "status": "cancelled" });
                        yield Event::default().event("cancelled").data(cancelled.to_string());
                        return;
                    }

                    let success = status.map(|s| s.success()).unwrap_or(false);

                    let step_done = serde_json::json!({
                        "step": step, "success": success, "file": file_label
                    });
                    yield Event::default().event("step_done").data(step_done.to_string());
                }
                Err(e) => {
                    tracing::error!("Audit step {} failed to start: {}", step, e);
                    let err = serde_json::json!({
                        "error": format!("Step {} ({}): {}", step, file_label, e),
                        "step": step
                    });
                    yield Event::default().event("step_error").data(err.to_string());
                }
            }
        }

        // ── Auto-detect project skills ──
        let detected_skill_ids = {
            let p = project_path.clone();
            tokio::task::spawn_blocking(move || detect_project_skills(&p))
                .await.unwrap_or_default()
        };
        let skill_ids_for_disc = if detected_skill_ids.is_empty() {
            project_default_skill_ids.clone()
        } else {
            // Save detected skills to DB
            let pid = project_id.clone();
            let sids = detected_skill_ids.clone();
            let _ = db.with_conn(move |conn| {
                crate::db::projects::update_project_default_skills(conn, &pid, &sids)
            }).await;
            detected_skill_ids
        };

        // ── Phase 3: Create validation discussion ──
        let pp = project_path_str.clone();
        let audit_info = tokio::task::spawn_blocking(move || {
            compute_audit_info_sync(&pp)
        }).await.unwrap_or_else(|_| AuditInfo { files: vec![], todos: vec![], tech_debt_items: vec![] });

        // Detect if project has an issue tracker MCP (GitHub, GitLab, Jira, Linear, etc.)
        let has_issue_tracker_mcp = detect_issue_tracker_mcp(&project_path);

        let validation_prompt = build_validation_prompt(&language, &audit_info, has_issue_tracker_mcp);

        let now = Utc::now();
        let discussion_id = Uuid::new_v4().to_string();
        let initial_message = DiscussionMessage {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::User,
            content: validation_prompt,
            agent_type: None,
            timestamp: now,
            tokens_used: 0,
            auth_mode: None,
        };

        let discussion = Discussion {
            id: discussion_id.clone(),
            project_id: Some(project_id.clone()),
            title: "Validation audit AI".to_string(),
            agent: agent_type.clone(),
            language: language.clone(),
            participants: vec![agent_type.clone()],
            messages: vec![initial_message.clone()],
            message_count: 1,
            skill_ids: skill_ids_for_disc.clone(),
            profile_ids: vec![
                "architect".into(),
                "tech-lead".into(),
                "qa-engineer".into(),
                "devils-advocate".into(),
            ],
            directive_ids: vec![],
            archived: false,
            created_at: now,
            updated_at: now,
        };

        let disc = discussion.clone();
        let msg = initial_message;
        let disc_created = db.with_conn(move |conn| {
            crate::db::discussions::insert_discussion(conn, &disc)?;
            crate::db::discussions::insert_message(conn, &disc.id, &msg)?;
            Ok(())
        }).await;

        let disc_id = match disc_created {
            Ok(()) => {
                let ev = serde_json::json!({ "discussion_id": discussion_id });
                yield Event::default().event("validation_created").data(ev.to_string());
                Some(discussion_id)
            }
            Err(e) => {
                tracing::error!("Failed to create validation discussion: {}", e);
                let err = serde_json::json!({ "error": format!("Failed to create validation discussion: {}", e) });
                yield Event::default().event("step_error").data(err.to_string());
                None
            }
        };

        let done = serde_json::json!({
            "status": "complete",
            "total_steps": total_steps,
            "discussion_id": disc_id,
            "template_was_installed": template_installed
        });
        yield Event::default().event("done").data(done.to_string());
    });

    Sse::new(stream)
}

/// Check if the project has an issue tracker MCP configured (.mcp.json).
/// Looks for known issue tracker server names: github, gitlab, jira, atlassian, linear, etc.
/// Auto-detect skills from project filesystem (config files, package managers, etc.)
fn detect_project_skills(project_path: &std::path::Path) -> Vec<String> {
    let mut skills: Vec<String> = Vec::new();

    // ── Language detection (from package managers / config files) ──
    if project_path.join("Cargo.toml").exists() {
        skills.push("rust".into());
    }
    if project_path.join("package.json").exists() {
        // Check if TypeScript
        if project_path.join("tsconfig.json").exists()
            || project_path.join("tsconfig.app.json").exists()
        {
            skills.push("typescript".into());
        }
    }
    if project_path.join("requirements.txt").exists()
        || project_path.join("pyproject.toml").exists()
        || project_path.join("setup.py").exists()
    {
        skills.push("python".into());
    }
    if project_path.join("go.mod").exists() {
        skills.push("go".into());
    }
    if project_path.join("composer.json").exists() {
        skills.push("php".into());
    }

    // ── Domain detection ──
    // DevOps: Dockerfile, CI/CD, IaC
    if project_path.join("Dockerfile").exists()
        || project_path.join("docker-compose.yml").exists()
        || project_path.join("docker-compose.yaml").exists()
        || project_path.join(".github").join("workflows").exists()
        || project_path.join(".gitlab-ci.yml").exists()
        || project_path.join("Makefile").exists()
    {
        skills.push("devops".into());
    }

    // Database: migrations, schema files
    if project_path.join("migrations").exists()
        || project_path.join("db").exists()
        || project_path.join("prisma").exists()
        || project_path.join("drizzle").exists()
    {
        skills.push("database".into());
    }

    // Security: auth configs, security headers
    if project_path.join(".env.example").exists()
        || project_path.join("security.yaml").exists()
        || project_path.join("config").join("packages").join("security.yaml").exists()
    {
        skills.push("security".into());
    }

    // ── Business detection ──
    // Web performance: frontend projects with build tools
    if project_path.join("webpack.config.js").exists()
        || project_path.join("vite.config.ts").exists()
        || project_path.join("vite.config.js").exists()
        || project_path.join("next.config.js").exists()
        || project_path.join("next.config.ts").exists()
    {
        skills.push("web-performance".into());
    }

    // SEO: robots.txt, sitemap
    if project_path.join("robots.txt").exists()
        || project_path.join("public").join("robots.txt").exists()
    {
        skills.push("seo".into());
    }

    // Filter to only keep skills that actually exist in the system
    let valid: Vec<String> = skills.into_iter()
        .filter(|id| crate::core::skills::get_skill(id).is_some())
        .collect();

    tracing::info!("Auto-detected skills for {}: {:?}", project_path.display(), valid);
    valid
}

fn detect_issue_tracker_mcp(project_path: &std::path::Path) -> bool {
    let mcp_file = project_path.join(".mcp.json");
    if let Ok(content) = std::fs::read_to_string(&mcp_file) {
        let lower = content.to_lowercase();
        return lower.contains("github") || lower.contains("gitlab")
            || lower.contains("jira") || lower.contains("atlassian")
            || lower.contains("linear") || lower.contains("youtrack");
    }
    false
}

/// Find the common parent directory of existing projects.
/// E.g. if projects are at /home/user/Repos/A and /home/user/Repos/B, returns /home/user/Repos.
fn find_common_parent(projects: &[Project]) -> Option<String> {
    let paths: Vec<&str> = projects.iter().map(|p| p.path.as_str()).collect();
    if paths.is_empty() {
        return None;
    }
    let first: Vec<&str> = paths[0].split('/').collect();
    let mut prefix_len = first.len();
    for path in &paths[1..] {
        let parts: Vec<&str> = path.split('/').collect();
        prefix_len = prefix_len.min(parts.len());
        for i in 0..prefix_len {
            if first[i] != parts[i] {
                prefix_len = i;
                break;
            }
        }
    }
    if prefix_len <= 1 {
        return None; // just "/" — not useful
    }
    Some(first[..prefix_len].join("/"))
}

/// Files installed by the audit template (to be removed on cancel).
const AUDIT_REDIRECTOR_FILES: &[&str] = &["CLAUDE.md", ".cursorrules", ".windsurfrules", ".clinerules"];

/// POST /api/projects/:id/cancel-audit
/// Cancel a running audit and remove ALL files created by the audit.
pub async fn cancel_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    // Look up project
    let project = match state.db.with_conn({
        let id = id.clone();
        move |conn| crate::db::projects::get_project(conn, &id)
    }).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_id = project.id.clone();

    // 1. Signal cancellation and kill any running agent process
    {
        let mut tracker = state.audit_tracker.lock().unwrap();
        tracker.cancelled.insert(project_id.clone());
        if let Some(pid) = tracker.running_pids.remove(&project_id) {
            tracing::info!("Killing audit agent process (PID {}) for project {}", pid, project_id);
            // Kill the process tree: first try killing the process group, then the process itself
            let _ = std::process::Command::new("kill")
                .args(["-9", &format!("-{}", pid)]) // negative PID = process group
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let _ = std::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    // Small delay to let the SSE stream detect the cancellation
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // 2. Delete all audit-created files
    let project_path_str = project.path.clone();
    let cleanup_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        if !project_path.exists() {
            return Err(format!("Project path not found: {}", project_path.display()));
        }

        // Remove ai/ directory entirely
        let ai_dir = project_path.join("ai");
        if ai_dir.exists() {
            std::fs::remove_dir_all(&ai_dir)
                .map_err(|e| format!("Failed to remove ai/: {}", e))?;
            tracing::info!("Removed ai/ directory from {}", project_path.display());
        }

        // Remove redirector files (CLAUDE.md, .cursorrules, etc.)
        for filename in AUDIT_REDIRECTOR_FILES {
            let file = project_path.join(filename);
            if file.exists() {
                if let Err(e) = std::fs::remove_file(&file) {
                    tracing::warn!("Failed to remove {}: {}", filename, e);
                } else {
                    tracing::info!("Removed {} from {}", filename, project_path.display());
                }
            }
        }

        Ok(())
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = cleanup_result {
        // Clear cancellation flag before returning error
        state.audit_tracker.lock().unwrap().cancelled.remove(&project_id);
        return Json(ApiResponse::err(e));
    }

    // 3. Delete any validation discussion for this project
    let pid = project_id.clone();
    let _ = state.db.with_conn(move |conn| {
        // Find and delete validation discussions for this project
        let discussions = crate::db::discussions::list_discussions(conn)?;
        for disc in discussions {
            if disc.project_id.as_deref() == Some(&pid) && disc.title == "Validation audit AI" {
                crate::db::discussions::delete_discussion(conn, &disc.id)?;
                tracing::info!("Deleted validation discussion {} for project {}", disc.id, pid);
            }
        }
        Ok(())
    }).await;

    // 4. Clear cancellation flag
    state.audit_tracker.lock().unwrap().cancelled.remove(&project_id);

    // Return updated status (should be NoTemplate now)
    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

/// PUT /api/projects/:id/default-skills
pub async fn set_default_skills(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(skill_ids): Json<Vec<String>>,
) -> Json<ApiResponse<bool>> {
    match state.db.with_conn(move |conn| {
        crate::db::projects::update_project_default_skills(conn, &id, &skill_ids)
    }).await {
        Ok(true) => Json(ApiResponse::ok(true)),
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
    match state.db.with_conn(move |conn| {
        crate::db::projects::update_project_default_profile(conn, &id, profile_id.as_deref())
    }).await {
        Ok(true) => Json(ApiResponse::ok(true)),
        Ok(false) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// Try to detect permission issues on an existing ai/ directory.
/// Returns Ok(()) if all files are accessible, or Err with description if unfixable.
fn check_ai_dir_permissions(ai_dir: &std::path::Path) -> Result<(), String> {
    for entry in walkdir::WalkDir::new(ai_dir).max_depth(5).into_iter() {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => return Err(format!("Cannot traverse ai/ directory: {}", e)),
        };
        let path = entry.path();
        if path.is_file() {
            if let Err(e) = std::fs::read(path) {
                return Err(format!("{}: {}", path.display(), e));
            }
        }
    }
    Ok(())
}

/// Remove the KRONN:BOOTSTRAP block from ai/index.md
fn remove_bootstrap_block(index_file: &std::path::Path) {
    let content = match std::fs::read_to_string(index_file) {
        Ok(c) => c,
        Err(_) => return,
    };

    if !content.contains("KRONN:BOOTSTRAP:START") {
        return;
    }

    // Remove everything between START and END markers (inclusive)
    let mut result = String::new();
    let mut in_block = false;
    for line in content.lines() {
        if line.contains("KRONN:BOOTSTRAP:START") {
            in_block = true;
            continue;
        }
        if line.contains("KRONN:BOOTSTRAP:END") {
            in_block = false;
            continue;
        }
        if !in_block {
            result.push_str(line);
            result.push('\n');
        }
    }

    // Trim leading whitespace from the cleaned content
    let trimmed = result.trim_start().to_string();
    if let Err(e) = std::fs::write(index_file, trimmed) {
        tracing::warn!("Failed to remove bootstrap block: {}", e);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// AI Documentation File Browser
// ═══════════════════════════════════════════════════════════════════════════════

/// GET /api/projects/:id/ai-files
/// Returns the tree of `.md` files under `ai/`.
pub async fn list_ai_files(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<AiFileNode>>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    let result = tokio::task::spawn_blocking(move || {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let ai_dir = project_path.join("ai");
        if !ai_dir.is_dir() {
            return vec![];
        }
        build_ai_file_tree(&ai_dir, "ai")
    }).await.unwrap_or_default();

    Json(ApiResponse::ok(result))
}

/// Recursively build a tree of `.md` files from the `ai/` directory.
fn build_ai_file_tree(dir: &std::path::Path, rel_prefix: &str) -> Vec<AiFileNode> {
    let mut nodes = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return nodes,
    };

    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = format!("{}/{}", rel_prefix, name);
        let file_type = entry.file_type().unwrap_or_else(|_| entry.metadata().unwrap().file_type());

        if file_type.is_dir() {
            let children = build_ai_file_tree(&entry.path(), &path);
            if !children.is_empty() {
                nodes.push(AiFileNode { path, name, is_dir: true, children });
            }
        } else if name.ends_with(".md") {
            nodes.push(AiFileNode { path, name, is_dir: false, children: vec![] });
        }
    }
    nodes
}

#[derive(Debug, serde::Deserialize)]
pub struct AiFileQuery {
    pub path: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct AiSearchQuery {
    pub q: String,
}

/// GET /api/projects/:id/ai-search?q=mcp
/// Full-text search across all `.md` files in `ai/`. Returns paths + match counts.
pub async fn search_ai_files(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AiSearchQuery>,
) -> Json<ApiResponse<Vec<AiSearchResult>>> {
    let q = query.q.trim().to_string();
    if q.is_empty() {
        return Json(ApiResponse::ok(vec![]));
    }

    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    let result = tokio::task::spawn_blocking(move || {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let ai_dir = project_path.join("ai");
        if !ai_dir.is_dir() {
            return vec![];
        }
        let mut results = Vec::new();
        search_ai_dir_recursive(&ai_dir, "ai", &q.to_lowercase(), &mut results);
        // Sort by match_count descending
        results.sort_by(|a, b| b.match_count.cmp(&a.match_count));
        results
    }).await.unwrap_or_default();

    Json(ApiResponse::ok(result))
}

fn search_ai_dir_recursive(dir: &std::path::Path, rel_prefix: &str, query: &str, results: &mut Vec<AiSearchResult>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = format!("{}/{}", rel_prefix, name);
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_dir() {
            search_ai_dir_recursive(&entry.path(), &path, query, results);
        } else if name.ends_with(".md") {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                let lower = content.to_lowercase();
                let mut count = 0u32;
                let mut start = 0;
                while let Some(idx) = lower[start..].find(query) {
                    count += 1;
                    start += idx + query.len();
                }
                if count > 0 {
                    results.push(AiSearchResult { path, match_count: count });
                }
            }
        }
    }
}

/// POST /api/projects/discover-repos
/// Discovers remote repositories from GitHub/GitLab that aren't yet tracked.
/// Accepts optional source_ids to filter which MCP configs to query.
pub async fn discover_repos(
    State(state): State<AppState>,
    Json(req): Json<DiscoverReposRequest>,
) -> Json<ApiResponse<DiscoverReposResponse>> {
    let mut all_repos: Vec<RemoteRepo> = vec![];
    let mut used_sources: Vec<String> = vec![];

    // Get existing projects to check "already_cloned"
    let existing = state.db.with_conn(crate::db::projects::list_projects).await.unwrap_or_default();
    let existing_urls: std::collections::HashSet<String> = existing.iter()
        .filter_map(|p| p.repo_url.as_ref())
        .map(|u| normalize_repo_url(u))
        .collect();
    let existing_names: std::collections::HashSet<String> = existing.iter()
        .map(|p| p.name.to_lowercase())
        .collect();

    // Get all available sources
    let all_sources = find_all_provider_sources(&state).await;
    let available_sources: Vec<RepoSource> = all_sources.iter().map(|(s, _)| s.clone()).collect();

    if all_sources.is_empty() {
        return Json(ApiResponse::err(
            "No GitHub or GitLab token found. Configure the GitHub or GitLab MCP with a Personal Access Token, or set GITHUB_TOKEN / GITLAB_TOKEN environment variable."
        ));
    }

    // Filter sources if specific IDs requested
    let sources_to_use: Vec<&(RepoSource, String)> = if req.source_ids.is_empty() {
        all_sources.iter().collect()
    } else {
        all_sources.iter().filter(|(s, _)| req.source_ids.contains(&s.id)).collect()
    };

    tracing::info!(
        "discover_repos: requested source_ids={:?}, available={:?}, using={:?}",
        req.source_ids,
        available_sources.iter().map(|s| format!("{}({})", s.label, s.id)).collect::<Vec<_>>(),
        sources_to_use.iter().map(|(s, _)| format!("{}({})", s.label, s.id)).collect::<Vec<_>>(),
    );

    // Deduplicate repos by full_name (in case multiple tokens see the same repo)
    let mut seen_full_names = std::collections::HashSet::new();

    for (source, token_data) in &sources_to_use {
        match source.provider.as_str() {
            "github" => {
                let token_preview = if token_data.len() > 8 { &token_data[..8] } else { token_data };
                tracing::info!("discover_repos: querying GitHub source '{}' with token {}...", source.label, token_preview);
                match fetch_github_repos(token_data).await {
                    Ok(repos) => {
                        tracing::info!("discover_repos: source '{}' returned {} repos", source.label, repos.len());
                        used_sources.push(source.label.clone());
                        for r in repos {
                            if !seen_full_names.insert(r.full_name.clone()) {
                                continue; // skip duplicate
                            }
                            let already = existing_urls.contains(&normalize_repo_url(&r.clone_url))
                                || existing_urls.contains(&normalize_repo_url(&r.ssh_url))
                                || existing_names.contains(&r.name.to_lowercase());
                            all_repos.push(RemoteRepo {
                                already_cloned: already,
                                ..r
                            });
                        }
                    }
                    Err(e) => {
                        tracing::warn!("GitHub repo discovery failed for {}: {}", source.label, e);
                    }
                }
            }
            "gitlab" => {
                let parts: Vec<&str> = token_data.splitn(2, '|').collect();
                let (token, api_url) = (parts[0], parts.get(1).unwrap_or(&"https://gitlab.com"));
                match fetch_gitlab_repos(token, api_url).await {
                    Ok(repos) => {
                        used_sources.push(source.label.clone());
                        for r in repos {
                            if !seen_full_names.insert(r.full_name.clone()) {
                                continue;
                            }
                            let already = existing_urls.contains(&normalize_repo_url(&r.clone_url))
                                || existing_urls.contains(&normalize_repo_url(&r.ssh_url))
                                || existing_names.contains(&r.name.to_lowercase());
                            all_repos.push(RemoteRepo {
                                already_cloned: already,
                                ..r
                            });
                        }
                    }
                    Err(e) => {
                        tracing::warn!("GitLab repo discovery failed for {}: {}", source.label, e);
                    }
                }
            }
            _ => {}
        }
    }

    // Sort: not-cloned first, then by updated_at descending
    all_repos.sort_by(|a, b| {
        a.already_cloned.cmp(&b.already_cloned)
            .then(b.updated_at.cmp(&a.updated_at))
    });

    Json(ApiResponse::ok(DiscoverReposResponse { repos: all_repos, sources: used_sources, available_sources }))
}

/// Find all available token sources from MCP configs and env vars.
async fn find_all_provider_sources(state: &AppState) -> Vec<(RepoSource, String)> {
    let mut sources: Vec<(RepoSource, String)> = vec![];

    // Read encryption secret
    let config = state.config.read().await;
    let secret = config.encryption_secret.clone();
    drop(config);

    // Scan MCP configs for GitHub/GitLab tokens
    if let Some(secret) = &secret {
        let secret_clone = secret.clone();
        let configs = state.db.with_conn(move |conn| {
            crate::db::mcps::list_configs(conn)
        }).await.unwrap_or_default();

        for cfg in configs {
            if cfg.env_encrypted.is_empty() {
                continue;
            }
            let env = match crate::db::mcps::decrypt_env(&cfg.env_encrypted, &secret_clone) {
                Ok(e) => e,
                Err(_) => continue,
            };

            // GitHub MCP
            if cfg.server_id == "mcp-github" {
                if let Some(token) = env.get("GITHUB_PERSONAL_ACCESS_TOKEN").filter(|v| !v.is_empty()) {
                    let token_end = if token.len() > 4 { &token[token.len()-4..] } else { token };
                    tracing::info!(
                        "discover: found GitHub MCP config '{}' (id={}) with token ...{}",
                        cfg.label, cfg.id, token_end
                    );
                    sources.push((
                        RepoSource {
                            id: cfg.id.clone(),
                            label: cfg.label.clone(),
                            provider: "github".into(),
                        },
                        token.clone(),
                    ));
                }
            }

            // GitLab MCP
            if cfg.server_id == "mcp-gitlab" {
                if let Some(token) = env.get("GITLAB_PERSONAL_ACCESS_TOKEN").filter(|v| !v.is_empty()) {
                    let api_url = env.get("GITLAB_API_URL")
                        .filter(|v| !v.is_empty())
                        .cloned()
                        .unwrap_or_else(|| "https://gitlab.com".into());
                    // Encode the API URL in the token string with a separator
                    sources.push((
                        RepoSource {
                            id: cfg.id.clone(),
                            label: cfg.label.clone(),
                            provider: "gitlab".into(),
                        },
                        format!("{}|{}", token, api_url),
                    ));
                }
            }
        }
    }

    // Environment variable fallbacks
    if let Ok(token) = std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN")) {
        // Only add env source if there's no MCP config for GitHub already
        let has_gh = sources.iter().any(|(s, _)| s.provider == "github");
        if !has_gh {
            sources.push((
                RepoSource {
                    id: "env:github".into(),
                    label: "GitHub (env)".into(),
                    provider: "github".into(),
                },
                token,
            ));
        }
    }

    if let Ok(token) = std::env::var("GITLAB_TOKEN") {
        let has_gl = sources.iter().any(|(s, _)| s.provider == "gitlab");
        if !has_gl {
            let api_url = std::env::var("GITLAB_API_URL").unwrap_or_else(|_| "https://gitlab.com".into());
            sources.push((
                RepoSource {
                    id: "env:gitlab".into(),
                    label: "GitLab (env)".into(),
                    provider: "gitlab".into(),
                },
                format!("{}|{}", token, api_url),
            ));
        }
    }

    sources
}

/// Normalize a repo URL for comparison (strip .git suffix, lowercase, strip protocol prefix)
fn normalize_repo_url(url: &str) -> String {
    url.to_lowercase()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .replace("https://github.com/", "github:")
        .replace("https://gitlab.com/", "gitlab:")
        .replace("git@github.com:", "github:")
        .replace("git@gitlab.com:", "gitlab:")
        .to_string()
}

/// Fetch all repos for the authenticated GitHub user, including organization repos.
async fn fetch_github_repos(token: &str) -> Result<Vec<RemoteRepo>, String> {
    let client = reqwest::Client::new();
    let mut all_repos = vec![];
    let mut seen = std::collections::HashSet::new();

    // 1. User repos (owned, collaborated, org-member)
    let mut page = 1u32;
    loop {
        let url = format!(
            "https://api.github.com/user/repos?per_page=100&page={}&sort=updated&affiliation=owner,organization_member,collaborator",
            page
        );
        let repos = github_get_json_array(&client, &url, token).await?;
        if repos.is_empty() { break; }
        let done = repos.len() < 100;
        for r in &repos {
            let full_name = r["full_name"].as_str().unwrap_or("").to_string();
            if seen.insert(full_name.clone()) {
                all_repos.push(parse_github_repo(r));
            }
        }
        if done { break; }
        page += 1;
    }

    // 2. Organization repos — covers org repos the token can see but /user/repos may miss
    if let Ok(orgs) = github_get_json_array(&client, "https://api.github.com/user/orgs?per_page=100", token).await {
        for org in &orgs {
            let login = match org["login"].as_str() {
                Some(l) => l,
                None => continue,
            };
            tracing::info!("discover_repos: fetching org '{}' repos", login);
            let mut page = 1u32;
            loop {
                let url = format!(
                    "https://api.github.com/orgs/{}/repos?per_page=100&page={}&sort=updated",
                    login, page
                );
                let repos = match github_get_json_array(&client, &url, token).await {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!("discover_repos: failed to list repos for org '{}': {}", login, e);
                        break;
                    }
                };
                if repos.is_empty() { break; }
                let done = repos.len() < 100;
                for r in &repos {
                    let full_name = r["full_name"].as_str().unwrap_or("").to_string();
                    if seen.insert(full_name.clone()) {
                        all_repos.push(parse_github_repo(r));
                    }
                }
                if done { break; }
                page += 1;
            }
        }
    }

    Ok(all_repos)
}

/// Helper: GET a JSON array from GitHub API with auth headers.
async fn github_get_json_array(client: &reqwest::Client, url: &str, token: &str) -> Result<Vec<serde_json::Value>, String> {
    let resp = client.get(url)
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Kronn/0.1")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GitHub request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub API error {}: {}", status, body));
    }

    resp.json().await
        .map_err(|e| format!("Failed to parse GitHub response: {}", e))
}

/// Parse a GitHub repo JSON object into a RemoteRepo.
fn parse_github_repo(r: &serde_json::Value) -> RemoteRepo {
    RemoteRepo {
        name: r["name"].as_str().unwrap_or("").to_string(),
        full_name: r["full_name"].as_str().unwrap_or("").to_string(),
        clone_url: r["clone_url"].as_str().unwrap_or("").to_string(),
        ssh_url: r["ssh_url"].as_str().unwrap_or("").to_string(),
        description: r["description"].as_str().map(|s| s.to_string()),
        language: r["language"].as_str().map(|s| s.to_string()),
        stargazers_count: r["stargazers_count"].as_u64().unwrap_or(0) as u32,
        updated_at: r["updated_at"].as_str().unwrap_or("").to_string(),
        source: "github".into(),
        already_cloned: false,
    }
}

/// Fetch all repos for the authenticated GitLab user, including group repos.
async fn fetch_gitlab_repos(token: &str, api_url: &str) -> Result<Vec<RemoteRepo>, String> {
    let client = reqwest::Client::new();
    let base = api_url.trim_end_matches('/');
    let mut all_repos = vec![];
    let mut seen = std::collections::HashSet::new();

    // 1. User-owned projects
    gitlab_collect_projects(&client, token, &format!(
        "{}/api/v4/projects?owned=true&per_page=100&order_by=updated_at", base
    ), &mut all_repos, &mut seen).await?;

    // 2. Projects from groups the user is a member of
    if let Ok(groups) = gitlab_get_json_array(&client, &format!(
        "{}/api/v4/groups?per_page=100&min_access_level=10", base
    ), token).await {
        for g in &groups {
            let group_id = match g["id"].as_u64() {
                Some(id) => id,
                None => continue,
            };
            let group_name = g["full_path"].as_str().unwrap_or("?");
            tracing::info!("discover_repos: fetching GitLab group '{}' projects", group_name);
            if let Err(e) = gitlab_collect_projects(&client, token, &format!(
                "{}/api/v4/groups/{}/projects?per_page=100&order_by=updated_at&include_subgroups=true", base, group_id
            ), &mut all_repos, &mut seen).await {
                tracing::warn!("discover_repos: failed to list projects for GitLab group '{}': {}", group_name, e);
            }
        }
    }

    Ok(all_repos)
}

/// Paginate a GitLab projects endpoint and collect results.
async fn gitlab_collect_projects(
    client: &reqwest::Client,
    token: &str,
    base_url: &str,
    out: &mut Vec<RemoteRepo>,
    seen: &mut std::collections::HashSet<String>,
) -> Result<(), String> {
    let mut page = 1u32;
    loop {
        let url = format!("{}&page={}", base_url, page);
        let repos = gitlab_get_json_array(client, &url, token).await?;
        if repos.is_empty() { break; }
        let done = repos.len() < 100;
        for r in &repos {
            let full_name = r["path_with_namespace"].as_str().unwrap_or("").to_string();
            if seen.insert(full_name.clone()) {
                out.push(parse_gitlab_repo(r));
            }
        }
        if done { break; }
        page += 1;
    }
    Ok(())
}

/// Helper: GET a JSON array from GitLab API with auth headers.
async fn gitlab_get_json_array(client: &reqwest::Client, url: &str, token: &str) -> Result<Vec<serde_json::Value>, String> {
    let resp = client.get(url)
        .header("PRIVATE-TOKEN", token)
        .header("User-Agent", "Kronn/0.1")
        .send()
        .await
        .map_err(|e| format!("GitLab request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitLab API error {}: {}", status, body));
    }

    resp.json().await
        .map_err(|e| format!("Failed to parse GitLab response: {}", e))
}

/// Parse a GitLab project JSON object into a RemoteRepo.
fn parse_gitlab_repo(r: &serde_json::Value) -> RemoteRepo {
    RemoteRepo {
        name: r["name"].as_str().unwrap_or("").to_string(),
        full_name: r["path_with_namespace"].as_str().unwrap_or("").to_string(),
        clone_url: r["http_url_to_repo"].as_str().unwrap_or("").to_string(),
        ssh_url: r["ssh_url_to_repo"].as_str().unwrap_or("").to_string(),
        description: r["description"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string()),
        language: None, // GitLab doesn't include language in list endpoint
        stargazers_count: r["star_count"].as_u64().unwrap_or(0) as u32,
        updated_at: r["last_activity_at"].as_str().unwrap_or("").to_string(),
        source: "gitlab".into(),
        already_cloned: false,
    }
}

/// GET /api/projects/:id/ai-file?path=ai/index.md
/// Reads a single file from the `ai/` directory with path traversal protection.
pub async fn read_ai_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AiFileQuery>,
) -> Json<ApiResponse<AiFileContent>> {
    // Path traversal protection
    if query.path.contains("..") || !query.path.starts_with("ai/") {
        return Json(ApiResponse::err("Invalid path: must start with ai/ and not contain .."));
    }

    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();
    let file_path = query.path.clone();

    let result = tokio::task::spawn_blocking(move || {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let full_path = project_path.join(&file_path);
        match std::fs::read_to_string(&full_path) {
            Ok(content) => Ok(AiFileContent { path: file_path, content }),
            Err(e) => Err(format!("Cannot read file: {}", e)),
        }
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(content) => Json(ApiResponse::ok(content)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Git Operations
// ═══════════════════════════════════════════════════════════════════════════════

/// Helper: resolve a project's filesystem path from its DB id.
async fn resolve_project_path(state: &AppState, id: &str) -> Result<std::path::PathBuf, String> {
    let pid = id.to_string();
    let project = state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
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
) -> Json<ApiResponse<GitStatusResponse>> {
    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(p) => p,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let result = tokio::task::spawn_blocking(move || -> Result<GitStatusResponse, String> {
        let run = |args: &[&str]| -> Result<String, String> {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo_path)
                .output()
                .map_err(|e| format!("Failed to run git: {}", e))?;
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        };

        let run_with_status = |args: &[&str]| -> (String, bool) {
            match std::process::Command::new("git")
                .args(args)
                .current_dir(&repo_path)
                .output()
            {
                Ok(o) => (String::from_utf8_lossy(&o.stdout).trim().to_string(), o.status.success()),
                Err(_) => (String::new(), false),
            }
        };

        // Current branch
        let branch = run(&["branch", "--show-current"])?;

        // Default branch detection: try main, then master
        let default_branch = {
            let (_, ok_main) = run_with_status(&["rev-parse", "--verify", "main"]);
            if ok_main {
                "main".to_string()
            } else {
                let (_, ok_master) = run_with_status(&["rev-parse", "--verify", "master"]);
                if ok_master {
                    "master".to_string()
                } else {
                    String::new()
                }
            }
        };

        let is_default_branch = !default_branch.is_empty() && branch == default_branch;

        // Parse porcelain v1 status
        // -u shows individual files in untracked directories (not just the directory name)
        let status_output = run(&["status", "--porcelain=v1", "-u"])?;
        let files: Vec<GitFileStatus> = status_output
            .lines()
            .filter(|l| l.len() >= 3)
            .map(|line| {
                let bytes = line.as_bytes();
                let staged_char = bytes[0] as char;
                let unstaged_char = bytes[1] as char;
                let raw_path = line[3..].to_string();
                // Handle renamed files ("old -> new") — show the new path
                let path = if raw_path.contains(" -> ") {
                    raw_path.split(" -> ").last().unwrap_or(&raw_path).to_string()
                } else {
                    raw_path
                };
                // Strip quotes that git adds for special characters
                let path = path.trim_matches('"').to_string();

                // Determine status label
                let status = match (staged_char, unstaged_char) {
                    ('?', '?') => "untracked",
                    ('A', _) => "added",
                    ('D', _) | (_, 'D') => "deleted",
                    ('R', _) => "renamed",
                    ('M', _) | (_, 'M') => "modified",
                    ('C', _) => "copied",
                    _ => "modified",
                }.to_string();

                let staged = staged_char != ' ' && staged_char != '?';

                GitFileStatus { path, status, staged }
            })
            .collect();

        // Ahead/behind upstream
        let (ahead, behind) = {
            let (ab_output, ab_ok) = run_with_status(&["rev-list", "--count", "--left-right", "@{upstream}...HEAD"]);
            if ab_ok {
                let parts: Vec<&str> = ab_output.split_whitespace().collect();
                if parts.len() == 2 {
                    let b = parts[0].parse::<u32>().unwrap_or(0);
                    let a = parts[1].parse::<u32>().unwrap_or(0);
                    (a, b)
                } else {
                    (0, 0)
                }
            } else {
                (0, 0)
            }
        };

        Ok(GitStatusResponse {
            branch,
            default_branch,
            is_default_branch,
            files,
            ahead,
            behind,
        })
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(status) => Json(ApiResponse::ok(status)),
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
    let result = tokio::task::spawn_blocking(move || -> Result<GitDiffResponse, String> {
        let run_diff = |args: &[&str]| -> String {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&repo_path)
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default()
        };

        // Unstaged diff
        let unstaged = run_diff(&["diff", "--", &file_path]);
        // Staged diff
        let staged = run_diff(&["diff", "--cached", "--", &file_path]);

        // For untracked or newly added files, git diff returns nothing.
        // Show the file content as a full-add diff instead.
        let untracked_diff = if unstaged.is_empty() && staged.is_empty() {
            let full_path = repo_path.join(&file_path);
            if full_path.exists() {
                // Show file as new content (similar to git diff --no-index)
                match std::fs::read_to_string(&full_path) {
                    Ok(content) => {
                        let lines: Vec<String> = content.lines()
                            .map(|l| format!("+{}", l))
                            .collect();
                        if lines.is_empty() {
                            String::new()
                        } else {
                            format!("--- /dev/null\n+++ b/{}\n@@ -0,0 +1,{} @@\n{}",
                                file_path, lines.len(), lines.join("\n"))
                        }
                    }
                    Err(_) => String::new(), // Binary or unreadable
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        // Combine all diffs
        let diff = if !staged.is_empty() && !unstaged.is_empty() {
            format!("--- Staged ---\n{}\n--- Unstaged ---\n{}", staged, unstaged)
        } else if !staged.is_empty() {
            staged
        } else if !unstaged.is_empty() {
            unstaged
        } else {
            untracked_diff
        };

        Ok(GitDiffResponse { path: file_path, diff })
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(diff) => Json(ApiResponse::ok(diff)),
        Err(e) => Json(ApiResponse::err(e)),
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
        let output = std::process::Command::new("git")
            .args(["checkout", "-b", &branch_name])
            .current_dir(&repo_path)
            .output()
            .map_err(|e| format!("Failed to run git: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git checkout -b failed: {}", stderr.trim()));
        }

        Ok(GitBranchResponse { branch: branch_name })
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

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
    // Validate file paths (no path traversal)
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
    let result = tokio::task::spawn_blocking(move || -> Result<GitCommitResponse, String> {
        // git add each file individually, skip missing files gracefully
        let mut added = 0;
        for file in &files {
            // Strip quotes that may come from the frontend
            let clean_file = file.trim_matches('"');
            let file_abs = repo_path.join(clean_file);

            if file_abs.exists() {
                // Existing file: git add
                let add_output = std::process::Command::new("git")
                    .args(["add", "--", clean_file])
                    .current_dir(&repo_path)
                    .output()
                    .map_err(|e| format!("Failed to run git add: {}", e))?;
                if add_output.status.success() {
                    added += 1;
                } else {
                    tracing::warn!("git add skipped '{}': {}", clean_file,
                        String::from_utf8_lossy(&add_output.stderr).trim());
                }
            } else {
                // Deleted file: stage the deletion
                let rm_output = std::process::Command::new("git")
                    .args(["rm", "--cached", "--ignore-unmatch", "--", clean_file])
                    .current_dir(&repo_path)
                    .output();
                if rm_output.map(|o| o.status.success()).unwrap_or(false) {
                    added += 1;
                }
            }
        }
        if added == 0 {
            return Err("No files could be staged".to_string());
        }

        // Ensure git identity is set (Docker container may not have one)
        let has_user = std::process::Command::new("git")
            .args(["config", "user.name"])
            .current_dir(&repo_path)
            .output()
            .map(|o| o.status.success() && !o.stdout.is_empty())
            .unwrap_or(false);
        if !has_user {
            let _ = std::process::Command::new("git")
                .args(["config", "user.name", "Kronn"])
                .current_dir(&repo_path).status();
            let _ = std::process::Command::new("git")
                .args(["config", "user.email", "kronn@localhost"])
                .current_dir(&repo_path).status();
        }

        // git commit -m <message>
        let commit_output = std::process::Command::new("git")
            .args(["commit", "-m", &message])
            .current_dir(&repo_path)
            .output()
            .map_err(|e| format!("Failed to run git commit: {}", e))?;

        if !commit_output.status.success() {
            let stderr = String::from_utf8_lossy(&commit_output.stderr);
            return Err(format!("git commit failed: {}", stderr.trim()));
        }

        // Get the commit hash
        let hash_output = std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(&repo_path)
            .output()
            .map_err(|e| format!("Failed to get commit hash: {}", e))?;

        let hash = String::from_utf8_lossy(&hash_output.stdout).trim().to_string();

        Ok(GitCommitResponse { hash, message })
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

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

    let result = tokio::task::spawn_blocking(move || -> Result<GitPushResponse, String> {
        // Get current branch
        let branch_output = std::process::Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(&repo_path)
            .output()
            .map_err(|e| format!("Failed to get branch: {}", e))?;

        let branch = String::from_utf8_lossy(&branch_output.stdout).trim().to_string();
        if branch.is_empty() {
            return Err("Cannot determine current branch (detached HEAD?)".to_string());
        }

        // git push -u origin <branch>
        let push_output = std::process::Command::new("git")
            .args(["push", "-u", "origin", &branch])
            .current_dir(&repo_path)
            .output()
            .map_err(|e| format!("Failed to run git push: {}", e))?;

        if push_output.status.success() {
            let stdout = String::from_utf8_lossy(&push_output.stdout);
            let stderr = String::from_utf8_lossy(&push_output.stderr);
            // git push often writes progress to stderr even on success
            let msg = if !stdout.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                stderr.trim().to_string()
            };
            Ok(GitPushResponse {
                success: true,
                message: msg,
            })
        } else {
            let stderr = String::from_utf8_lossy(&push_output.stderr);
            Ok(GitPushResponse {
                success: false,
                message: stderr.trim().to_string(),
            })
        }
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

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

    // Block dangerous commands
    let first_word = cmd.split_whitespace().next().unwrap_or("");
    const BLOCKED: &[&str] = &["rm", "sudo", "chmod", "chown", "kill", "reboot", "shutdown", "mkfs", "dd"];
    if BLOCKED.contains(&first_word) {
        return Json(ApiResponse::err(format!("Command '{}' is not allowed", first_word)));
    }

    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(p) => p,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let result = tokio::task::spawn_blocking(move || -> Result<ExecResponse, String> {
        let output = std::process::Command::new("sh")
            .args(["-c", &cmd])
            .current_dir(&repo_path)
            .output()
            .map_err(|e| format!("Failed to execute: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // Add helpful hint when a command is not found
        if !output.status.success() && (stderr.contains("not found") || stderr.contains("No such file")) {
            stderr.push_str(
                "\n\n💡 Commande introuvable. Le terminal s'exécute dans le container Docker \
                avec accès aux binaires du host (/usr/bin). Si l'outil est installé ailleurs, \
                vérifiez votre PATH ou installez-le dans le container."
            );
        }

        Ok(ExecResponse {
            stdout,
            stderr,
            exit_code: output.status.code().unwrap_or(-1),
        })
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(resp) => Json(ApiResponse::ok(resp)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}
