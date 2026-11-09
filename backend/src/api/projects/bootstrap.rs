// `POST /api/projects/bootstrap` + the localized prompt builders that
// seed the very first user message of the bootstrap discussion. Two
// flavours: classic (6-step linear) and Bootstrap++ (defers to the
// `bootstrap-architect` skill for gated multi-stage work).

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::agents::runner;
use crate::core::scanner;
use crate::models::*;
use crate::AppState;

use super::{
    determine_parent_dir, enrich_audit_status,
    template::{ensure_agent_writable_subfolders, resolve_templates_dir},
};

/// POST /api/projects/bootstrap
/// Create a new project from scratch: create dir, git init, install template, create bootstrap discussion.
pub async fn bootstrap(
    State(state): State<AppState>,
    Json(req): Json<BootstrapProjectRequest>,
) -> Response {
    let mut detected = crate::agents::detect_all_cached(false).await;
    crate::agents::apply_configured_status(&mut detected, &*state.config.read().await);
    bootstrap_as(state, req, Uuid::new_v4().to_string(), &detected)
        .await
        .into_response()
}

/// Why `agent` cannot run a bootstrap, mirroring the form's `canAudit`: an
/// audit-capable type that is installed (or reachable), enabled and signed in.
pub(crate) fn bootstrap_agent_refusal(
    agent: &AgentType,
    detected: &[AgentDetection],
) -> Option<String> {
    if !crate::api::audit::agent_can_audit(agent) {
        return Some(format!(
            "{agent:?} cannot run a bootstrap: it has no file tools Kronn can scope to the new project."
        ));
    }
    let Some(detection) = detected.iter().find(|d| &d.agent_type == agent) else {
        return Some(format!("{agent:?} is not available on this host."));
    };
    if !(detection.installed || detection.runtime_available) {
        return Some(format!("{agent:?} is not installed."));
    }
    if !detection.enabled {
        return Some(format!(
            "{agent:?} is disabled. Enable it in Config › Agents or choose another agent."
        ));
    }
    if detection.auth_ready == Some(false) {
        return Some(format!(
            "{agent:?} is not signed in. Finish its setup in Config › Agents or choose another agent."
        ));
    }
    None
}

/// `bootstrap` under a caller-chosen project id and agent detection, so a test
/// can hold that project's audit lease or stand in for the host's agents.
pub(crate) async fn bootstrap_as(
    state: AppState,
    req: BootstrapProjectRequest,
    project_id: String,
    detected: &[AgentDetection],
) -> (StatusCode, Json<ApiResponse<BootstrapProjectResponse>>) {
    // 1. Determine parent directory: use the common parent of existing projects,
    // or fall back to KRONN_REPOS_DIR env var, or first scan path.
    // We need a writable directory — KRONN_HOST_HOME is mounted read-only.
    let parent_dir = match determine_parent_dir(&state).await {
        Ok(d) => d,
        Err(e) => return (StatusCode::OK, Json(ApiResponse::err(e))),
    };
    if req.name.trim().is_empty() {
        return (
            StatusCode::OK,
            Json(ApiResponse::err("Project name is required")),
        );
    }
    // API clients bypass the form's agent list: refuse an agent that cannot run.
    if let Some(refusal) = bootstrap_agent_refusal(&req.agent, detected) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ApiResponse::err_coded(ApiErrorCode::Validation, refusal)),
        );
    }
    (
        StatusCode::OK,
        bootstrap_in(state, req, project_id, parent_dir).await,
    )
}

async fn bootstrap_in(
    state: AppState,
    req: BootstrapProjectRequest,
    project_id: String,
    parent_dir: String,
) -> Json<ApiResponse<BootstrapProjectResponse>> {
    let project_name = req.name.trim().to_string();
    if project_name.is_empty() {
        return Json(ApiResponse::err("Project name is required"));
    }

    // Sanitize name for directory (kebab-case)
    let dir_name: String = project_name
        .to_lowercase()
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
    let configured_adapter = crate::core::root_agent_files::adapter_for_agent_type(&agent_type);

    let lease =
        match crate::api::audit::docs_write::DocsWriteLease::acquire(&state, &project_id).await {
            Ok(lease) => lease,
            Err(error) => return Json(ApiResponse::err(error)),
        };

    // 2. Create directory + git init on blocking thread
    let setup_result = lease
        .blocking({
            let parent = parent_dir.clone();
            let dirname = dir_name.clone();
            move || -> Result<(), String> {
                // Resolve the parent dir (which exists) then append the new dir name
                let parent_resolved = scanner::resolve_host_path(&parent);
                if !parent_resolved.exists() {
                    return Err(format!(
                        "Parent directory not found: {}",
                        parent_resolved.display()
                    ));
                }
                let project_path = parent_resolved.join(&dirname);
                if project_path.exists() {
                    return Err(format!(
                        "Directory already exists: {}",
                        project_path.display()
                    ));
                }
                std::fs::create_dir_all(&project_path)
                    .map_err(|e| format!("Failed to create directory: {}", e))?;

                // git init
                let status = crate::core::cmd::git_cmd()
                    .arg("init")
                    .current_dir(&project_path)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .map_err(|e| format!("git init failed: {}", e))?;
                if !status.success() {
                    return Err("git init failed".into());
                }

                // Install template — fresh projects get the modern `docs/`
                // convention (post-0.7.1). Existing projects with a legacy
                // `ai/` continue to work via `detect_docs_dir`; this path
                // only fires for never-bootstrapped projects.
                let template_dir = resolve_templates_dir();
                if template_dir.exists() {
                    let docs_template = template_dir.join("docs");
                    // The project's docs root (KT-1192): `docs/` unless configured.
                    let docs_root =
                        crate::core::docs_location::DocsRoot::for_project(&project_path)?;
                    let docs_target = docs_root.dir(&project_path);
                    if docs_template.is_dir() {
                        super::template::copy_dir_nondestructive_in(
                            &project_path,
                            &docs_template,
                            &docs_target,
                            &docs_root,
                        )?;
                    }
                    // 0.8.7 — copy the anti-hallu spec embedded in the binary
                    // into the project's `docs/conventions/` so any agent
                    // running on this project (with or without Kronn) can
                    // open the convention locally. Idempotent : skip if
                    // already present (re-bootstrap doesn't clobber user
                    // edits ; PR3 endpoint `/anti-hallu/inject` is the
                    // explicit re-sync path).
                    let spec_path = docs_target
                        .join("conventions")
                        .join("agents-md-format-v1.md");
                    let _ = crate::core::fs_guard::guarded_write_new(
                        &project_path,
                        &spec_path,
                        crate::core::anti_halluc::SPEC_AGENTS_MD_V1.as_bytes(),
                    );
                    // 0.7.1 — agent-writable subfolders. Bootstrapped only
                    // when missing (idempotent), with a short README so a
                    // human poking around immediately understands what each
                    // folder is for.
                    ensure_agent_writable_subfolders(&project_path, &docs_target);
                    // Human-friendly landing page. AGENTS.md targets LLMs;
                    // index.md is what a human sees when they open `docs/`
                    // on GitHub. Idempotent — only writes when absent.
                    if let Err(e) =
                        crate::core::docs_migration::ensure_docs_index(&project_path, &docs_target)
                    {
                        tracing::warn!("docs index not installed: {e}");
                    }
                    // A brand-new repository starts with no filesystem signal.
                    // Install the shared entry plus the adapter explicitly chosen
                    // for this bootstrap, when that provider has one.
                    let mut detected =
                        crate::core::root_agent_files::detect_agent_adapters(&project_path);
                    if let Some(adapter) = configured_adapter {
                        detected.insert(adapter);
                    }
                    let report = crate::core::root_agent_files::install_detected_agent_files(
                        &project_path,
                        &template_dir,
                        &detected,
                    );
                    for failure in report.failed {
                        tracing::warn!("Agent instruction install failed: {failure}");
                    }
                }

                runner::fix_file_ownership(&project_path);
                Ok(())
            }
        })
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = setup_result {
        return Json(ApiResponse::err(e));
    }

    // 3. Create project in DB
    let now = Utc::now();
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
        tech_debt_count: 0,
        needs_docs_migration: false,
        path_exists: true,
        write_access: None,
        mcp_sync_report: None,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        linked_repos: vec![],
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    enrich_audit_status(&mut project);

    let p = project.clone();
    let mcp_ids = req.mcp_config_ids.clone();
    if let Err(e) = state
        .db
        .with_conn(move |conn| {
            crate::db::projects::insert_project(conn, &p)?;
            // Link selected MCP configs to the new project
            for mcp_id in &mcp_ids {
                crate::db::mcps::link_config_project(conn, mcp_id, &p.id)?;
            }
            Ok(())
        })
        .await
    {
        return Json(ApiResponse::err(format!("DB error: {}", e)));
    }

    // Sync .mcp.json for the new project if MCPs were linked
    if !req.mcp_config_ids.is_empty() {
        let config = state.config.read().await;
        if let Some(ref secret) = config.encryption_secret {
            let secret = secret.clone();
            let pid = project_id.clone();
            if let Err(e) = state
                .db
                .with_conn(move |conn| {
                    crate::core::mcp_scanner::sync_affected_projects(conn, &[pid], &secret);
                    Ok::<_, anyhow::Error>(())
                })
                .await
            {
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
    let bootstrap_prompt = if req.skill_ids.contains(&"bootstrap-architect".to_string()) {
        // Bootstrap++ mode: short prompt, the skill handles the gated flow
        build_bootstrap_plus_prompt(&language, &project_name, &description)
    } else {
        // Classic bootstrap: 6-step prompt with KRONN:BOOTSTRAP_COMPLETE
        build_bootstrap_prompt(&language, &project_name, &description)
    };

    let discussion_id = Uuid::new_v4().to_string();
    let initial_message = DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        session_continuity: None,
        kind: Default::default(),
        model: None,
        lint_report: None,
        id: Uuid::new_v4().to_string(),
        role: MessageRole::User,
        channel: MessageChannel::Main,
        content: bootstrap_prompt,
        agent_type: None,
        timestamp: now,
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
    };

    let discussion = Discussion {
        connection_id: None,
        awaiting_agent: false,
        agent_running: false,
        id: discussion_id.clone(),
        project_id: Some(project_id.clone()),
        title: format!("Bootstrap: {}", project_name),
        agent: agent_type.clone(),
        language: language.clone(),
        participants: vec![agent_type],
        messages: vec![initial_message.clone()],
        message_count: 1,
        non_system_message_count: 1,
        skill_ids: req.skill_ids.clone(),
        profile_ids: vec![
            "architect".into(),
            "product-owner".into(),
            "entrepreneur".into(),
        ],
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        tier: req.tier.unwrap_or(
            if req.skill_ids.contains(&"bootstrap-architect".to_string()) {
                crate::models::ModelTier::Reasoning
            } else {
                crate::models::ModelTier::Default
            },
        ),
        model: None,
        pin_first_message: true,
        worktree_branch: None,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: crate::models::SummaryStrategy::OnDemand,
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };

    let disc = discussion.clone();
    let msg = initial_message;
    if let Err(e) = state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::insert_discussion(conn, &disc)?;
            crate::db::discussions::insert_message(conn, &disc.id, &msg)?;
            Ok(())
        })
        .await
    {
        return Json(ApiResponse::err(format!(
            "Failed to create discussion: {}",
            e
        )));
    }

    Json(ApiResponse::ok(BootstrapProjectResponse {
        project_id,
        discussion_id,
    }))
}

/// Build the bootstrap discussion prompt
/// Bootstrap++ prompt: short user message that defers to the bootstrap-architect skill.
/// The skill (injected as system context) handles the gated validation flow and
/// decides which stage to run first (Stage 0 if a repo MCP is configured,
/// otherwise Stage 1). The user prompt MUST NOT hardcode a starting stage —
/// earlier versions said "Commence par l'Étape 1" which contradicted the skill
/// and caused the agent to skip Stage 0 entirely (seen in disc 8716ae79).
fn build_bootstrap_plus_prompt(language: &str, project_name: &str, description: &str) -> String {
    match language {
        "en" => format!(
            r#"# Bootstrap for project "{project_name}"

Respond in English.

## Project description
{description}

---

Follow the **Bootstrap Architect** skill instructions exactly. The skill defines 4 gated stages (Repo & Project Setup → Architecture → Plan → Issues) and tells you which one to start with based on the configured MCPs. Read the skill first, then start at the right stage. Do NOT skip stages. Emit the stage signal at the end of each message and wait for my validation before continuing."#
        ),
        "es" => format!(
            r#"# Bootstrap del proyecto "{project_name}"

Responde en español.

## Descripción del proyecto
{description}

---

Sigue exactamente las instrucciones del skill **Bootstrap Architect**. El skill define 4 etapas con puertas (Repo y Project → Arquitectura → Plan → Issues) e indica cuál iniciar según los MCPs configurados. Lee el skill primero, luego comienza en la etapa correcta. NO saltes etapas. Emite la señal de la etapa al final de cada mensaje y espera mi validación antes de continuar."#
        ),
        _ => format!(
            r#"# Bootstrap du projet "{project_name}"

Réponds en français.

## Description du projet
{description}

---

Suis les instructions du skill **Bootstrap Architect** à la lettre. Le skill définit 4 étapes avec validation (Repo & Project Setup → Architecture → Plan → Issues) et t'indique par laquelle commencer selon les MCPs configurés. Lis le skill d'abord, puis démarre à la bonne étape. Ne saute AUCUNE étape. Émets le signal de l'étape à la fin de chaque message et attends ma validation avant de continuer."#
        ),
    }
}

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

#[cfg(all(test, feature = "full-tests"))]
mod prompt_tests {
    use super::*;
    use crate::core::scanner;

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
            assert!(
                prompt.contains("KRONN:BOOTSTRAP_COMPLETE"),
                "Bootstrap prompt ({}) must contain completion signal",
                lang
            );
        }
    }

    #[test]
    fn bootstrap_plus_prompt_defers_to_skill() {
        for lang in ["fr", "en", "es"] {
            let prompt = build_bootstrap_plus_prompt(lang, "MyApp", "A cool app");
            // Must contain the project info
            assert!(
                prompt.contains("MyApp"),
                "Plus prompt ({}) must contain project name",
                lang
            );
            assert!(
                prompt.contains("A cool app"),
                "Plus prompt ({}) must contain description",
                lang
            );
            // Must reference the skill by name — that's how the LLM knows to
            // look at the injected system prompt for the gated workflow.
            assert!(
                prompt.contains("Bootstrap Architect"),
                "Plus prompt ({}) must reference the skill",
                lang
            );
            // v4: must NOT hardcode a starting stage. Earlier versions said
            // "Commence par l'Étape 1" / "Start with Stage 1" which made the
            // agent skip Stage 0 entirely (disc 8716ae79). The skill decides
            // which stage to start based on the configured MCPs.
            assert!(!prompt.contains("Commence par l'**Étape 1**"),
                "Plus prompt ({}) must NOT hardcode 'Commence par l'Étape 1' — let the skill decide", lang);
            assert!(
                !prompt.contains("Start with **Stage 1**"),
                "Plus prompt ({}) must NOT hardcode 'Start with Stage 1'",
                lang
            );
            assert!(
                !prompt.contains("Comienza con la **Etapa 1**"),
                "Plus prompt ({}) must NOT hardcode 'Comienza con la Etapa 1'",
                lang
            );
            // Must NOT mention legacy signals that the skill no longer uses
            assert!(
                !prompt.contains("KRONN:BOOTSTRAP_COMPLETE"),
                "Plus prompt ({}) must NOT mention BOOTSTRAP_COMPLETE (handled by skill)",
                lang
            );
            // Must tell the agent to not skip stages — otherwise Stage 0
            // can be silently bypassed when the LLM infers the wrong start.
            assert!(
                prompt.to_lowercase().contains("saute")
                    || prompt.to_lowercase().contains("skip")
                    || prompt.to_lowercase().contains("salt"),
                "Plus prompt ({}) must instruct the agent to not skip stages",
                lang
            );
        }
    }

    // ─── Path traversal validation ─────────────────────────────────────────

    #[test]
    fn create_rejects_path_with_dotdot_components() {
        // POST /api/projects must refuse paths containing `..` to prevent
        // a future multi-user / peer caller from registering a project that
        // anchors reads outside the intended scan roots.
        let bad = scanner::contains_parent_dir("/home/user/../etc/passwd");
        assert!(
            bad,
            "scanner::contains_parent_dir must flag /home/user/../etc/passwd"
        );
        let good = scanner::contains_parent_dir("/home/user/repos/my-project");
        assert!(!good, "Clean absolute paths must not be flagged");
    }

    #[test]
    fn create_rejects_relative_traversal() {
        // Relative paths with `..` are equally dangerous and must be flagged.
        assert!(scanner::contains_parent_dir("../../etc/shadow"));
        assert!(scanner::contains_parent_dir("a/b/../c"));
    }

    #[test]
    fn create_allows_dotdot_inside_filename() {
        // A double-dot sequence within a filename component (e.g. "file..bak")
        // is not a parent-dir component — must NOT be rejected.
        assert!(!scanner::contains_parent_dir("/home/user/file..bak"));
        assert!(!scanner::contains_parent_dir(
            "/home/user/.config/app..conf"
        ));
    }
}

/// An installed, enabled, signed-in detection of `agent`, for tests.
#[cfg(test)]
pub(crate) fn usable_for_tests(agent: AgentType) -> AgentDetection {
    AgentDetection {
        name: format!("{agent:?}"),
        agent_type: agent,
        installed: true,
        enabled: true,
        path: None,
        fallback_command: None,
        version: None,
        latest_version: None,
        version_checked_at: None,
        version_check_error: None,
        version_source_url: None,
        origin: "test".into(),
        install_command: None,
        host_managed: false,
        host_label: None,
        runtime_available: true,
        auth_ready: Some(true),
        auth_setup_command: None,
        rtk_available: false,
        rtk_hook_configured: false,
        runtime_warning: None,
        shadowed_installs: None,
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod discussion_target_tests {
    use super::*;

    fn state() -> AppState {
        AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap()),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        )
    }

    /// Bootstraps "News Site" next to a seeded project and returns its discussion.
    async fn bootstrap_discussion(tier: Option<ModelTier>, skill_ids: Vec<String>) -> Discussion {
        let state = state();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_string_lossy().into_owned();
        state
            .db
            .with_conn(move |conn| {
                let now = Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO projects (id, name, path, created_at, updated_at)
                     VALUES ('p1', 'P', ?1, ?2, ?2)",
                    rusqlite::params![path, now],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let request = BootstrapProjectRequest {
            name: "News Site".into(),
            description: "A news site".into(),
            agent: AgentType::Codex,
            mcp_config_ids: vec![],
            skill_ids,
            tier,
        };
        let response = bootstrap_as(
            state.clone(),
            request,
            "boot-1".into(),
            &[usable_for_tests(AgentType::Codex)],
        )
        .await
        .1
         .0;
        assert!(response.success, "{response:?}");
        let discussion_id = response.data.unwrap().discussion_id;
        state
            .db
            .with_read_conn(move |conn| {
                crate::db::discussions::get_discussion(conn, &discussion_id)
            })
            .await
            .unwrap()
            .expect("bootstrap discussion stored")
    }

    #[tokio::test]
    async fn bootstrap_discussion_keeps_the_chosen_agent_and_tier() {
        let disc =
            bootstrap_discussion(Some(ModelTier::Economy), vec!["bootstrap-architect".into()])
                .await;
        assert_eq!(disc.agent, AgentType::Codex);
        assert_eq!(disc.participants, vec![AgentType::Codex]);
        assert_eq!(disc.tier, ModelTier::Economy);
        assert_eq!(
            disc.model, None,
            "the model resolves from the tier at run time"
        );
    }

    #[tokio::test]
    async fn bootstrap_discussion_without_tier_keeps_the_skill_default() {
        let plus = bootstrap_discussion(None, vec!["bootstrap-architect".into()]).await;
        assert_eq!(plus.tier, ModelTier::Reasoning);
        let classic = bootstrap_discussion(None, vec![]).await;
        assert_eq!(classic.tier, ModelTier::Default);
    }

    #[test]
    fn bootstrap_refuses_an_agent_the_form_would_not_offer() {
        let usable = usable_for_tests(AgentType::Codex);
        assert_eq!(
            bootstrap_agent_refusal(&AgentType::Codex, std::slice::from_ref(&usable)),
            None
        );
        let vibe = usable_for_tests(AgentType::Vibe);
        assert!(bootstrap_agent_refusal(&AgentType::Vibe, &[vibe]).is_some());
        assert!(
            bootstrap_agent_refusal(&AgentType::ClaudeCode, std::slice::from_ref(&usable))
                .is_some()
        );
        let refused = |change: fn(&mut AgentDetection)| {
            let mut detection = usable.clone();
            change(&mut detection);
            bootstrap_agent_refusal(&AgentType::Codex, &[detection]).expect("refused")
        };
        assert!(refused(|d| d.enabled = false).contains("disabled"));
        assert!(refused(|d| {
            d.installed = false;
            d.runtime_available = false;
        })
        .contains("not installed"));
        assert!(refused(|d| d.auth_ready = Some(false)).contains("not signed in"));
    }

    #[tokio::test]
    async fn bootstrap_with_an_unusable_agent_is_a_422_that_creates_nothing() {
        let state = state();
        let root = tempfile::tempdir().unwrap();
        state.config.write().await.scan.paths = vec![root.path().to_string_lossy().into_owned()];
        let request = BootstrapProjectRequest {
            name: "News Site".into(),
            description: "A news site".into(),
            agent: AgentType::Vibe,
            mcp_config_ids: vec![],
            skill_ids: vec![],
            tier: None,
        };
        let (status, response) = bootstrap_as(
            state.clone(),
            request,
            "boot-1".into(),
            &[usable_for_tests(AgentType::Vibe)],
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!response.0.success);
        assert_eq!(response.0.error_code.as_deref(), Some("validation"));
        assert!(!root.path().join("news-site").exists());
        let projects = state
            .db
            .with_read_conn(|conn| Ok(crate::db::projects::list_projects(conn)?.len()))
            .await
            .unwrap();
        assert_eq!(projects, 0);
    }
}
