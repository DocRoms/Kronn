use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::agents::runner;
use crate::core::cmd::sync_cmd;
use crate::core::scanner;
use crate::models::*;
use crate::AppState;

/// Read briefing notes: try ai/briefing.md from the filesystem first, fall back to DB field.
pub(crate) fn resolve_briefing_notes(project_path: &std::path::Path, db_notes: &Option<String>) -> Option<String> {
    let briefing_file = project_path.join("ai/briefing.md");
    if let Ok(content) = std::fs::read_to_string(&briefing_file) {
        if !content.trim().is_empty() {
            return Some(content);
        }
    }
    db_notes.clone()
}

/// Populate audit_status and ai_todo_count on a project (computed from filesystem)
pub(crate) fn enrich_audit_status(project: &mut Project) {
    project.audit_status = scanner::detect_audit_status(&project.path);
    project.ai_todo_count = scanner::count_ai_todos(&project.path);
}

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
        briefing_notes: None,
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
            let status = sync_cmd("git")
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
                for filename in super::audit::AUDIT_REDIRECTOR_FILES {
                    let src = template_dir.join(filename);
                    let dst = project_path.join(filename);
                    if src.exists() && !dst.exists() {
                        if let Some(parent) = dst.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::copy(&src, &dst);
                    }
                }
                // Kiro steering (nested path, not in super::audit::AUDIT_REDIRECTOR_FILES)
                let kiro_src = template_dir.join(".kiro/steering/instructions.md");
                let kiro_dst = project_path.join(".kiro/steering/instructions.md");
                if kiro_src.exists() && !kiro_dst.exists() {
                    // Safety: kiro_dst is a multi-segment path (.kiro/steering/instructions.md), parent() cannot be None
                    let _ = std::fs::create_dir_all(kiro_dst.parent().expect("kiro_dst has a parent"));
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
        briefing_notes: None,
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
            if let Err(e) = state.db.with_conn(move |conn| {
                crate::core::mcp_scanner::sync_affected_projects(conn, &[pid], &secret);
                Ok::<_, anyhow::Error>(())
            }).await {
                tracing::error!("Failed to sync MCP config for new project: {e}");
            }
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
        model_tier: None, cost_usd: None, author_pseudo: None, author_avatar_email: None,
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
            "entrepreneur".into(),
        ],
        directive_ids: vec![],
        archived: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        tier: crate::models::ModelTier::Default,
        pin_first_message: true,
        worktree_branch: None,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        shared_id: None,
        shared_with: vec![],
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
    match language {
        "en" => format!(
r#"# Bootstrap for project "{project_name}"

Respond in English.

## Project description
{description}

## Your mission
You are a software architect and product owner. Help me build this project from scratch, step by step.

Start by analyzing the description above, then guide me through the following steps:

### 1. Vision & Goals
- Restate the project vision in 2-3 clear sentences
- Identify target users
- List the 3-5 main goals

### 2. Technical architecture
- Propose a suitable tech stack (frontend, backend, DB, infra)
- Justify each choice
- Draw the architecture in ASCII if relevant

### 3. Project structure
- Propose a file/folder tree
- Explain naming conventions

### 4. MVP — Priority features
- List the features for a functional MVP
- Prioritize them (P0 = essential, P1 = important, P2 = nice-to-have)
- Estimate relative complexity for each feature

### 5. Action plan
- Propose a sequential development plan
- Identify dependencies between features
- Suggest the first files to create

### 6. Finalization
- When you have completed all steps, write exactly `KRONN:BOOTSTRAP_COMPLETE` in your final message.

Start now with step 1. Ask me questions if the description lacks details."#
        ),
        "es" => format!(
r#"# Bootstrap del proyecto "{project_name}"

Responde en español.

## Descripción del proyecto
{description}

## Tu misión
Eres un arquitecto de software y product owner. Ayúdame a construir este proyecto desde cero, paso a paso.

Comienza analizando la descripción anterior y luego guíame a través de los siguientes pasos:

### 1. Visión y objetivos
- Reformula la visión del proyecto en 2-3 frases claras
- Identifica los usuarios objetivo
- Lista los 3-5 objetivos principales

### 2. Arquitectura técnica
- Propón un stack técnico adecuado (frontend, backend, DB, infra)
- Justifica cada elección
- Dibuja la arquitectura en ASCII si es pertinente

### 3. Estructura del proyecto
- Propón un árbol de archivos/carpetas
- Explica las convenciones de nombres

### 4. MVP — Features prioritarias
- Lista las features para un MVP funcional
- Priorízalas (P0 = indispensable, P1 = importante, P2 = nice-to-have)
- Estima la complejidad relativa de cada feature

### 5. Plan de acción
- Propón un plan de desarrollo secuencial
- Identifica las dependencias entre features
- Sugiere los primeros archivos a crear

### 6. Finalización
- Cuando hayas terminado todos los pasos, escribe exactamente `KRONN:BOOTSTRAP_COMPLETE` en tu último mensaje.

Comienza ahora por el paso 1. Hazme preguntas si la descripción carece de detalles."#
        ),
        _ => format!(
r#"# Bootstrap du projet "{project_name}"

Réponds en français.

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
        ),
    }
}

/// Inject a token into an HTTPS git URL for authenticated cloning.
/// Returns the original URL unchanged if it's not HTTPS or no matching provider is found.
fn inject_token_into_url(url: &str, provider: &str, token: &str) -> Option<String> {
    if !url.starts_with("https://") {
        return None;
    }
    match provider {
        "github" if url.contains("github.com") => {
            Some(url.replacen("https://github.com", &format!("https://x-access-token:{}@github.com", token), 1))
        }
        "gitlab" if url.contains("gitlab") => {
            let real_token = token.split('|').next().unwrap_or(token);
            url.find("://").map(|i| {
                let after_scheme = &url[i + 3..];
                format!("https://oauth2:{}@{}", real_token, after_scheme)
            })
        }
        _ => None,
    }
}

/// Convert an HTTPS git URL to its SSH equivalent.
fn https_to_ssh(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let slash_pos = rest.find('/')?;
    let host = &rest[..slash_pos];
    let path = &rest[slash_pos + 1..];
    Some(format!("git@{}:{}", host, path))
}

/// For HTTPS clone URLs, inject a Personal Access Token into the URL so that
/// `git clone` works inside Docker where no interactive credential helper is
/// available.  Falls back to converting HTTPS → SSH when keys are mounted.
async fn inject_clone_auth(url: &str, state: &AppState) -> String {
    if !url.starts_with("https://") {
        return url.to_string();
    }

    let sources = super::discover::find_all_provider_sources(state).await;

    // Try to inject a token from configured MCP sources
    for (source, token) in &sources {
        if let Some(authed_url) = inject_token_into_url(url, &source.provider, token) {
            return authed_url;
        }
    }

    // No token found — try SSH fallback if SSH keys are available
    if url.contains("github.com") || url.contains("gitlab.com") {
        let ssh_dir = std::env::var("HOME").ok().map(|h| std::path::PathBuf::from(h).join(".ssh"));
        let has_ssh_keys = ssh_dir
            .map(|d| d.join("id_rsa").exists() || d.join("id_ed25519").exists())
            .unwrap_or(false);
        if has_ssh_keys {
            if let Some(ssh_url) = https_to_ssh(url) {
                return ssh_url;
            }
        }
    }

    url.to_string()
}

#[cfg(test)]
mod clone_auth_tests {
    use super::*;

    #[test]
    fn inject_github_token() {
        let url = "https://github.com/org/repo.git";
        let result = inject_token_into_url(url, "github", "ghp_abc123").unwrap();
        assert_eq!(result, "https://x-access-token:ghp_abc123@github.com/org/repo.git");
    }

    #[test]
    fn inject_gitlab_token() {
        let url = "https://gitlab.com/org/repo.git";
        let result = inject_token_into_url(url, "gitlab", "glpat-xyz|https://gitlab.com").unwrap();
        assert_eq!(result, "https://oauth2:glpat-xyz@gitlab.com/org/repo.git");
    }

    #[test]
    fn inject_gitlab_token_no_pipe() {
        let url = "https://gitlab.example.com/org/repo.git";
        let result = inject_token_into_url(url, "gitlab", "glpat-xyz").unwrap();
        assert_eq!(result, "https://oauth2:glpat-xyz@gitlab.example.com/org/repo.git");
    }

    #[test]
    fn inject_wrong_provider_returns_none() {
        let url = "https://github.com/org/repo.git";
        assert!(inject_token_into_url(url, "gitlab", "token").is_none());
    }

    #[test]
    fn inject_ssh_url_returns_none() {
        let url = "git@github.com:org/repo.git";
        assert!(inject_token_into_url(url, "github", "token").is_none());
    }

    #[test]
    fn https_to_ssh_github() {
        let url = "https://github.com/org/repo.git";
        assert_eq!(https_to_ssh(url).unwrap(), "git@github.com:org/repo.git");
    }

    #[test]
    fn https_to_ssh_gitlab() {
        let url = "https://gitlab.com/group/subgroup/repo.git";
        assert_eq!(https_to_ssh(url).unwrap(), "git@gitlab.com:group/subgroup/repo.git");
    }

    #[test]
    fn https_to_ssh_not_https() {
        assert!(https_to_ssh("git@github.com:org/repo.git").is_none());
    }
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

    // Git clone — inject auth token for HTTPS URLs when available
    let clone_url = inject_clone_auth(&url, &state).await;
    let original_url = url.clone();
    let clone_path = host_path.clone();
    let clone_path2 = host_path.clone();
    let clone_result = tokio::task::spawn_blocking(move || {
        sync_cmd("git")
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

    // Reset the remote URL to the original (without embedded token) so that
    // secrets don't persist in .git/config and don't leak via git remote scans.
    let _ = tokio::task::spawn_blocking(move || {
        sync_cmd("git")
            .args(["remote", "set-url", "origin", &original_url])
            .current_dir(&clone_path2)
            .output()
    }).await;

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
        briefing_notes: None,
        created_at: now,
        updated_at: now,
    };
    enrich_audit_status(&mut project);

    let p = project.clone();
    if let Err(e) = state.db.with_conn(move |conn| crate::db::projects::insert_project(conn, &p)).await {
        return Json(ApiResponse::err(format!("DB error: {}", e)));
    }

    // Auto-detect skills
    let detected = super::audit::detect_project_skills(&host_path);
    if !detected.is_empty() {
        let pid = project_id.clone();
        let skills = detected.clone();
        if let Err(e) = state.db.with_conn(move |conn| {
            crate::db::projects::update_project_default_skills(conn, &pid, &skills)
        }).await {
            tracing::error!("Failed to update project default skills: {e}");
        }
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
            if let Err(e) = super::audit::check_ai_dir_permissions(&ai_target) {
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
pub(crate) fn resolve_templates_dir() -> std::path::PathBuf {
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
pub(crate) fn copy_dir_nondestructive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
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
pub(crate) fn inject_bootstrap_prompt(index_file: &std::path::Path) {
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


// Git Operations
// ═══════════════════════════════════════════════════════════════════════════════

/// Helper: resolve a project's filesystem path from its DB id.
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
        let output = sync_cmd("git")
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

    let github_token = resolve_github_token_from_state(&state).await;
    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_git_push(&repo_path, github_token.as_deref())
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

    let repo_path = match resolve_project_path(&state, &id).await {
        Ok(p) => p,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    // Rate-limit concurrent exec calls via the shared agent semaphore
    let _permit = match state.agent_semaphore.acquire().await {
        Ok(p) => p,
        Err(_) => return Json(ApiResponse::err("Server is shutting down")),
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
    let github_token = resolve_github_token_from_state(&state).await;
    let result = tokio::task::spawn_blocking(move || {
        super::git_ops::run_create_pr(&repo_path, &title, &body, &base, github_token.as_deref())
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

    let branch = sync_cmd("git")
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

#[cfg(test)]
mod prompt_tests {
    use super::*;

    #[test]
    fn bootstrap_prompt_is_localized() {
        let fr = build_bootstrap_prompt("fr", "TestProj", "A test project");
        let en = build_bootstrap_prompt("en", "TestProj", "A test project");
        let es = build_bootstrap_prompt("es", "TestProj", "A test project");
        // All should contain the project name
        assert!(fr.contains("TestProj"));
        assert!(en.contains("TestProj"));
        assert!(es.contains("TestProj"));
        // They should be different from each other
        assert_ne!(fr, en, "FR and EN bootstrap prompts must differ");
        assert_ne!(en, es, "EN and ES bootstrap prompts must differ");
    }

    #[test]
    fn bootstrap_prompt_contains_completion_signal() {
        for lang in ["fr", "en", "es"] {
            let prompt = build_bootstrap_prompt(lang, "P", "d");
            assert!(prompt.contains("KRONN:BOOTSTRAP_COMPLETE"),
                "Bootstrap prompt ({}) must contain completion signal", lang);
        }
    }

}
