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
                for filename in AUDIT_REDIRECTOR_FILES {
                    let src = template_dir.join(filename);
                    let dst = project_path.join(filename);
                    if src.exists() && !dst.exists() {
                        if let Some(parent) = dst.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::copy(&src, &dst);
                    }
                }
                // Kiro steering (nested path, not in AUDIT_REDIRECTOR_FILES)
                let kiro_src = template_dir.join(".kiro/steering/instructions.md");
                let kiro_dst = project_path.join(".kiro/steering/instructions.md");
                if kiro_src.exists() && !kiro_dst.exists() {
                    let _ = std::fs::create_dir_all(kiro_dst.parent().unwrap());
                    let _ = std::fs::copy(&kiro_src, &kiro_dst);
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
        model_tier: None,
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
        workspace_mode: "Direct".into(),
        workspace_path: None,
        tier: crate::models::ModelTier::Default,
        worktree_branch: None,
        summary_cache: None,
        summary_up_to_msg_idx: None,
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
> 9. **`ai/inconsistencies-tech-debt.md`** — Scan source code across: dependencies (EOL/deprecated),
>    security (secrets, injection, auth), code quality (complexity, SRP, dead code), scalability (N+1, leaks),
>    maintainability (coupling, missing tests), compliance (GDPR, licenses), infrastructure (Docker, CI).
>    Create `ai/tech-debt/TD-*.md` detail files for each entry. Cite file paths.
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
- Replace ALL `{{PLACEHOLDERS}}` and `<!-- ... -->` comment placeholders with real content. {{PLACEHOLDERS}} are literal text markers — replace by editing file content directly.\n\
- Keep the existing file structure and section headings — fill in the blanks, do NOT rewrite the file from scratch.\n\
- If a section does not apply to this project, replace placeholders with 'N/A — not used in this project.' Do not delete the section.\n\
- Write plain facts, not opinions or recommendations. No debate, no trade-offs analysis.\n\
- Each section should be self-contained: another AI agent reading just that section should get the full picture.\n\
- Add or remove table rows as needed to match the project. Write fewer entries rather than inventing content to fill slots.";

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
Read ai/index.md. Search codebase for domain terms, abbreviations, naming conventions.\n\n\
Fill ai/glossary.md — replace ALL {{PLACEHOLDERS}}:\n\
- Categorize: Architecture, Domain, Environments, External, Abbreviations\n\
- Each term: one-line definition + optional ai/ reference\n\
- Unknown domain terms: add `<!-- TODO: ask user -->` after the definition\n\
- Cover: framework terms, model names, services, acronyms in code"),

    // Step 3: Repo map
    ("ai/repo-map.md", "\
Read ai/index.md and ai/glossary.md for context. Explore the directory structure (2-3 levels deep).\n\n\
Fill ai/repo-map.md — replace ALL {{PLACEHOLDERS}}:\n\
- {{STACK_OVERVIEW}}: one paragraph summarizing the architecture\n\
- Key folders tree: replace {{FOLDER_*}} with every major directory (2-3 levels deep), tree format with annotations\n\
- Entrypoints table: replace {{ENTRYPOINT_*}} with 5-7 key files (config, routes, models, etc.)\n\
- Auto-generated files: replace {{FILE_PATTERN}} with files NOT to edit manually"),

    // Step 4: Coding rules
    ("ai/coding-rules.md", "\
Read ai/index.md for context. Find ALL linter, formatter, and type-checker configs in the repo \
(e.g. .eslintrc, eslint.config.js, prettier, rustfmt.toml, tsconfig.json, phpcs.xml, etc.).\n\n\
Fill ai/coding-rules.md — replace ALL {{PLACEHOLDERS}}:\n\
- Replace {{LANGUAGE_*}} with one section per language/framework used in the project\n\
- For each language, fill the Tools table: {{CONFIG}} and {{COMMAND}} for linter, formatter, type checker\n\
- Replace {{CONVENTION_*}} with coding conventions OBSERVED in existing code (naming, error handling, imports). Write fewer rather than inventing.\n\
- Replace {{MISTAKE_*}} with common mistakes to avoid (from linter configs, framework gotchas observed in code)\n\
- If no linter/formatter is configured, write 'Not configured' in the Config column\n\
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
Real issues only, not hypothetical. Read all ai/ files AND scan source code.\n\
Scan: entry points, config files, Dockerfiles, CI configs, and 5-10 core source files \
(prioritize auth, data persistence, external input handling).\n\n\
Systematically audit across these dimensions:\n\
- Dependencies: EOL/deprecated runtimes, frameworks, packages, or versions significantly behind stable\n\
- Security: hardcoded secrets, missing auth checks, injection vectors (SQL/XSS), insecure defaults, exposed debug endpoints\n\
- Code quality: functions >50 lines, god classes, SRP violations, dead code, error swallowing (empty catch/let _ =)\n\
- Scalability: N+1 queries, unbounded loops, missing pagination, memory leaks, missing indexes\n\
- Maintainability: tight coupling, circular dependencies, missing tests for critical paths, unclear naming\n\
- Compliance: GDPR issues (external resources, data retention), license incompatibilities\n\
- Infrastructure: Docker misconfigs (root user, no resource limits), CI gaps, missing health checks\n\n\
Fill ai/inconsistencies-tech-debt.md — replace ALL {{PLACEHOLDERS}} and <!-- ... --> comments:\n\
1. Outdated prerequisites table: flag EOL/deprecated/behind-stable runtimes, frameworks, packages\n\
2. For each issue found: (a) create `ai/tech-debt/TD-YYYYMMDD-slug.md` (YYYYMMDD=today) first, \
then (b) add the one-line entry to the Current list table. Do both or neither.\n\
   Severity: Critical=security/data loss, High=blocks prod, Medium=dev friction/perf, Low=cosmetic. Cite file paths.\n\
3. Limit to 15-20 most impactful findings. Prioritize Critical and High.\n\
4. No issues found → single row: 'None identified during initial audit'\n\n\
Detail file format:\n\
- **ID**: TD-YYYYMMDD-slug\n\
- **Area**: Backend | Frontend | CI | Infra | Security | Docs\n\
- **Severity**: Critical | High | Medium | Low\n\
- **Problem (fact)**: one-line description\n\
- **Impact**: what goes wrong if not fixed\n\
- **Where (pointers)**: file paths with line numbers\n\
- **Suggested direction**: non-binding fix suggestion\n\
- **Next step**: create ticket"),

    // Step 10: Final review
    ("REVIEW", "\
Read ALL ai/ files. Final quality pass — fix issues directly.\n\n\
Check: no remaining `{{` placeholders · no orphan `<!-- fill -->` comments (keep `<!-- TODO: ask user -->`) \
· no duplicated facts · consistent terminology with glossary · valid cross-references \
· no contradictions · no empty critical sections · clean markdown · each tech-debt entry has a detail file \
· TODO markers are genuine unknowns.\n\n\
Empty sections for missing features → 'N/A — not used'."),
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
            let file_label = if *target_file == "REVIEW" { "Final review" } else { target_file };

            let step_start = serde_json::json!({
                "step": step,
                "total": total_steps,
                "file": file_label
            });
            yield Event::default().event("step_start").data(step_start.to_string());

            // Inject today's date so agents don't have to guess it
            let today = Utc::now().format("%Y-%m-%d").to_string();
            let today_compact = Utc::now().format("%Y%m%d").to_string();
            let full_prompt = format!("{}\n\n{}", PROMPT_PREAMBLE, step_prompt)
                .replace("YYYYMMDD=today", &format!("YYYYMMDD={}", today_compact))
                .replace("today's date (YYYY-MM-DD)", &today)
                .replace("set to today's date", &format!("set to {}", today));

            // No profiles for audit — solo agent mode produces clean factual documentation.
            // Multi-profile debate format would pollute ai/ files with discussion artifacts.

            // Always use full_access for audit (agent needs to write files)
            match runner::start_agent_with_config(runner::AgentStartConfig {
                agent_type: &agent_type, project_path: &project_path_str, work_dir: None,
                prompt: &full_prompt, tokens: &tokens, full_access: true,
                skill_ids: &[], directive_ids: &[], profile_ids: &[],
                mcp_context_override: None,
                tier: crate::models::ModelTier::Reasoning, model_tiers: None,
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
                "Validate the AI context (ai/ folder). Follow this 4-phase protocol. ",
                "Do NOT emit KRONN:VALIDATION_COMPLETE until ALL phases are done.\n\n",
                "## Phase 1 — Auto-fix (autonomous)\n",
                "Read source code. Fix autonomously: orphan TODO markers, empty/skeleton files inferable from code, outdated info. ",
                "Update ai/ files directly. Report fixes.\n\n",
                "## Phase 2 — Ambiguity questions (interactive)\n",
                "Ask remaining ambiguities **one by one**. After each answer, update ai/ files immediately. ",
                "Follow-up on new unknowns. Do not skip to Phase 3 until resolved.\n",
                "If user answers 'I don't know' or 'skip', mark as `<!-- TODO: unknown -->` and move on.\n",
                "Phase 2 ends when all TODOs are addressed or explicitly skipped.\n\n",
                "## Phase 3 — Tech debt review (interactive)\n",
                "For each entry in `ai/inconsistencies-tech-debt.md`:\n",
                "1. Read its detail file in `ai/tech-debt/`\n",
                "2. Verify against source code — does the issue still exist? Is the description accurate?\n",
                "3. Present to user one by one (or grouped by area if >10). Ask: confirm/reject? correct severity? priority?\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str("Also ask: create a ticket? (issue tracker available via MCP)\n");
            }
            s.push_str(concat!(
                "Do not batch-confirm. Update/remove entries per feedback.\n",
                "Also ask: did the audit miss anything obvious? (security, performance, compliance)\n\n",
                "## Phase 4 — Doc challenge (interactive)\n",
                "Ask 2-3 practical onboarding questions that must be answerable from ai/ files alone. ",
                "Examples: 'How would a new dev add a new API endpoint?', 'What command runs all tests?', 'Where is the DB schema?'. ",
                "Check if ai/ docs answer them correctly. Fix gaps.\n\n",
                "## Completion\n",
                "All phases done → end with exact phrase: \"KRONN:VALIDATION_COMPLETE\". Never emit early.",
            ));
            s
        },
        "es" => {
            let mut s = String::from(concat!(
                "Valida el contexto AI (carpeta ai/). Sigue este protocolo de 4 fases. ",
                "NO emitas KRONN:VALIDATION_COMPLETE hasta completar TODAS las fases.\n\n",
                "## Fase 1 — Auto-correccion (autonoma)\n",
                "Lee el codigo. Corrige: TODOs huerfanos, archivos esqueleto inferibles del codigo, info obsoleta. ",
                "Actualiza ai/ directamente. Reporta.\n\n",
                "## Fase 2 — Preguntas (interactiva)\n",
                "Pregunta ambiguedades **una por una**. Tras cada respuesta, actualiza ai/ antes de seguir. ",
                "No pases a Fase 3 sin resolver todo.\n",
                "Si el usuario responde 'no se' o 'saltar', marca como `<!-- TODO: unknown -->` y continua.\n\n",
                "## Fase 3 — Deuda tecnica (interactiva)\n",
                "Para cada entrada en `ai/inconsistencies-tech-debt.md`:\n",
                "1. Lee su archivo detalle en `ai/tech-debt/`\n",
                "2. Verifica contra el codigo fuente — ¿el problema existe? ¿la descripcion es correcta?\n",
                "3. Presenta al usuario una por una (o agrupadas por area si >10). Pregunta: ¿confirmar/rechazar? ¿severidad? ¿prioridad?\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str("Tambien: ¿crear ticket? (gestor de issues disponible via MCP)\n");
            }
            s.push_str(concat!(
                "No confirmar en lote. Actualiza/elimina segun feedback.\n",
                "Tambien pregunta: ¿la auditoria omitio algo obvio? (seguridad, rendimiento, cumplimiento)\n\n",
                "## Fase 4 — Challenge doc (interactiva)\n",
                "Haz 2-3 preguntas practicas de onboarding que deben ser respondibles solo con los archivos ai/. ",
                "Ejemplos: '¿Como agregar un endpoint?', '¿Que comando ejecuta los tests?'. Corrige gaps.\n\n",
                "## Fin\n",
                "Todas las fases completas → termina con: \"KRONN:VALIDATION_COMPLETE\". Nunca antes.",
            ));
            s
        },
        _ => {
            let mut s = String::from(concat!(
                "Valide le contexte AI (dossier ai/). Suis ce protocole en 4 phases. ",
                "NE PAS emettre KRONN:VALIDATION_COMPLETE avant la fin des 4 phases.\n\n",
                "## Phase 1 — Auto-correction (autonome)\n",
                "Lis le code source. Corrige : TODOs orphelins, fichiers squelettes inferables du code, infos obsoletes. ",
                "Mets a jour ai/ directement. Rapporte les corrections.\n\n",
                "## Phase 2 — Questions (interactif)\n",
                "Pose les ambiguites **une par une**. Apres chaque reponse, mets a jour ai/ avant la question suivante. ",
                "Ne passe pas a la Phase 3 sans tout resoudre.\n",
                "Si l'utilisateur repond 'je ne sais pas' ou 'passer', marque `<!-- TODO: unknown -->` et continue.\n\n",
                "## Phase 3 — Dette technique (interactif)\n",
                "Pour chaque entree dans `ai/inconsistencies-tech-debt.md` :\n",
                "1. Lis son fichier detail dans `ai/tech-debt/`\n",
                "2. Verifie dans le code source — le probleme existe-t-il ? La description est-elle exacte ?\n",
                "3. Presente a l'utilisateur un par un (ou par domaine si >10). Demande : confirmer/rejeter ? severite ? priorite ?\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str("Aussi : creer un ticket ? (gestionnaire d'issues dispo via MCP)\n");
            }
            s.push_str(concat!(
                "Pas de confirmation en lot. Mets a jour/supprime selon feedback.\n",
                "Demande aussi : l'audit a-t-il rate quelque chose d'evident ? (securite, performance, conformite)\n\n",
                "## Phase 4 — Challenge doc (interactif)\n",
                "Pose 2-3 questions pratiques d'onboarding qui doivent etre couvertes par les fichiers ai/ seuls. ",
                "Exemples : 'Comment ajouter un endpoint ?', 'Quelle commande lance les tests ?'. Corrige les lacunes.\n\n",
                "## Fin\n",
                "Toutes les phases terminees → termine par : \"KRONN:VALIDATION_COMPLETE\". Jamais avant.",
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
            let file_label = if *target_file == "REVIEW" { "Final review" } else { target_file };

            let step_start = serde_json::json!({
                "step": step, "total": total_steps, "file": file_label
            });
            yield Event::default().event("step_start").data(step_start.to_string());

            let today = Utc::now().format("%Y-%m-%d").to_string();
            let today_compact = Utc::now().format("%Y%m%d").to_string();
            let full_prompt = format!("{}\n\n{}", PROMPT_PREAMBLE, step_prompt)
                .replace("YYYYMMDD=today", &format!("YYYYMMDD={}", today_compact))
                .replace("today's date (YYYY-MM-DD)", &today)
                .replace("set to today's date", &format!("set to {}", today));

            match runner::start_agent_with_config(runner::AgentStartConfig {
                agent_type: &agent_type, project_path: &project_path_str, work_dir: None,
                prompt: &full_prompt, tokens: &tokens, full_access: true,
                skill_ids: &[], directive_ids: &[], profile_ids: &[],
                mcp_context_override: None,
                tier: crate::models::ModelTier::Reasoning, model_tiers: None,
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
            model_tier: None,
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
            tier: crate::models::ModelTier::Default,
            archived: false,
            workspace_mode: "Direct".into(),
            workspace_path: None,
            worktree_branch: None,
            summary_cache: None,
            summary_up_to_msg_idx: None,
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
const AUDIT_REDIRECTOR_FILES: &[&str] = &[
    "CLAUDE.md", "GEMINI.md", "AGENTS.md",
    ".cursorrules", ".windsurfrules", ".clinerules",
    ".github/copilot-instructions.md",
];

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

    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_git_status(&repo_path)
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
    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_git_diff(&repo_path, &file_path)
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
        super::git_ops::run_git_commit(&repo_path, &files, &message, amend, sign)
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

    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_git_push(&repo_path)
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

    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_exec(&repo_path, &cmd)
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

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
    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_create_pr(&repo_path, &title, &body, &base)
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

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

    let branch = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(&repo_path)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let template = super::git_ops::read_pr_template(&repo_path)
        .unwrap_or_else(|| super::git_ops::default_pr_template(&branch));

    let source = if super::git_ops::read_pr_template(&repo_path).is_some() {
        "project"
    } else {
        "kronn"
    };

    Json(ApiResponse::ok(serde_json::json!({
        "template": template,
        "source": source,
    })))
}
