use crate::agents;
use crate::core::{config, scanner};
use crate::models::*;
use crate::AppState;
use axum::{
    body::Body,
    extract::{Multipart, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;

/// Resolve default scan paths (best candidate for the wizard).
/// In Docker: KRONN_HOST_HOME.
/// On Windows native: first WSL user home (most repos live there) or Windows home.
/// On Linux/macOS: user home.
fn default_scan_path() -> Option<String> {
    // Docker: mounted host home
    if let Ok(host_home) = std::env::var("KRONN_HOST_HOME") {
        return Some(host_home);
    }

    // On Windows: prefer WSL home over Windows home (most dev repos are in WSL)
    #[cfg(target_os = "windows")]
    {
        for distro_path in &["\\\\wsl.localhost", "\\\\wsl$"] {
            let wsl_root = std::path::Path::new(distro_path);
            if let Ok(entries) = std::fs::read_dir(wsl_root) {
                for entry in entries.flatten() {
                    let home = entry.path().join("home");
                    if let Ok(users) = std::fs::read_dir(&home) {
                        for user in users.flatten() {
                            if user.path().is_dir() {
                                return Some(user.path().to_string_lossy().to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallback: native user home
    directories::UserDirs::new().map(|d| d.home_dir().to_string_lossy().to_string())
}

/// GET /api/setup/status
/// Returns current setup state with auto-detected repos.
/// Fast path: if config exists and scan_paths are set, skip the expensive
/// agent detection + filesystem scan — setup is already complete.
pub async fn get_status(State(state): State<AppState>) -> Json<ApiResponse<SetupStatus>> {
    let is_first = config::is_first_run().await.unwrap_or(true);

    let config = state.config.read().await;
    let scan_paths_set = !config.scan.paths.is_empty();

    // Fast path: setup already completed — skip expensive scan & agent detection.
    // This makes the wizard→dashboard transition instant on Windows/WSL.
    if !is_first && scan_paths_set {
        // Light agent detection: reuse cached results or run in background.
        // For the status check we only need to know if at least one is installed,
        // so we still detect but the scan is skipped entirely.
        drop(config);
        // Cached: the boot hits this on every dashboard load and the frontend
        // blocks on it — an uncached sweep spawns `<binary> --version` per
        // agent and froze the app under concurrent-agent load.
        let agents_detected = agents::detect_all_cached(false).await;
        return Json(ApiResponse::ok(SetupStatus {
            is_first_run: false,
            current_step: SetupStep::Complete,
            agents_detected,
            scan_paths_set: true,
            repos_detected: vec![], // Skip scan — projects page will load them
            default_scan_path: default_scan_path(),
        }));
    }

    // Full path: first run or scan paths not yet configured.
    // Run agent detection and repo scan IN PARALLEL to halve the wait time.
    let mut scan_paths: Vec<String> = if scan_paths_set {
        config.scan.paths.clone()
    } else {
        default_scan_path().into_iter().collect()
    };
    // On Windows: always include WSL home directories
    for wsl_home in crate::api::projects::discover_wsl_homes() {
        if !scan_paths.contains(&wsl_home) {
            scan_paths.push(wsl_home);
        }
    }
    let scan_ignore = config.scan.ignore.clone();
    let scan_depth = config.scan.scan_depth;
    drop(config);

    let has_wsl_paths = scan_paths
        .iter()
        .any(|p| p.starts_with(r"\\wsl.localhost\") || p.starts_with(r"\\wsl$\"));
    let scan_timeout = if has_wsl_paths { 15 } else { 5 };

    // Parallel: detect agents + scan repos simultaneously
    let (agents_detected, repos_result) = tokio::join!(
        agents::detect_all_cached(false),
        tokio::time::timeout(
            std::time::Duration::from_secs(scan_timeout),
            scanner::scan_paths_with_depth(&scan_paths, &scan_ignore, scan_depth),
        )
    );

    let repos_detected = repos_result
        .unwrap_or_else(|_| {
            tracing::warn!(
                "Repo scan timed out after {}s — returning empty list",
                scan_timeout
            );
            Ok(vec![])
        })
        .unwrap_or_default();

    let current_step = if agents_detected.iter().all(|a| !a.installed) {
        SetupStep::Agents
    } else if repos_detected.is_empty() && !scan_paths_set {
        SetupStep::ScanPaths
    } else {
        SetupStep::Complete
    };

    Json(ApiResponse::ok(SetupStatus {
        is_first_run: is_first,
        current_step,
        agents_detected,
        scan_paths_set,
        repos_detected,
        default_scan_path: default_scan_path(),
    }))
}

/// GET /api/config/scan-paths
/// Returns the configured scan paths
pub async fn get_scan_paths(State(state): State<AppState>) -> Json<ApiResponse<Vec<String>>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(config.scan.paths.clone()))
}

/// GET /api/config/scan-ignore
/// Returns the configured scan ignore patterns
pub async fn get_scan_ignore(State(state): State<AppState>) -> Json<ApiResponse<Vec<String>>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(config.scan.ignore.clone()))
}

/// POST /api/config/scan-ignore
/// Set scan ignore patterns
pub async fn set_scan_ignore(
    State(state): State<AppState>,
    Json(patterns): Json<Vec<String>>,
) -> Json<ApiResponse<()>> {
    let mut config = state.config.write().await;
    config.scan.ignore = patterns;
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// POST /api/setup/scan-paths
/// Set repository scan paths
pub async fn set_scan_paths(
    State(state): State<AppState>,
    Json(req): Json<SetScanPathsRequest>,
) -> Json<ApiResponse<()>> {
    let mut config = state.config.write().await;
    config.scan.paths = req.paths;

    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// POST /api/setup/install-agent
/// Install an agent
pub async fn install_agent(Json(agent_type): Json<AgentType>) -> Json<ApiResponse<String>> {
    match agents::install_agent(&agent_type).await {
        Ok(output) => Json(ApiResponse::ok(output)),
        Err(e) => Json(ApiResponse::err(format!("Install failed: {}", e))),
    }
}

/// POST /api/setup/complete
/// Mark setup as complete
pub async fn complete(State(state): State<AppState>) -> Json<ApiResponse<()>> {
    let config = state.config.read().await;
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to finalize: {}", e))),
    }
}

/// GET /api/config/tokens
/// Returns all API keys (masked) grouped by provider
pub async fn get_tokens(State(state): State<AppState>) -> Json<ApiResponse<ApiKeysResponse>> {
    let config = state.config.read().await;
    let keys = config
        .tokens
        .keys
        .iter()
        .map(|k| ApiKeyDisplay {
            id: k.id.clone(),
            name: k.name.clone(),
            provider: k.provider.clone(),
            masked_value: mask_token(&k.value),
            active: k.active,
        })
        .collect();
    Json(ApiResponse::ok(ApiKeysResponse {
        keys,
        disabled_overrides: config.tokens.disabled_overrides.clone(),
    }))
}

/// POST /api/config/api-keys
/// Create or update an API key
pub async fn save_api_key(
    State(state): State<AppState>,
    Json(req): Json<SaveApiKeyRequest>,
) -> Json<ApiResponse<ApiKeyDisplay>> {
    let mut config = state.config.write().await;

    if req.value.is_empty() || req.value.contains('*') {
        return Json(ApiResponse::err("Invalid key value"));
    }

    let key = if let Some(ref id) = req.id {
        // Update existing
        if let Some(k) = config.tokens.keys.iter_mut().find(|k| &k.id == id) {
            k.name = req.name.clone();
            k.value = req.value.clone();
            k.clone()
        } else {
            return Json(ApiResponse::err("Key not found"));
        }
    } else {
        // Create new
        let is_first = !config
            .tokens
            .keys
            .iter()
            .any(|k| k.provider == req.provider);
        let new_key = ApiKey {
            id: uuid::Uuid::new_v4().to_string(),
            name: req.name,
            provider: req.provider,
            value: req.value,
            active: is_first, // First key for this provider is auto-activated
        };
        config.tokens.keys.push(new_key.clone());
        new_key
    };

    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(ApiKeyDisplay {
            id: key.id,
            name: key.name,
            provider: key.provider,
            masked_value: mask_token(&key.value),
            active: key.active,
        })),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// DELETE /api/config/api-keys/:id
/// Delete an API key
pub async fn delete_api_key(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    let mut config = state.config.write().await;

    let idx = config.tokens.keys.iter().position(|k| k.id == id);
    if let Some(i) = idx {
        let removed = config.tokens.keys.remove(i);
        // If the deleted key was active, activate the next key for this provider
        if removed.active {
            if let Some(next) = config
                .tokens
                .keys
                .iter_mut()
                .find(|k| k.provider == removed.provider)
            {
                next.active = true;
            }
        }
        match config::save(&config).await {
            Ok(_) => Json(ApiResponse::ok(())),
            Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
        }
    } else {
        Json(ApiResponse::err("Key not found"))
    }
}

/// POST /api/config/api-keys/:id/activate
/// Set this key as the active one for its provider
pub async fn activate_api_key(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    let mut config = state.config.write().await;

    let provider = config
        .tokens
        .keys
        .iter()
        .find(|k| k.id == id)
        .map(|k| k.provider.clone());

    if let Some(provider) = provider {
        for k in config.tokens.keys.iter_mut() {
            if k.provider == provider {
                k.active = k.id == id;
            }
        }
        match config::save(&config).await {
            Ok(_) => Json(ApiResponse::ok(())),
            Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
        }
    } else {
        Json(ApiResponse::err("Key not found"))
    }
}

/// POST /api/config/toggle-token-override
/// Toggle whether a provider's API key override is active.
/// When disabling, also removes the key from agent auth files.
/// When re-enabling, writes the key back.
pub async fn toggle_token_override(
    State(state): State<AppState>,
    Json(provider): Json<String>,
) -> Json<ApiResponse<bool>> {
    let mut config = state.config.write().await;
    let disabled = &mut config.tokens.disabled_overrides;
    let is_now_enabled = if disabled.contains(&provider) {
        disabled.retain(|d| d != &provider);
        true
    } else {
        disabled.push(provider.clone());
        false
    };

    // Sync agent auth files: write key if enabled, remove if disabled
    if provider == "openai" {
        sync_codex_auth(if is_now_enabled {
            config.tokens.active_key_for("openai")
        } else {
            None
        });
    }
    if provider == "google" {
        crate::core::key_discovery::write_gemini_key(if is_now_enabled {
            config.tokens.active_key_for("google")
        } else {
            None
        });
    }

    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(is_now_enabled)),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// GET /api/config/language
pub async fn get_language(State(state): State<AppState>) -> Json<ApiResponse<String>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(config.language.clone()))
}

/// POST /api/config/language
pub async fn save_language(
    State(state): State<AppState>,
    Json(lang): Json<String>,
) -> Json<ApiResponse<()>> {
    let mut config = state.config.write().await;
    config.language = lang;
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// GET /api/config/ui-language
///
/// UI locale of the React frontend (FR/EN/ES). Returned by the frontend at
/// mount to survive Tauri WebView2 localStorage wipes (app update, profile
/// rotation on Windows) — localStorage remains the fast-path write so the
/// UI doesn't flash on navigation.
pub async fn get_ui_language(State(state): State<AppState>) -> Json<ApiResponse<String>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(config.ui_language.clone()))
}

/// POST /api/config/ui-language
pub async fn save_ui_language(
    State(state): State<AppState>,
    Json(lang): Json<String>,
) -> Json<ApiResponse<()>> {
    // Validate — refuse random strings to avoid poisoning the config. We
    // accept only the three locales the frontend actually ships.
    if !matches!(lang.as_str(), "fr" | "en" | "es") {
        return Json(ApiResponse::err(format!(
            "Invalid ui_language '{}'. Expected fr|en|es.",
            lang
        )));
    }
    let mut config = state.config.write().await;
    config.ui_language = lang;
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// GET /api/config/stt-model
pub async fn get_stt_model(State(state): State<AppState>) -> Json<ApiResponse<Option<String>>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(config.stt_model.clone()))
}

/// POST /api/config/stt-model
pub async fn save_stt_model(
    State(state): State<AppState>,
    Json(model_id): Json<String>,
) -> Json<ApiResponse<()>> {
    let mut config = state.config.write().await;
    config.stt_model = if model_id.is_empty() {
        None
    } else {
        Some(model_id)
    };
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// GET /api/config/tts-voices — returns the whole map so the frontend can
/// render the correct voice on whatever language it currently displays.
pub async fn get_tts_voices(
    State(state): State<AppState>,
) -> Json<ApiResponse<std::collections::HashMap<String, String>>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(config.tts_voices.clone()))
}

#[derive(Debug, serde::Deserialize)]
pub struct TtsVoiceRequest {
    pub lang: String,
    pub voice_id: String,
}

/// GET /api/config/global-context
///
/// Global knowledge base injected into all discussions. Markdown content
/// that provides glossary, company conventions, tech stack overview, etc.
/// Supplements project-level `ai/` context — this one applies even when
/// the discussion has no project attached.
pub async fn get_global_context(State(state): State<AppState>) -> Json<ApiResponse<String>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(
        config.server.global_context.clone().unwrap_or_default(),
    ))
}

/// GET /api/config/global-context-mode
pub async fn get_global_context_mode(State(state): State<AppState>) -> Json<ApiResponse<String>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(config.server.global_context_mode.clone()))
}

/// POST /api/config/global-context-mode
pub async fn save_global_context_mode(
    State(state): State<AppState>,
    Json(mode): Json<String>,
) -> Json<ApiResponse<()>> {
    if !matches!(mode.as_str(), "always" | "no_project" | "never") {
        return Json(ApiResponse::err(format!(
            "Invalid mode '{}'. Expected always|no_project|never.",
            mode
        )));
    }
    let mut config = state.config.write().await;
    config.server.global_context_mode = mode;
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// GET /api/config/anti-hallucination-mode
pub async fn get_anti_hallucination_mode(
    State(state): State<AppState>,
) -> Json<ApiResponse<String>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(
        config.server.anti_hallucination_mode.clone(),
    ))
}

/// POST /api/config/anti-hallucination-mode
pub async fn save_anti_hallucination_mode(
    State(state): State<AppState>,
    Json(mode): Json<String>,
) -> Json<ApiResponse<()>> {
    if !crate::core::anti_halluc::is_valid_mode(&mode) {
        return Json(ApiResponse::err(format!(
            "Invalid mode '{}'. Expected off|warn|enforce.",
            mode
        )));
    }
    let mut config = state.config.write().await;
    config.server.anti_hallucination_mode = mode.clone();
    match config::save(&config).await {
        Ok(_) => {
            // Keep the process-global flag in sync so the change takes effect
            // immediately on the next agent spawn (no restart needed).
            crate::core::anti_halluc::set_mode(&mode);
            Json(ApiResponse::ok(()))
        }
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// GET /api/config/continual-learning-enabled — 0.9.0 master toggle (default OFF/beta).
pub async fn get_continual_learning_enabled(
    State(state): State<AppState>,
) -> Json<ApiResponse<bool>> {
    Json(ApiResponse::ok(
        state.config.read().await.server.continual_learning_enabled,
    ))
}

/// POST /api/config/continual-learning-enabled — flip the master toggle.
/// Per-project doc-wiring (`learnings` section) is synced separately via
/// `POST /api/projects/{id}/learnings/sync` (on audit / per project).
pub async fn save_continual_learning_enabled(
    State(state): State<AppState>,
    Json(enabled): Json<bool>,
) -> Json<ApiResponse<()>> {
    {
        let mut config = state.config.write().await;
        config.server.continual_learning_enabled = enabled;
        if let Err(e) = config::save(&config).await {
            return Json(ApiResponse::err(format!("Failed to save: {}", e)));
        }
    }
    // Doc-wiring: flipping the toggle syncs every project's `docs/AGENTS.md`
    // `learnings` pointer section (inject when ON / remove when OFF). Without
    // this, ON would write project learnings that agents never load (the pointer
    // wouldn't exist). Best-effort per project — a failure on one doesn't block.
    let projects = state
        .db
        .with_conn(crate::db::projects::list_projects)
        .await
        .unwrap_or_default();
    let mut synced = 0usize;
    for p in &projects {
        match crate::core::learning_doc::sync(std::path::Path::new(&p.path), enabled) {
            Ok(crate::core::learning_doc::LearningDocOutcome::Inserted)
            | Ok(crate::core::learning_doc::LearningDocOutcome::Removed) => synced += 1,
            Ok(_) => {}
            Err(e) => tracing::warn!(
                target: "continual_learning",
                "doc-wiring sync failed for project {}: {e}", p.id
            ),
        }
    }
    tracing::info!(target: "continual_learning", "toggle={enabled} → doc-wiring synced {synced} project(s)");
    Json(ApiResponse::ok(()))
}

/// GET /api/conventions/agents-md-format-v1
///
/// Returns the Phase 2 spec embedded in the binary (`include_str!`) as
/// `text/markdown`. Linked from Settings → Sourcing & Anti-hallucination so
/// users see the convention this installation actually implements (the GitHub
/// `main` copy may have moved on).
pub async fn get_agents_md_spec_v1() -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
        crate::core::anti_halluc::SPEC_AGENTS_MD_V1,
    )
        .into_response()
}

/// POST /api/config/global-context
pub async fn save_global_context(
    State(state): State<AppState>,
    Json(content): Json<String>,
) -> Json<ApiResponse<()>> {
    let mut config = state.config.write().await;
    config.server.global_context = if content.trim().is_empty() {
        None
    } else {
        Some(content)
    };
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// POST /api/config/tts-voice — update a single `(lang, voice_id)` mapping.
pub async fn save_tts_voice(
    State(state): State<AppState>,
    Json(req): Json<TtsVoiceRequest>,
) -> Json<ApiResponse<()>> {
    if req.lang.is_empty() {
        return Json(ApiResponse::err("lang is required"));
    }
    let mut config = state.config.write().await;
    if req.voice_id.is_empty() {
        config.tts_voices.remove(&req.lang);
    } else {
        config.tts_voices.insert(req.lang, req.voice_id);
    }
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// GET /api/config/scan-depth
pub async fn get_scan_depth(State(state): State<AppState>) -> Json<ApiResponse<usize>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(config.scan.scan_depth))
}

/// POST /api/config/scan-depth
pub async fn set_scan_depth(
    State(state): State<AppState>,
    Json(depth): Json<usize>,
) -> Json<ApiResponse<usize>> {
    let clamped = depth.clamp(2, 10);
    let mut config = state.config.write().await;
    config.scan.scan_depth = clamped;
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(clamped)),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// GET /api/config/agent-access
pub async fn get_agent_access(State(state): State<AppState>) -> Json<ApiResponse<AgentsConfig>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(config.agents.clone()))
}

/// POST /api/config/agent-access
/// Toggle full_access for an agent
pub async fn set_agent_access(
    State(state): State<AppState>,
    Json(req): Json<SetAgentAccessRequest>,
) -> Json<ApiResponse<()>> {
    let mut config = state.config.write().await;
    match req.agent {
        AgentType::ClaudeCode => config.agents.claude_code.full_access = req.full_access,
        AgentType::Codex => config.agents.codex.full_access = req.full_access,
        AgentType::GeminiCli => config.agents.gemini_cli.full_access = req.full_access,
        AgentType::Kiro => config.agents.kiro.full_access = req.full_access,
        AgentType::Vibe => config.agents.vibe.full_access = req.full_access,
        _ => return Json(ApiResponse::err("Agent does not support access flags")),
    }
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// GET /api/config/model-tiers
pub async fn get_model_tiers(State(state): State<AppState>) -> Json<ApiResponse<ModelTiersConfig>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(config.agents.model_tiers.clone()))
}

/// POST /api/config/model-tiers
pub async fn set_model_tiers(
    State(state): State<AppState>,
    Json(req): Json<ModelTiersConfig>,
) -> Json<ApiResponse<()>> {
    let mut config = state.config.write().await;
    config.agents.model_tiers = req;
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// GET /api/config/server
pub async fn get_server_config(
    State(state): State<AppState>,
) -> Json<ApiResponse<ServerConfigPublic>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(ServerConfigPublic {
        host: config.server.host.clone(),
        port: config.server.port,
        domain: config.server.domain.clone(),
        max_concurrent_agents: config.server.max_concurrent_agents,
        agent_stall_timeout_min: config.server.agent_stall_timeout_min,
        auth_enabled: config.server.auth_enabled && config.server.auth_token.is_some(),
        pseudo: config.server.pseudo.clone(),
        avatar_email: config.server.avatar_email.clone(),
        bio: config.server.bio.clone(),
        debug_mode: config.server.debug_mode,
        default_model_tier: config.server.default_model_tier,
        default_summary_strategy: config.server.default_summary_strategy,
    }))
}

/// POST /api/config/server
pub async fn set_server_config(
    State(state): State<AppState>,
    Json(req): Json<UpdateServerConfigRequest>,
) -> Json<ApiResponse<()>> {
    let mut config = state.config.write().await;
    if let Some(domain) = req.domain {
        config.server.domain = if domain.is_empty() {
            None
        } else {
            Some(domain)
        };
    }
    if let Some(max) = req.max_concurrent_agents {
        config.server.max_concurrent_agents = max.clamp(1, 20);
    }
    if let Some(timeout) = req.agent_stall_timeout_min {
        config.server.agent_stall_timeout_min = clamp_stall_timeout_min(timeout);
    }
    if let Some(pseudo) = req.pseudo {
        config.server.pseudo = if pseudo.is_empty() {
            None
        } else {
            Some(pseudo)
        };
    }
    if let Some(email) = req.avatar_email {
        config.server.avatar_email = if email.is_empty() { None } else { Some(email) };
    }
    if let Some(bio) = req.bio {
        config.server.bio = if bio.is_empty() { None } else { Some(bio) };
    }
    if let Some(debug_mode) = req.debug_mode {
        if debug_mode != config.server.debug_mode {
            config.server.debug_mode = debug_mode;
            // The tracing EnvFilter was fixed at startup; toggling this
            // flag only takes effect on the next process restart. Log
            // prominently so the user knows what to expect.
            tracing::warn!(
                "debug_mode changed to {} — restart the backend for the new log level to take effect",
                debug_mode
            );
        }
    }
    // 0.8.6 phase 4 — default model tier. PATCH-semantic : only update
    // when explicitly passed (None keeps existing). No clamp / validation
    // beyond the enum's deserialise step ; ModelTier has only 3 variants
    // so an unknown value would already 422 at deserialize.
    if let Some(tier) = req.default_model_tier {
        config.server.default_model_tier = tier;
    }
    // 0.8.6 phase 4 — default summary strategy. Same PATCH semantic.
    if let Some(strategy) = req.default_summary_strategy {
        config.server.default_summary_strategy = strategy;
    }
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

// ── Network exposure ("Allow connections from other devices") ───────────────

/// State of the LAN/Tailscale exposure toggle.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct NetworkExposure {
    /// Configured to bind a network-reachable address (`0.0.0.0`/`::`), not just localhost.
    pub exposed: bool,
    /// The configured exposure differs from what the process bound at boot →
    /// a restart is needed for it to take effect.
    pub restart_required: bool,
    pub port: u16,
    /// Reachable addresses (LAN + Tailscale) a peer could use to reach us.
    pub reachable_ips: Vec<crate::core::tailscale::DetectedIp>,
}

#[derive(Debug, serde::Deserialize)]
pub struct SetNetworkExposureRequest {
    pub exposed: bool,
}

/// GET /api/config/network-exposure
pub async fn get_network_exposure(
    State(state): State<AppState>,
) -> Json<ApiResponse<NetworkExposure>> {
    let (host, port) = {
        let config = state.config.read().await;
        (config.server.host.clone(), config.server.port)
    };
    let reachable_ips = crate::core::tailscale::detect_all_ips().await;
    Json(ApiResponse::ok(NetworkExposure {
        exposed: crate::core::net_expose::is_exposed_host(&host),
        restart_required: crate::core::net_expose::restart_required(&host),
        port,
        reachable_ips,
    }))
}

/// POST /api/config/network-exposure — flip LAN/Tailscale exposure.
///
/// Exposing binds `0.0.0.0` (vs `127.0.0.1`) and, secure-by-default, forces auth
/// on + ensures a token exists: a LAN/Tailscale peer isn't localhost, so the
/// loopback auth-bypass won't apply to it — but the owner's local UI keeps
/// working through that same bypass, so this never locks them out. Takes effect
/// on the next restart (the host is only bound at boot).
pub async fn set_network_exposure(
    State(state): State<AppState>,
    Json(req): Json<SetNetworkExposureRequest>,
) -> Json<ApiResponse<NetworkExposure>> {
    {
        let mut config = state.config.write().await;
        config.server.host = if req.exposed {
            "0.0.0.0".to_string()
        } else {
            "127.0.0.1".to_string()
        };
        if req.exposed {
            config.server.auth_enabled = true;
            if config.server.auth_token.as_deref().unwrap_or("").is_empty() {
                config.server.auth_token = Some(uuid::Uuid::new_v4().to_string());
            }
        }
        if let Err(e) = config::save(&config).await {
            return Json(ApiResponse::err(format!("Failed to save: {}", e)));
        }
    }
    get_network_exposure(State(state)).await
}

/// POST /api/config/auth-token/regenerate
pub async fn regenerate_auth_token(State(state): State<AppState>) -> Json<ApiResponse<String>> {
    let mut config = state.config.write().await;
    let new_token = uuid::Uuid::new_v4().to_string();
    config.server.auth_token = Some(new_token.clone());
    config.server.auth_enabled = true;
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(new_token)),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// GET /api/config/auth-token
pub async fn get_auth_token(State(state): State<AppState>) -> Json<ApiResponse<Option<String>>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(config.server.auth_token.clone()))
}

/// Write or remove the OpenAI key from ~/.codex/auth.json
fn sync_codex_auth(key: Option<&str>) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let codex_dir = std::path::PathBuf::from(home).join(".codex");
    let codex_auth_path = codex_dir.join("auth.json");

    // MERGE into the existing auth.json — never wholesale-replace it. A user
    // logged into Codex via ChatGPT-subscription OAuth has refresh/access
    // tokens in this file that Kronn didn't write; replacing (or deleting)
    // the file destroyed that login. We only own two fields.
    let existing: serde_json::Map<String, serde_json::Value> =
        match std::fs::read_to_string(&codex_auth_path) {
            Ok(s) => match serde_json::from_str::<serde_json::Value>(&s) {
                Ok(serde_json::Value::Object(m)) => m,
                Ok(_) | Err(_) if key.is_some() => {
                    // Corrupt/non-object file and we're about to write: don't
                    // merge garbage, but say what we're replacing.
                    tracing::warn!(
                        "{} was not valid JSON — rewriting it",
                        codex_auth_path.display()
                    );
                    serde_json::Map::new()
                }
                _ => {
                    tracing::warn!(
                        "{} unreadable as JSON — leaving it untouched",
                        codex_auth_path.display()
                    );
                    return;
                }
            },
            Err(_) => serde_json::Map::new(), // missing file: start fresh
        };

    match key {
        Some(k) => {
            let _ = std::fs::create_dir_all(&codex_dir);
            let mut merged = existing;
            merged.insert(
                "auth_mode".into(),
                serde_json::Value::String("apikey".into()),
            );
            merged.insert("OPENAI_API_KEY".into(), serde_json::Value::String(k.into()));
            let content = serde_json::Value::Object(merged);
            // Safety: serializing a serde_json::Value cannot fail
            match std::fs::write(
                &codex_auth_path,
                serde_json::to_string_pretty(&content)
                    .expect("JSON Value serialization cannot fail"),
            ) {
                Ok(_) => tracing::info!(
                    "Synced OpenAI key into {} (other fields preserved)",
                    codex_auth_path.display()
                ),
                Err(e) => tracing::warn!("Failed to write {}: {}", codex_auth_path.display(), e),
            }
        }
        None => {
            // Remove ONLY our fields; other credentials (OAuth tokens) stay.
            // Delete the file only when nothing else remains in it.
            let mut merged = existing;
            if merged.remove("OPENAI_API_KEY").is_none()
                && merged.get("auth_mode").and_then(|v| v.as_str()) != Some("apikey")
            {
                return; // nothing of ours in there
            }
            if merged.get("auth_mode").and_then(|v| v.as_str()) == Some("apikey") {
                merged.remove("auth_mode");
            }
            if merged.is_empty() {
                match std::fs::remove_file(&codex_auth_path) {
                    Ok(_) => tracing::info!(
                        "Removed {} (Codex will use local auth)",
                        codex_auth_path.display()
                    ),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        tracing::warn!("Failed to remove {}: {}", codex_auth_path.display(), e)
                    }
                }
            } else {
                let content = serde_json::Value::Object(merged);
                match std::fs::write(
                    &codex_auth_path,
                    serde_json::to_string_pretty(&content)
                        .expect("JSON Value serialization cannot fail"),
                ) {
                    Ok(_) => tracing::info!(
                        "Removed Kronn's API-key fields from {} (other credentials preserved)",
                        codex_auth_path.display()
                    ),
                    Err(e) => {
                        tracing::warn!("Failed to write {}: {}", codex_auth_path.display(), e)
                    }
                }
            }
        }
    }
}

/// POST /api/config/sync-agent-tokens
/// Write API tokens into agent-specific auth files (e.g. ~/.codex/auth.json)
pub async fn sync_agent_tokens(State(state): State<AppState>) -> Json<ApiResponse<Vec<String>>> {
    let config = state.config.read().await;
    let mut synced: Vec<String> = Vec::new();

    // ── Codex: ~/.codex/auth.json ──
    if let Some(openai_key) = config.tokens.active_key_for("openai") {
        if !config
            .tokens
            .disabled_overrides
            .contains(&"openai".to_string())
        {
            sync_codex_auth(Some(openai_key));
            synced.push("Codex".into());
        }
    }

    // ── Gemini CLI: ~/.gemini/settings.json ──
    if let Some(google_key) = config.tokens.active_key_for("google") {
        if !config
            .tokens
            .disabled_overrides
            .contains(&"google".to_string())
        {
            crate::core::key_discovery::write_gemini_key(Some(google_key));
            synced.push("Gemini CLI".into());
        }
    }

    Json(ApiResponse::ok(synced))
}

/// POST /api/config/discover-keys
/// Scan env vars and agent config files for API keys, auto-import new ones.
/// Keys are named after the host username by default.
pub async fn discover_keys(
    State(state): State<AppState>,
) -> Json<ApiResponse<DiscoverKeysResponse>> {
    let discovered = crate::core::key_discovery::discover_keys().await;
    let mut config = state.config.write().await;
    let mut imported_count = 0u32;
    let mut results = Vec::new();

    for dk in discovered {
        // Check duplicate: does any existing key have the same value?
        let already_exists = config.tokens.keys.iter().any(|k| k.value == dk.value);

        if !already_exists {
            let is_first = !config.tokens.keys.iter().any(|k| k.provider == dk.provider);
            config.tokens.keys.push(ApiKey {
                id: uuid::Uuid::new_v4().to_string(),
                name: dk.suggested_name.clone(),
                provider: dk.provider.clone(),
                value: dk.value,
                active: is_first,
            });
            imported_count += 1;
        }

        results.push(DiscoveredKey {
            provider: dk.provider,
            source: dk.source,
            suggested_name: dk.suggested_name,
            already_exists,
        });
    }

    if imported_count > 0 {
        match config::save(&config).await {
            Ok(_) => tracing::info!("Auto-imported {} API key(s)", imported_count),
            // Keys live only in memory now — claiming success would leave the
            // user believing they persisted (they vanish at next restart).
            Err(e) => {
                tracing::error!("Auto-imported {} API key(s) but SAVING the config failed: {e} — keys are in memory only and will be lost at restart", imported_count);
                return Json(ApiResponse::err(format!(
                    "Imported {imported_count} key(s) but persisting config.toml failed: {e}"
                )));
            }
        }
    }

    Json(ApiResponse::ok(DiscoverKeysResponse {
        discovered: results,
        imported_count,
    }))
}

fn mask_token(token: &str) -> String {
    if token.len() <= 8 {
        return "*".repeat(token.len());
    }
    format!("{}...{}", &token[..4], &token[token.len() - 4..])
}

/// Clamp the operator-supplied agent inactivity timeout (in minutes) to
/// the supported range. The upper bound was 60 min until 0.7.0 — bumped
/// to 120 because heavy Ticket Autopilot implements (real enterprise
/// tickets with 8-10 sub-tasks) routinely run 60-90 min of streamed
/// activity. A 60 min cap killed those mid-run even when the agent was
/// healthily emitting tool calls. The operator can still set 1 min if
/// they want aggressive babysitting.
pub(crate) fn clamp_stall_timeout_min(input: u64) -> u32 {
    input.clamp(1, 120) as u32
}

/// GET /api/config/db-info
pub async fn db_info(State(state): State<AppState>) -> Json<ApiResponse<DbInfo>> {
    let size_bytes = std::fs::metadata(state.db.path())
        .map(|m| m.len())
        .unwrap_or(0);

    // Count custom skills/directives/profiles (file-based, not in DB)
    let custom_skill_count = crate::core::skills::list_all_skills()
        .iter()
        .filter(|s| !s.is_builtin)
        .count() as u32;
    let custom_profile_count = crate::core::profiles::list_all_profiles()
        .iter()
        .filter(|p| !p.is_builtin)
        .count() as u32;
    let custom_directive_count = crate::core::directives::list_all_directives()
        .iter()
        .filter(|d| !d.is_builtin)
        .count() as u32;

    match state
        .db
        .with_conn(move |conn| {
            let count = |table: &str| -> u32 {
                conn.query_row(&format!("SELECT COUNT(*) FROM {}", table), [], |r| r.get(0))
                    .unwrap_or(0)
            };
            Ok(DbInfo {
                size_bytes,
                project_count: count("projects"),
                discussion_count: count("discussions"),
                message_count: count("messages"),
                mcp_count: count("mcp_configs"),
                workflow_count: count("workflows"),
                workflow_run_count: count("workflow_runs"),
                custom_skill_count,
                custom_profile_count,
                custom_directive_count,
            })
        })
        .await
    {
        Ok(info) => Json(ApiResponse::ok(info)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// Result of a `POST /api/db/backup` call. The frontend surfaces the
/// `backup_path` so the user can copy it (or the absolute `.bak` ref
/// if they want to script around it).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DbBackupResponse {
    /// Absolute path of the backup file written.
    pub backup_path: String,
    /// Size of the backup in bytes (sanity check + UI display).
    pub size_bytes: u64,
    /// Timestamp the backup was taken (ISO-8601 UTC).
    pub taken_at: chrono::DateTime<chrono::Utc>,
}

/// `POST /api/db/backup` — write a consistent snapshot of the live
/// database to `<data_dir>/backups/kronn-YYYYMMDD-HHMM.db`.
///
/// Uses SQLite's online-backup API (via `rusqlite::backup::Backup`)
/// so the snapshot is consistent even while the backend has the DB
/// open. Equivalent to the runbook's manual
/// `sqlite3 .backup '...'` command, but reachable from Settings →
/// Server with one click.
///
/// Idempotent: re-running drops a new file with a fresh timestamp.
/// Older backups stay — the operator decides retention.
pub async fn db_backup(State(state): State<AppState>) -> Json<ApiResponse<DbBackupResponse>> {
    let source_path = state.db.path().clone();

    // In-memory DB (test mode) → nothing meaningful to back up.
    if source_path.to_string_lossy() == ":memory:" {
        return Json(ApiResponse::err(
            "Database is in-memory; backup is a no-op (check KRONN_DATA_DIR config)".to_string(),
        ));
    }

    // Backup directory: sibling of the live DB. Self-contained, no
    // env wrangling. Operator can move/copy the file afterwards.
    let backup_dir = match source_path.parent() {
        Some(p) => p.join("backups"),
        None => {
            return Json(ApiResponse::err(
                "Cannot determine backup parent dir from DB path".to_string(),
            ))
        }
    };
    if let Err(e) = std::fs::create_dir_all(&backup_dir) {
        return Json(ApiResponse::err(format!(
            "Failed to create backup dir: {}",
            e
        )));
    }

    let now = chrono::Utc::now();
    let backup_filename = format!("kronn-{}.db", now.format("%Y%m%d-%H%M%S"));
    let backup_path = backup_dir.join(&backup_filename);

    // Run the SQLite online-backup inside the DB executor so the
    // source connection's mutex is held for the duration. This
    // mirrors how `with_conn` runs every other query — the operator
    // doesn't need a quiet window, the API holds the mutex while it
    // copies pages.
    let backup_path_owned = backup_path.clone();
    let result = state
        .db
        .with_conn(move |conn| {
            let mut dst = rusqlite::Connection::open(&backup_path_owned)?;
            let backup = rusqlite::backup::Backup::new(conn, &mut dst)?;
            // One-shot copy via `step(-1)` (all pages in a single call). Pausing
            // between page batches only helps when OTHER connections could write
            // in the gaps — Kronn has a single shared connection, so a pause just
            // holds the global mutex longer (~2.5s/MB) for no benefit.
            // NOT `run_to_completion(-1, …)`: it asserts pages_per_step > 0 and
            // panics (2026-07-09 boot-tick incident — poisoned the DB mutex).
            match backup.step(-1)? {
                rusqlite::backup::StepResult::Done => {}
                other => anyhow::bail!("backup did not complete in one step: {other:?}"),
            }
            Ok(())
        })
        .await;

    match result {
        Ok(()) => {
            let size_bytes = std::fs::metadata(&backup_path)
                .map(|m| m.len())
                .unwrap_or(0);
            tracing::info!(
                "DB backup written: {} ({} bytes)",
                backup_path.display(),
                size_bytes
            );
            Json(ApiResponse::ok(DbBackupResponse {
                backup_path: backup_path.to_string_lossy().to_string(),
                size_bytes,
                taken_at: now,
            }))
        }
        Err(e) => {
            // Clean up the partial file if the backup failed mid-flight.
            let _ = std::fs::remove_file(&backup_path);
            Json(ApiResponse::err(format!("Backup failed: {}", e)))
        }
    }
}

/// Build the DbExport from current state
async fn build_export(state: &AppState) -> Result<DbExport, String> {
    // ADR-001 O2 — the export walks EVERY table; read connection.
    let projects = state
        .db
        .with_read_conn(crate::db::projects::list_projects)
        .await
        .map_err(|e| format!("DB error: {}", e))?;
    let discussions = state
        .db
        .with_read_conn(crate::db::discussions::list_discussions_with_messages)
        .await
        .map_err(|e| format!("DB error: {}", e))?;
    let (workflows, mcp_servers, mcp_configs) = state
        .db
        .with_read_conn(|conn| {
            let wf = crate::db::workflows::list_workflows(conn)?;
            let servers = crate::db::mcps::list_servers(conn)?;
            let configs = crate::db::mcps::list_configs(conn)?;
            Ok((wf, servers, configs))
        })
        .await
        .map_err(|e| format!("DB error: {}", e))?;
    let contacts = state
        .db
        .with_read_conn(crate::db::contacts::list_contacts)
        .await
        .map_err(|e| format!("DB error: {}", e))?;
    let quick_prompts = state
        .db
        .with_read_conn(crate::db::quick_prompts::list_quick_prompts)
        .await
        .map_err(|e| format!("DB error: {}", e))?;
    let quick_apis = state
        .db
        .with_read_conn(crate::db::quick_apis::list_quick_apis)
        .await
        .map_err(|e| format!("DB error: {}", e))?;
    // All learnings, every status — pending candidates and promoted facts both
    // matter on a migrated box (None filters = no status / no project narrowing).
    let learnings = state
        .db
        .with_read_conn(|conn| crate::db::learnings::list(conn, None, None))
        .await
        .map_err(|e| format!("DB error: {}", e))?;
    // v5 (passe D) — QP version lineage + rejection counters, previously lost.
    let quick_prompt_versions = state
        .db
        .with_read_conn(crate::db::quick_prompts::list_all_quick_prompt_versions)
        .await
        .map_err(|e| format!("DB error: {}", e))?;
    let learning_rejections = state
        .db
        .with_read_conn(crate::db::learnings::list_rejections)
        .await
        .map_err(|e| format!("DB error: {}", e))?;

    let custom_skills: Vec<_> = crate::core::skills::list_all_skills()
        .into_iter()
        .filter(|s| !s.is_builtin)
        .collect();
    let custom_directives: Vec<_> = crate::core::directives::list_all_directives()
        .into_iter()
        .filter(|d| !d.is_builtin)
        .collect();
    let custom_profiles: Vec<_> = crate::core::profiles::list_all_profiles()
        .into_iter()
        .filter(|p| !p.is_builtin)
        .collect();

    Ok(DbExport {
        version: crate::models::db::CURRENT_EXPORT_VERSION,
        exported_at: Utc::now(),
        projects,
        discussions,
        workflows,
        mcp_servers,
        mcp_configs,
        custom_skills,
        custom_directives,
        custom_profiles,
        contacts,
        quick_prompts,
        quick_apis,
        learnings,
        quick_prompt_versions,
        learning_rejections,
    })
}

/// Build an exportable config.toml (without auth_token, encryption_secret, and API key values)
fn build_export_config(config: &AppConfig) -> AppConfig {
    let mut export_cfg = config.clone();
    // Strip secrets
    export_cfg.server.auth_token = None;
    export_cfg.server.auth_enabled = false;
    export_cfg.encryption_secret = None;
    // Strip API key values (keep metadata for reference)
    for key in &mut export_cfg.tokens.keys {
        key.value = String::new();
    }
    export_cfg
}

/// GET /api/config/export — returns a ZIP containing data.json + config.toml
pub async fn export_data(State(state): State<AppState>) -> Response {
    let db_export = match build_export(&state).await {
        Ok(e) => e,
        Err(msg) => return (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response(),
    };

    let data_json = match serde_json::to_string_pretty(&db_export) {
        Ok(j) => j,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("JSON error: {}", e),
            )
                .into_response()
        }
    };

    let config = state.config.read().await;
    let export_cfg = build_export_config(&config);
    let config_toml = match toml::to_string_pretty(&export_cfg) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("TOML error: {}", e),
            )
                .into_response()
        }
    };
    drop(config);

    // P2 — bundle the recovery blob (the encryption key wrapped under the user's
    // Argon2id passphrase) when one is configured. With it, THIS export + the
    // passphrase restore the plugin SECRETS too, not just the data — the export
    // used to carry undecryptable ciphertext only (the 2026-06-30 re-enter-
    // everything pain). Safe to ship: the blob is useless without the passphrase;
    // the raw key itself is still never exported.
    let recovery_code = config::config_dir()
        .ok()
        .and_then(|d| crate::core::recovery::load_blob(&d))
        .map(|b| crate::core::recovery::to_code(&b));

    let bytes = match build_export_zip(&data_json, &config_toml, recovery_code.as_deref()) {
        Ok(b) => b,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"kronn-export.zip\"",
        )
        .body(Body::from(bytes))
        .unwrap()
}

/// Assemble the export ZIP: data.json + config.toml (+ recovery.key when the
/// user configured a recovery passphrase). Factored out of the handler so the
/// archive layout is unit-testable.
fn build_export_zip(
    data_json: &str,
    config_toml: &str,
    recovery_code: Option<&str>,
) -> Result<Vec<u8>, String> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        zip.start_file("data.json", options)
            .map_err(|e| format!("ZIP error: {}", e))?;
        std::io::Write::write_all(&mut zip, data_json.as_bytes())
            .map_err(|e| format!("ZIP write error: {}", e))?;

        zip.start_file("config.toml", options)
            .map_err(|e| format!("ZIP error: {}", e))?;
        std::io::Write::write_all(&mut zip, config_toml.as_bytes())
            .map_err(|e| format!("ZIP write error: {}", e))?;

        if let Some(code) = recovery_code {
            zip.start_file("recovery.key", options)
                .map_err(|e| format!("ZIP error: {}", e))?;
            std::io::Write::write_all(&mut zip, code.as_bytes())
                .map_err(|e| format!("ZIP write error: {}", e))?;
        }

        zip.finish()
            .map_err(|e| format!("ZIP finish error: {}", e))?;
    }
    Ok(buf.into_inner())
}

/// Install a recovery blob carried by an imported backup — without EVER
/// destroying existing recovery material: a differing local blob (it protects
/// the LOCAL key) is copied to `recovery.key.backup` first. Once installed, the
/// Plugins restore panel works with the source machine's passphrase alone.
/// Returns user-facing warnings for the ImportResult.
fn persist_imported_recovery(dir: &std::path::Path, code: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    let Ok(blob) = crate::core::recovery::from_code(code) else {
        warnings
            .push("The backup carries a recovery blob but it is malformed — ignored.".to_string());
        return warnings;
    };
    match crate::core::recovery::load_blob(dir) {
        Some(existing) if existing == blob => {} // already in place — nothing to do
        Some(_) => {
            let _ = std::fs::copy(
                dir.join(crate::core::recovery::RECOVERY_FILENAME),
                dir.join("recovery.key.backup"),
            );
            if crate::core::recovery::save_blob(dir, &blob).is_ok() {
                warnings.push(
                    "The backup's recovery blob was installed (this machine's previous one was \
                     kept as recovery.key.backup). If imported plugin secrets are unreadable, \
                     use Plugins → 'Restore from recovery passphrase' with the passphrase set \
                     on the SOURCE machine."
                        .to_string(),
                );
            }
        }
        None => {
            if crate::core::recovery::save_blob(dir, &blob).is_ok() {
                warnings.push(
                    "The backup's recovery blob was installed. If imported plugin secrets are \
                     unreadable, use Plugins → 'Restore from recovery passphrase' with the \
                     passphrase set on the SOURCE machine."
                        .to_string(),
                );
            }
        }
    }
    warnings
}

/// Which table-clear statements a selective import runs for this payload. Only
/// clears a table group when the export carries rows for it, so an older or
/// partial export can NEVER wipe a table it doesn't contain (the 2026-06-29
/// quick_apis/learnings silent-wipe). Child rows precede parents to respect FKs.
fn import_clear_statements(data: &DbExport) -> Vec<&'static str> {
    let mut stmts: Vec<&'static str> = Vec::new();
    if !data.discussions.is_empty() {
        stmts.push("DELETE FROM messages");
        stmts.push("DELETE FROM discussions");
    }
    if !data.mcp_servers.is_empty() || !data.mcp_configs.is_empty() {
        stmts.push("DELETE FROM mcp_config_projects");
        stmts.push("DELETE FROM mcp_configs");
        stmts.push("DELETE FROM mcp_servers");
    }
    if !data.workflows.is_empty() {
        stmts.push("DELETE FROM workflow_runs");
        stmts.push("DELETE FROM workflows");
    }
    if !data.contacts.is_empty() {
        stmts.push("DELETE FROM contacts");
    }
    if !data.quick_prompts.is_empty() {
        stmts.push("DELETE FROM quick_prompts");
    }
    // Versions clear ONLY when the archive carries some (v5+): a v4 export
    // must not wipe local lineage it knows nothing about. Referential
    // integrity vs the replaced parents is handled by the post-import prune.
    if !data.quick_prompt_versions.is_empty() {
        stmts.push("DELETE FROM quick_prompt_versions");
    }
    if !data.quick_apis.is_empty() {
        stmts.push("DELETE FROM quick_apis");
    }
    if !data.learnings.is_empty() {
        stmts.push("DELETE FROM learnings");
    }
    if !data.learning_rejections.is_empty() {
        stmts.push("DELETE FROM learning_rejections");
    }
    if !data.projects.is_empty() {
        stmts.push("DELETE FROM projects");
    }
    stmts
}

/// Import DB data from a DbExport struct. Returns warnings and invalid paths.
async fn do_import_db(state: &AppState, data: &DbExport) -> Result<ImportResult, String> {
    let mut warnings = Vec::new();
    let mut invalid_paths = Vec::new();

    // Downgrade guard: an export OLDER than this build can't carry tables added
    // since (they deserialize to empty), so restoring it must NOT be read as
    // "the user has none of X". Warn loudly, and clear SELECTIVELY below.
    if data.version < crate::models::db::CURRENT_EXPORT_VERSION {
        warnings.push(format!(
            "This backup is an older format (v{} < v{}). Tables it doesn't carry are left \
             untouched instead of wiped — your current data for any newer feature is preserved.",
            data.version,
            crate::models::db::CURRENT_EXPORT_VERSION
        ));
    }

    // Selective clear (replaces the old unconditional wipe): only clear the
    // tables the payload actually carries — see `import_clear_statements`.
    let stmts = import_clear_statements(data);
    if !stmts.is_empty() {
        let batch = format!("{};", stmts.join("; "));
        state
            .db
            .with_conn(move |conn| {
                conn.execute_batch(&batch)?;
                Ok(())
            })
            .await
            .map_err(|e| format!("Failed to clear DB: {}", e))?;
    }

    // Import projects (check path validity)
    for project in &data.projects {
        let p = project.clone();
        let path = project.path.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| crate::db::projects::insert_project(conn, &p))
            .await
        {
            tracing::warn!("Import project error: {}", e);
        }
        if !std::path::Path::new(&path).exists() {
            invalid_paths.push(path);
        }
    }

    // Import discussions with their messages
    for disc in &data.discussions {
        let d = disc.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| crate::db::discussions::insert_discussion(conn, &d))
            .await
        {
            tracing::warn!("Import discussion error: {}", e);
        }
        let did = disc.id.clone();
        for msg in &disc.messages {
            let m = msg.clone();
            let id = did.clone();
            if let Err(e) = state
                .db
                .with_conn(move |conn| crate::db::discussions::insert_message(conn, &id, &m))
                .await
            {
                tracing::error!("Failed to import discussion message: {e}");
            }
        }
    }

    // Import MCP servers & configs
    for server in &data.mcp_servers {
        let s = server.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| crate::db::mcps::upsert_server(conn, &s))
            .await
        {
            tracing::error!("Failed to import MCP server: {e}");
        }
    }
    for config_entry in &data.mcp_configs {
        let c = config_entry.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| crate::db::mcps::insert_config(conn, &c))
            .await
        {
            tracing::error!("Failed to import MCP config: {e}");
        }
    }

    // Import workflows
    for wf in &data.workflows {
        let w = wf.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &w))
            .await
        {
            tracing::error!("Failed to import workflow: {e}");
        }
    }

    // Import custom skills/directives/profiles (file-based)
    for skill in &data.custom_skills {
        let _ = crate::core::skills::save_custom_skill(
            &skill.name,
            &skill.description,
            &skill.icon,
            &skill.category,
            &skill.content,
            skill.license.as_deref(),
            skill.allowed_tools.as_deref(),
        );
    }
    for directive in &data.custom_directives {
        let _ = crate::core::directives::save_custom_directive(
            &directive.name,
            &directive.description,
            &directive.icon,
            &directive.category,
            &directive.content,
            &directive.conflicts,
        );
    }
    for profile in &data.custom_profiles {
        let _ =
            crate::core::profiles::save_custom_profile(&crate::core::profiles::CustomProfileData {
                name: &profile.name,
                persona_name: &profile.persona_name,
                role: &profile.role,
                avatar: &profile.avatar,
                color: &profile.color,
                category: &profile.category,
                persona_prompt: &profile.persona_prompt,
                default_engine: profile.default_engine.as_deref(),
            });
    }

    // Import contacts
    for contact in &data.contacts {
        let c = contact.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| crate::db::contacts::insert_contact(conn, &c))
            .await
        {
            tracing::warn!("Import contact error: {}", e);
        }
    }

    // Import quick prompts
    for qp in &data.quick_prompts {
        let q = qp.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| crate::db::quick_prompts::insert_quick_prompt(conn, &q))
            .await
        {
            tracing::warn!("Import quick prompt error: {}", e);
        }
    }

    // Import QP version history (v5) — verbatim rows, after their parents.
    for v in &data.quick_prompt_versions {
        let v = v.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| {
                crate::db::quick_prompts::insert_quick_prompt_version_row(conn, &v)
            })
            .await
        {
            tracing::warn!("Import quick prompt version error: {}", e);
        }
    }

    // Import quick APIs
    for qa in &data.quick_apis {
        let a = qa.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| crate::db::quick_apis::insert_quick_api(conn, &a))
            .await
        {
            tracing::warn!("Import quick API error: {}", e);
        }
    }

    // Import continual-learning candidates. `learnings::insert` rejects rows
    // with empty evidence[] — every stored learning has at least one, so a
    // failure here is a corrupt export, logged not fatal.
    for learning in &data.learnings {
        let l = learning.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| crate::db::learnings::insert(conn, &l))
            .await
        {
            tracing::warn!("Import learning error: {}", e);
        }
    }

    // Referential prune — local version rows whose parent QP no longer
    // exists after the import (v4 archive: parents replaced, lineage kept
    // for same-id QPs, orphans dropped).
    match state
        .db
        .with_conn(|conn| {
            conn.execute(
                "DELETE FROM quick_prompt_versions
             WHERE quick_prompt_id NOT IN (SELECT id FROM quick_prompts)",
                [],
            )
            .map_err(Into::into)
        })
        .await
    {
        Ok(n) if n > 0 => warnings.push(format!(
            "{n} quick-prompt version row(s) dropped — their prompts are not part of this import"
        )),
        Ok(_) => {}
        Err(e) => tracing::warn!("Import version prune error: {}", e),
    }

    // Import rejection counters (v5) — verbatim, keeps the anti-repetition
    // threshold armed across a migration.
    for rej in &data.learning_rejections {
        let r = rej.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| crate::db::learnings::insert_rejection_row(conn, &r))
            .await
        {
            tracing::warn!("Import learning rejection error: {}", e);
        }
    }

    if !invalid_paths.is_empty() {
        warnings.push(format!(
            "{} project(s) have invalid paths — remap them in the Projects page",
            invalid_paths.len()
        ));
    }

    Ok(ImportResult {
        warnings,
        invalid_paths,
    })
}

/// Merge imported config into current config (language, pseudo, bio, scan_paths — keep existing keys)
async fn merge_import_config(state: &AppState, imported: &AppConfig) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut config = state.config.write().await;

    // Merge user identity
    if imported.server.pseudo.is_some() {
        config.server.pseudo = imported.server.pseudo.clone();
    }
    if imported.server.avatar_email.is_some() {
        config.server.avatar_email = imported.server.avatar_email.clone();
    }
    if imported.server.bio.is_some() {
        config.server.bio = imported.server.bio.clone();
    }

    // Merge global context (injected into every agent prompt). It travels in
    // the exported config.toml but was previously dropped on import — a silent
    // loss of the user's cross-project instructions. Only overwrite when the
    // import actually carries one, so re-importing a context-less export onto a
    // configured box doesn't wipe it.
    if imported.server.global_context.is_some() {
        config.server.global_context = imported.server.global_context.clone();
        config.server.global_context_mode = imported.server.global_context_mode.clone();
    }

    // Merge language
    if !imported.language.is_empty() {
        config.language = imported.language.clone();
    }

    // Merge scan paths (union)
    for path in &imported.scan.paths {
        if !config.scan.paths.contains(path) {
            config.scan.paths.push(path.clone());
        }
    }

    // Check for MCP secrets warning: if imported config has any MCP-related env vars,
    // the encryption_secret is different so they need reconfiguration
    warnings.push(
        "MCP secrets are encrypted with a different key — reconfigure them in the Plugins page"
            .to_string(),
    );

    if let Err(e) = config::save(&config).await {
        tracing::error!("Failed to save merged config: {}", e);
        warnings.push(format!("Failed to save config: {}", e));
    }

    warnings
}

/// Per-entry DECOMPRESSED caps for the import ZIP (passe D). The 512 MiB
/// route-layer limit only bounds the compressed upload — a highly
/// compressible entry could otherwise balloon to an arbitrary allocation
/// (zip bomb) before any validation runs.
const ZIP_CAP_DATA_JSON: u64 = 512 * 1024 * 1024;
const ZIP_CAP_CONFIG_TOML: u64 = 1024 * 1024;
const ZIP_CAP_RECOVERY_KEY: u64 = 64 * 1024;

/// Read a ZIP entry into a String, refusing past `cap` decompressed bytes.
/// `take(cap + 1)` makes the overflow detectable without allocating it.
fn read_zip_entry_capped(f: impl std::io::Read, name: &str, cap: u64) -> Result<String, String> {
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut f.take(cap + 1), &mut contents)
        .map_err(|e| format!("Failed to read {name}: {e}"))?;
    if contents.len() as u64 > cap {
        return Err(format!(
            "{name} exceeds the {cap}-byte decompressed limit — refusing (zip bomb?)"
        ));
    }
    Ok(contents)
}

/// Extract data.json, config.toml and the optional recovery blob from a ZIP
/// file (synchronous, no await).
fn extract_zip(file_bytes: &[u8]) -> Result<(DbExport, Option<AppConfig>, Option<String>), String> {
    let cursor = std::io::Cursor::new(file_bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("Invalid ZIP: {}", e))?;

    // Read data.json (required)
    let data: DbExport = {
        let mut f = archive
            .by_name("data.json")
            .map_err(|e| format!("data.json not found in ZIP: {}", e))?;
        let contents = read_zip_entry_capped(&mut f, "data.json", ZIP_CAP_DATA_JSON)?;
        serde_json::from_str(&contents).map_err(|e| format!("Invalid data.json: {}", e))?
    };

    // Read config.toml (optional) — but an OVERSIZED one is a hard error,
    // not a silent None (that would downgrade a bomb into "no config").
    let imported_config = if let Ok(mut f) = archive.by_name("config.toml") {
        let contents = read_zip_entry_capped(&mut f, "config.toml", ZIP_CAP_CONFIG_TOML)?;
        toml::from_str::<AppConfig>(&contents).ok()
    } else {
        None
    };

    // Read recovery.key (optional, P2) — kept only if it parses as a valid
    // recovery code; a corrupt entry is dropped, never an import error.
    // An oversized one is refused like the others.
    let recovery_code = if let Ok(mut f) = archive.by_name("recovery.key") {
        let contents = read_zip_entry_capped(&mut f, "recovery.key", ZIP_CAP_RECOVERY_KEY)?;
        let trimmed = contents.trim().to_string();
        crate::core::recovery::from_code(&trimmed)
            .ok()
            .map(|_| trimmed)
    } else {
        None
    };

    Ok((data, imported_config, recovery_code))
}

/// POST /api/config/import — accepts ZIP (multipart) or JSON (legacy)
pub async fn import_data(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Json<ApiResponse<ImportResult>> {
    // Read the uploaded file
    let file_bytes = match multipart.next_field().await {
        Ok(Some(field)) => match field.bytes().await {
            Ok(b) => b,
            Err(e) => return Json(ApiResponse::err(format!("Failed to read upload: {}", e))),
        },
        Ok(None) => return Json(ApiResponse::err("No file uploaded".to_string())),
        Err(e) => return Json(ApiResponse::err(format!("Multipart error: {}", e))),
    };

    // Detect format: ZIP (starts with PK\x03\x04) or JSON legacy
    let is_zip = file_bytes.len() >= 4
        && file_bytes[0] == b'P'
        && file_bytes[1] == b'K'
        && file_bytes[2] == 0x03
        && file_bytes[3] == 0x04;

    if is_zip {
        // Extract ZIP (sync — no await needed, avoids Send issues with zip reader)
        let (data, imported_config, recovery_code) = match extract_zip(&file_bytes) {
            Ok(r) => r,
            Err(e) => return Json(ApiResponse::err(e)),
        };

        // Import DB data
        let mut result = match do_import_db(&state, &data).await {
            Ok(r) => r,
            Err(e) => return Json(ApiResponse::err(e)),
        };

        // Merge config if present
        if let Some(cfg) = imported_config {
            let config_warnings = merge_import_config(&state, &cfg).await;
            result.warnings.extend(config_warnings);
        }

        // P2 — install the backup's recovery blob so the source passphrase can
        // unlock the imported secrets (never destroys local recovery material).
        if let Some(code) = recovery_code {
            if let Ok(dir) = config::config_dir() {
                result
                    .warnings
                    .extend(persist_imported_recovery(&dir, &code));
            }
        }

        Json(ApiResponse::ok(result))
    } else {
        // Legacy JSON import (v2 compat)
        let data: DbExport = match serde_json::from_slice(&file_bytes) {
            Ok(d) => d,
            Err(e) => return Json(ApiResponse::err(format!("Invalid JSON: {}", e))),
        };

        let result = match do_import_db(&state, &data).await {
            Ok(r) => r,
            Err(e) => return Json(ApiResponse::err(e)),
        };

        Json(ApiResponse::ok(result))
    }
}

/// POST /api/setup/reset
/// Delete config file to trigger first-run wizard again
pub async fn reset(State(state): State<AppState>) -> Json<ApiResponse<()>> {
    // Delete config file
    if let Ok(path) = config::config_path() {
        let _ = tokio::fs::remove_file(&path).await;
        tracing::info!("Config reset: {}", path.display());
    }

    // Reset in-memory config to defaults
    let mut cfg = state.config.write().await;
    *cfg = config::default_config();

    // Clear all data from DB
    if let Err(e) = state.db.with_conn(|conn| {
        conn.execute_batch(
            "DELETE FROM messages; DELETE FROM discussions; DELETE FROM mcp_config_projects; DELETE FROM mcp_configs; DELETE FROM mcp_servers; DELETE FROM projects;"
        )?;
        Ok(())
    }).await {
        tracing::error!("Failed to clear database during reset: {e}");
    }

    Json(ApiResponse::ok(()))
}

// ── Recovery passphrase (P2) ────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct SetRecoveryRequest {
    pub passphrase: String,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SetRecoveryResponse {
    /// The off-machine copy the user must save. With it + the passphrase, the
    /// encryption key survives total loss of the machine / keychain / data dir.
    pub recovery_code: String,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct RecoveryStatus {
    pub configured: bool,
}

#[derive(serde::Deserialize)]
pub struct RestoreRecoveryRequest {
    pub passphrase: String,
    /// Optional: the saved recovery code. When omitted, the local `recovery.key`
    /// sidecar is used (works unless the whole data dir was lost).
    #[serde(default)]
    pub recovery_code: Option<String>,
}

/// GET /api/config/recovery/status — is a recovery passphrase configured?
pub async fn recovery_status() -> Json<ApiResponse<RecoveryStatus>> {
    let configured = config::config_dir()
        .map(|d| crate::core::recovery::is_configured(&d))
        .unwrap_or(false);
    Json(ApiResponse::ok(RecoveryStatus { configured }))
}

/// POST /api/config/recovery/set — wrap the active key under a passphrase and
/// return the recovery code to save. Strongly-offered, non-blocking (no wizard
/// gate). Auth-gated like the destructive endpoints (see `auth_allows`).
pub async fn set_recovery(
    State(state): State<AppState>,
    Json(req): Json<SetRecoveryRequest>,
) -> Json<ApiResponse<SetRecoveryResponse>> {
    let config = state.config.read().await;
    match crate::core::keystore::set_recovery_passphrase(&config, &req.passphrase) {
        Ok(recovery_code) => Json(ApiResponse::ok(SetRecoveryResponse { recovery_code })),
        Err(e) => Json(ApiResponse::err(e.to_string())),
    }
}

/// POST /api/config/recovery/restore — restore the encryption key from a
/// passphrase (+ optional saved recovery code) when the token subsystem is
/// locked. Verifies the recovered key actually decrypts this instance's data,
/// then mirrors it back into the keychain/sidecar. Auth-gated.
pub async fn restore_recovery(
    State(state): State<AppState>,
    Json(req): Json<RestoreRecoveryRequest>,
) -> Json<ApiResponse<()>> {
    let dir = match config::config_dir() {
        Ok(d) => d,
        Err(e) => return Json(ApiResponse::err(e.to_string())),
    };
    let store = crate::core::keyvault::KeyStore::standard(&dir);
    let mut config = state.config.write().await;
    match crate::core::keystore::recover_with_passphrase(
        &mut config,
        &state.db,
        &store,
        &req.passphrase,
        req.recovery_code.as_deref(),
        &dir,
    )
    .await
    {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(e.to_string())),
    }
}

// ── Open URL in system browser ─────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct OpenUrlRequest {
    pub url: String,
}

/// POST /api/open-url — open a URL in the system default browser.
/// Used by the Tauri desktop app where webview doesn't handle target="_blank".
/// In Docker mode this is a no-op (no desktop to open).
pub async fn open_url(Json(req): Json<OpenUrlRequest>) -> Json<ApiResponse<()>> {
    if !req.url.starts_with("http://") && !req.url.starts_with("https://") {
        return Json(ApiResponse::err("Only http/https URLs are allowed"));
    }
    // Test binaries (target/*/deps) must never launch a real browser: the
    // integration suite exercises this endpoint's contract, and on macOS the
    // real open() popped an example.com tab at every `cargo test` run.
    let in_test_binary = std::env::current_exe()
        .map(|p| p.components().any(|c| c.as_os_str() == "deps"))
        .unwrap_or(false);
    if in_test_binary {
        tracing::info!("open-url suppressed in a test binary: {}", req.url);
        return Json(ApiResponse::ok(()));
    }
    match open::that(&req.url) {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(e) => {
            tracing::warn!("Failed to open URL '{}': {}", req.url, e);
            Json(ApiResponse::err(format!("Failed to open URL: {}", e)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config;
    use serial_test::serial;

    // ─── extract_zip decompressed caps (passe D: zip bomb) ────────────────

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, bytes) in entries {
                w.start_file(*name, opts).unwrap();
                w.write_all(bytes).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn extract_zip_refuses_oversized_decompressed_entries() {
        // A tiny COMPRESSED upload can hide megabytes of zeros — the
        // route-layer 512 MiB body limit never sees the decompressed size.
        let data = serde_json::to_vec(&empty_export()).unwrap();
        let bomb_key = vec![b' '; (ZIP_CAP_RECOVERY_KEY + 1024) as usize];
        let bytes = zip_with(&[("data.json", &data), ("recovery.key", &bomb_key)]);
        let err = extract_zip(&bytes).expect_err("oversized recovery.key must refuse");
        assert!(err.contains("zip bomb"), "{err}");

        let bomb_cfg = vec![b' '; (ZIP_CAP_CONFIG_TOML + 1024) as usize];
        let bytes = zip_with(&[("data.json", &data), ("config.toml", &bomb_cfg)]);
        let err = extract_zip(&bytes).expect_err("oversized config.toml must refuse");
        assert!(err.contains("zip bomb"), "{err}");

        // Within caps → the archive still imports (recovery dropped as
        // non-parsing, config parsed, data ok).
        let bytes = zip_with(&[("data.json", &data), ("recovery.key", b"not-a-code")]);
        let (export, cfg, key) = extract_zip(&bytes).expect("valid archive imports");
        assert_eq!(export.version, crate::models::db::CURRENT_EXPORT_VERSION);
        assert!(cfg.is_none());
        assert!(
            key.is_none(),
            "non-parsing recovery code is dropped, not an error"
        );
    }

    // ─── export v5 round-trip (passe D: QP versions + rejection counters) ────

    #[tokio::test]
    async fn export_v5_round_trips_qp_versions_and_rejection_counters() {
        let mk_state = || async {
            let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
            let cfg = std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            ));
            crate::AppState::new_defaults(cfg, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
        };
        let source = mk_state().await;

        // Seed: one QP + one snapshot version + one armed rejection counter.
        source
            .db
            .with_conn(|conn| {
                let qp: crate::models::QuickPrompt = serde_json::from_value(serde_json::json!({
                    "id": "qp-1", "name": "QP", "icon": "x", "prompt_template": "T {{v}}",
                    "variables": [], "agent": "ClaudeCode", "project_id": null,
                    "skill_ids": [], "profile_ids": [], "directive_ids": [],
                    "tier": "default", "description": "",
                    "created_at": chrono::Utc::now().to_rfc3339(),
                    "updated_at": chrono::Utc::now().to_rfc3339(),
                }))
                .unwrap();
                // insert_quick_prompt auto-snapshots version 1; a second snapshot
                // gives the lineage a real history to round-trip.
                crate::db::quick_prompts::insert_quick_prompt(conn, &qp)?;
                crate::db::quick_prompts::snapshot_quick_prompt_version(conn, &qp)?;
                crate::db::learnings::record_rejection(conn, "hash-1", "too vague")?;
                crate::db::learnings::record_rejection(conn, "hash-1", "too vague")?;
                Ok(())
            })
            .await
            .unwrap();

        let export = build_export(&source).await.expect("export");
        assert_eq!(export.version, 5);
        assert_eq!(
            export.quick_prompt_versions.len(),
            2,
            "version lineage exported"
        );
        assert_eq!(export.learning_rejections.len(), 1);
        assert_eq!(
            export.learning_rejections[0].count, 2,
            "cumulative count exported"
        );

        // Import into a FRESH instance — both tables restored verbatim.
        let target = mk_state().await;
        do_import_db(&target, &export).await.expect("import");
        let (versions, rej_count) = target
            .db
            .with_conn(|conn| {
                let v = crate::db::quick_prompts::list_quick_prompt_versions(conn, "qp-1")?;
                let c = crate::db::learnings::rejection_count(conn, "hash-1")?;
                Ok((v, c))
            })
            .await
            .unwrap();
        assert_eq!(versions.len(), 2, "version history survives the migration");
        assert_eq!(
            versions[0].version_index, 2,
            "newest first, indices preserved verbatim"
        );
        assert_eq!(rej_count, 2, "anti-repetition threshold stays armed");
    }

    #[tokio::test]
    async fn v4_import_preserves_local_lineage_and_prunes_orphans() {
        // Codex review (export v5): a v4 archive carries quick_prompts but no
        // versions — importing it must NOT wipe the local lineage of QPs it
        // re-imports (same id), while versions of QPs absent from the archive
        // are pruned with their parents.
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        let cfg = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        ));
        let state = crate::AppState::new_defaults(cfg, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);

        let mk_qp = |id: &str| -> crate::models::QuickPrompt {
            serde_json::from_value(serde_json::json!({
                "id": id, "name": id, "icon": "x", "prompt_template": "T",
                "variables": [], "agent": "ClaudeCode", "project_id": null,
                "skill_ids": [], "profile_ids": [], "directive_ids": [],
                "tier": "default", "description": "",
                "created_at": chrono::Utc::now().to_rfc3339(),
                "updated_at": chrono::Utc::now().to_rfc3339(),
            }))
            .unwrap()
        };

        // Local state: two versioned QPs.
        let (kept, dropped) = (mk_qp("qp-kept"), mk_qp("qp-dropped"));
        {
            let (kept, dropped) = (kept.clone(), dropped.clone());
            state
                .db
                .with_conn(move |conn| {
                    crate::db::quick_prompts::insert_quick_prompt(conn, &kept)?;
                    crate::db::quick_prompts::insert_quick_prompt(conn, &dropped)?;
                    Ok(())
                })
                .await
                .unwrap();
        }

        // v4 archive: carries qp-kept only, no version lineage at all.
        let mut v4 = empty_export();
        v4.version = 4;
        v4.quick_prompts = vec![kept];
        do_import_db(&state, &v4).await.expect("v4 import");

        let (kept_versions, dropped_versions) = state
            .db
            .with_conn(|conn| {
                let k = crate::db::quick_prompts::list_quick_prompt_versions(conn, "qp-kept")?;
                let d = crate::db::quick_prompts::list_quick_prompt_versions(conn, "qp-dropped")?;
                Ok((k, d))
            })
            .await
            .unwrap();
        assert!(
            !kept_versions.is_empty(),
            "v4 import must NOT wipe local lineage of a re-imported QP"
        );
        assert!(
            dropped_versions.is_empty(),
            "orphaned lineage (parent gone) is pruned"
        );
    }

    // ─── import_clear_statements (I10: selective clear, no downgrade wipe) ────

    fn empty_export() -> crate::models::db::DbExport {
        crate::models::db::DbExport {
            version: crate::models::db::CURRENT_EXPORT_VERSION,
            exported_at: chrono::Utc::now(),
            projects: vec![],
            discussions: vec![],
            workflows: vec![],
            mcp_servers: vec![],
            mcp_configs: vec![],
            custom_skills: vec![],
            custom_directives: vec![],
            custom_profiles: vec![],
            contacts: vec![],
            quick_prompts: vec![],
            quick_apis: vec![],
            learnings: vec![],
            quick_prompt_versions: vec![],
            learning_rejections: vec![],
        }
    }

    fn a_contact() -> crate::models::Contact {
        crate::models::Contact {
            id: "c1".into(),
            pseudo: "p".into(),
            avatar_email: None,
            kronn_url: "u".into(),
            invite_code: "x".into(),
            status: "accepted".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn empty_or_old_export_clears_nothing() {
        // THE fix: an export that carries no rows (incl. an older-format export
        // whose newer tables deserialize empty) must wipe NOTHING on the target.
        assert!(import_clear_statements(&empty_export()).is_empty());
    }

    #[test]
    fn selective_clear_only_touches_tables_the_payload_carries() {
        let mut exp = empty_export();
        exp.contacts.push(a_contact());
        let stmts = import_clear_statements(&exp);
        assert_eq!(stmts, vec!["DELETE FROM contacts"]);
        // Crucially, tables ABSENT from the payload are NOT cleared (the
        // 2026-06-29 quick_apis/learnings silent wipe).
        assert!(!stmts.iter().any(|s| s.contains("quick_apis")));
        assert!(!stmts.iter().any(|s| s.contains("learnings")));
        assert!(!stmts.iter().any(|s| s.contains("projects")));
    }

    #[test]
    fn selective_clear_orders_children_before_parents() {
        let mut exp = empty_export();
        exp.mcp_servers.push(crate::models::McpServer {
            id: "s".into(),
            name: "n".into(),
            description: String::new(),
            transport: crate::models::McpTransport::Stdio {
                command: "echo".into(),
                args: vec![],
            },
            source: crate::models::McpSource::Registry,
            api_spec: None,
        });
        let stmts = import_clear_statements(&exp);
        // FK-safe order: link table + configs cleared before the parent servers.
        let pos = |needle: &str| stmts.iter().position(|s| s.contains(needle)).unwrap();
        assert!(pos("mcp_config_projects") < pos("mcp_configs"));
        assert!(pos("mcp_configs") < pos("mcp_servers"));
    }

    // ─── clamp_stall_timeout_min (0.7.0 — 60 min cap was too aggressive
    //    for heavy implements; bumped to 120). ─────────────────────────

    #[test]
    fn stall_timeout_zero_clamps_to_one() {
        // Operator can't disable the safety net — minimum of 1 min.
        assert_eq!(clamp_stall_timeout_min(0), 1);
    }

    #[test]
    fn stall_timeout_inside_range_passes_through() {
        // Mid-range value isn't altered.
        assert_eq!(clamp_stall_timeout_min(30), 30);
        assert_eq!(clamp_stall_timeout_min(1), 1);
        assert_eq!(clamp_stall_timeout_min(120), 120);
    }

    #[test]
    fn stall_timeout_above_120_clamps_to_120() {
        // 0.7.0 ceiling. Critical regression guard: the previous cap was
        // 60. If anyone reverts the clamp, this test fails.
        assert_eq!(clamp_stall_timeout_min(121), 120);
        assert_eq!(clamp_stall_timeout_min(9999), 120);
    }

    #[test]
    fn stall_timeout_max_is_at_least_120_minutes() {
        // Documents the 0.7.0 contract: heavy Ticket Autopilot implements
        // (60-90 min streamed) must fit. Catches an accidental rollback
        // to the 0.6.x ceiling that would silently re-break those runs.
        assert!(
            clamp_stall_timeout_min(120) >= 120,
            "stall timeout cap must allow at least 120 min for heavy implements"
        );
    }

    #[test]
    fn build_export_config_strips_secrets() {
        let mut cfg = config::default_config();
        cfg.server.auth_token = Some("secret-token".into());
        cfg.encryption_secret = Some("secret-key".into());
        cfg.server.auth_enabled = true;
        cfg.tokens.keys.push(ApiKey {
            id: "k1".into(),
            name: "TestKey".into(),
            provider: "anthropic".into(),
            value: "sk-ant-real-key".into(),
            active: true,
        });
        cfg.server.pseudo = Some("TestUser".into());
        cfg.language = "fr".into();

        let exported = build_export_config(&cfg);

        // Secrets stripped
        assert!(exported.server.auth_token.is_none());
        assert!(exported.encryption_secret.is_none());
        assert!(!exported.server.auth_enabled);

        // API key values cleared but metadata kept
        assert_eq!(exported.tokens.keys.len(), 1);
        assert_eq!(exported.tokens.keys[0].name, "TestKey");
        assert_eq!(exported.tokens.keys[0].value, "");

        // User data preserved
        assert_eq!(exported.server.pseudo, Some("TestUser".into()));
        assert_eq!(exported.language, "fr");
    }

    #[test]
    fn extract_zip_roundtrip() {
        let data = DbExport {
            version: 4,
            exported_at: Utc::now(),
            quick_prompt_versions: vec![],
            learning_rejections: vec![],
            projects: vec![],
            discussions: vec![],
            workflows: vec![],
            mcp_servers: vec![],
            mcp_configs: vec![],
            custom_skills: vec![],
            custom_directives: vec![],
            custom_profiles: vec![],
            contacts: vec![],
            quick_prompts: vec![],
            quick_apis: vec![],
            learnings: vec![],
        };
        let data_json = serde_json::to_string(&data).unwrap();

        let cfg = config::default_config();
        let config_toml = toml::to_string_pretty(&cfg).unwrap();

        // Build ZIP
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("data.json", opts).unwrap();
            std::io::Write::write_all(&mut zip, data_json.as_bytes()).unwrap();
            zip.start_file("config.toml", opts).unwrap();
            std::io::Write::write_all(&mut zip, config_toml.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        let bytes = buf.into_inner();

        // Extract
        let (extracted_data, extracted_config, recovery_code) = extract_zip(&bytes).unwrap();
        assert_eq!(extracted_data.version, 4);
        assert!(extracted_config.is_some());
        assert!(recovery_code.is_none(), "no recovery.key in this zip");
    }

    /// A pre-0.8.9 (v3) export has no `quick_apis` / `learnings` keys at all.
    /// `#[serde(default)]` must keep it importable (empty vecs), never error.
    #[test]
    fn v3_export_without_new_fields_still_deserializes() {
        let v3_json = r#"{
            "version": 3,
            "exported_at": "2026-01-01T00:00:00Z",
            "projects": [],
            "discussions": []
        }"#;
        let parsed: DbExport = serde_json::from_str(v3_json).expect("v3 export must still parse");
        assert_eq!(parsed.version, 3);
        assert!(parsed.quick_apis.is_empty());
        assert!(parsed.learnings.is_empty());
        assert!(parsed.quick_prompts.is_empty());
    }

    // ─── Whole-DB export/import round-trip (0.8.9 — blinder l'export) ────────

    /// Tests mutating `KRONN_DATA_DIR` (process-wide) must serialize so they
    /// don't read back another test's config path. Mirrors the convention in
    /// `core::config` tests. `tokio::sync::Mutex` because it's held across
    /// `.await` (clippy `await_holding_lock` rejects a std mutex).
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn test_state() -> AppState {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let config_arc = std::sync::Arc::new(tokio::sync::RwLock::new(config::default_config()));
        AppState::new_defaults(config_arc, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    fn sample_quick_api(id: &str) -> QuickApi {
        QuickApi {
            id: id.into(),
            name: "Daily Top Articles".into(),
            icon: "🌐".into(),
            description: "Chartbeat top 5".into(),
            project_id: None,
            api_plugin_slug: "api-chartbeat".into(),
            api_config_id: "cfg-1".into(),
            api_endpoint_path: "/live/toppages/v3".into(),
            api_method: Some("GET".into()),
            api_query: None,
            api_path_params: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            variables: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn sample_learning(id: &str) -> Learning {
        Learning {
            id: id.into(),
            claim: "Gateway must be restarted after a frontend rebuild".into(),
            evidence: vec![Evidence {
                kind: "user".into(),
                reference: "2026-06-26".into(),
                quote: None,
            }],
            kind: LearningKind::Fact,
            status: LearningStatus::Validated,
            scope: Some(LearningScope::Project),
            confidence: Some(0.9),
            faithfulness: None,
            discussion_id: None,
            project_id: None,
            source_agent: Some("ClaudeCode".into()),
            promoted_target: None,
            created_at: Utc::now().to_rfc3339(),
            last_validated_at: None,
            validated_by: None,
        }
    }

    /// The whole point of 0.8.9: a Quick API and a continual-learning row must
    /// survive a full export → import cycle. Before the fix, `build_export`
    /// never collected them, so they vanished silently on migration.
    #[tokio::test]
    async fn export_import_roundtrips_quick_apis_and_learnings() {
        let state = test_state();

        let qa = sample_quick_api("qa-1");
        state
            .db
            .with_conn(move |conn| crate::db::quick_apis::insert_quick_api(conn, &qa))
            .await
            .unwrap();
        let l = sample_learning("l-1");
        state
            .db
            .with_conn(move |conn| crate::db::learnings::insert(conn, &l))
            .await
            .unwrap();

        let export = build_export(&state).await.expect("build_export");
        assert_eq!(
            export.version,
            crate::models::db::CURRENT_EXPORT_VERSION,
            "payload bumped to the current export version"
        );
        assert_eq!(export.quick_apis.len(), 1, "quick_apis must be exported");
        assert_eq!(export.learnings.len(), 1, "learnings must be exported");

        // Re-importing the same export clears then re-inserts — neither entity
        // may duplicate nor disappear.
        do_import_db(&state, &export).await.expect("do_import_db");
        let qas = state
            .db
            .with_conn(crate::db::quick_apis::list_quick_apis)
            .await
            .unwrap();
        let ls = state
            .db
            .with_conn(|conn| crate::db::learnings::list(conn, None, None))
            .await
            .unwrap();
        assert_eq!(qas.len(), 1, "quick_api survives import (no dup, no drop)");
        assert_eq!(qas[0].name, "Daily Top Articles");
        assert_eq!(ls.len(), 1, "learning survives import");
        assert_eq!(
            ls[0].claim,
            "Gateway must be restarted after a frontend rebuild"
        );
    }

    /// `global_context` travels in the exported config.toml but was dropped on
    /// import before 0.8.9. The merge must now re-apply it (+ its mode).
    #[tokio::test]
    #[serial]
    async fn import_reapplies_global_context() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "kronn-import-gctx-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());

        let state = test_state();
        let mut imported = config::default_config();
        imported.server.global_context = Some("Always answer in French.".into());
        imported.server.global_context_mode = "no_project".into(); // differs from "always" default

        let warnings = merge_import_config(&state, &imported).await;
        assert!(
            !warnings.iter().any(|w| w.contains("Failed to save")),
            "config save must succeed: {warnings:?}"
        );

        let cfg = state.config.read().await;
        assert_eq!(
            cfg.server.global_context.as_deref(),
            Some("Always answer in French.")
        );
        assert_eq!(cfg.server.global_context_mode, "no_project");
    }

    #[test]
    fn extract_zip_missing_data_json() {
        // Build a ZIP without data.json
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("other.txt", opts).unwrap();
            std::io::Write::write_all(&mut zip, b"hello").unwrap();
            zip.finish().unwrap();
        }
        let bytes = buf.into_inner();

        let result = extract_zip(&bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("data.json not found"));
    }

    #[test]
    fn extract_zip_without_config_toml() {
        let data = DbExport {
            version: 4,
            exported_at: Utc::now(),
            quick_prompt_versions: vec![],
            learning_rejections: vec![],
            projects: vec![],
            discussions: vec![],
            workflows: vec![],
            mcp_servers: vec![],
            mcp_configs: vec![],
            custom_skills: vec![],
            custom_directives: vec![],
            custom_profiles: vec![],
            contacts: vec![],
            quick_prompts: vec![],
            quick_apis: vec![],
            learnings: vec![],
        };
        let data_json = serde_json::to_string(&data).unwrap();

        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("data.json", opts).unwrap();
            std::io::Write::write_all(&mut zip, data_json.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        let bytes = buf.into_inner();

        let (extracted_data, extracted_config, _) = extract_zip(&bytes).unwrap();
        assert_eq!(extracted_data.version, 4);
        assert!(extracted_config.is_none(), "config.toml should be optional");
    }

    // ─── P2 — recovery blob bundled in export/import ───────────────────

    #[test]
    fn export_zip_bundles_and_roundtrips_the_recovery_code() {
        let key = crate::core::crypto::generate_secret();
        let blob = crate::core::recovery::wrap_key(&key, "passphrase-123").unwrap();
        let code = crate::core::recovery::to_code(&blob);

        let data_json = serde_json::to_string(&empty_export()).unwrap();
        let bytes = build_export_zip(&data_json, "", Some(&code)).unwrap();

        let (_, _, extracted) = extract_zip(&bytes).unwrap();
        assert_eq!(
            extracted.as_deref(),
            Some(code.as_str()),
            "recovery code must roundtrip"
        );
        // …and the roundtripped code still unwraps the key with the passphrase.
        let parsed = crate::core::recovery::from_code(extracted.as_deref().unwrap()).unwrap();
        assert_eq!(
            crate::core::recovery::unwrap_key(&parsed, "passphrase-123").unwrap(),
            key
        );
    }

    #[test]
    fn export_zip_without_recovery_carries_no_blob() {
        let data_json = serde_json::to_string(&empty_export()).unwrap();
        let bytes = build_export_zip(&data_json, "", None).unwrap();
        let (_, _, extracted) = extract_zip(&bytes).unwrap();
        assert!(extracted.is_none());
    }

    #[test]
    fn extract_zip_drops_a_corrupt_recovery_entry_without_failing_import() {
        let data_json = serde_json::to_string(&empty_export()).unwrap();
        let bytes = build_export_zip(&data_json, "", Some("not-a-valid-recovery-code")).unwrap();
        let (_, _, extracted) = extract_zip(&bytes).unwrap();
        assert!(
            extracted.is_none(),
            "garbage recovery.key must be dropped, not error"
        );
    }

    /// Import must NEVER destroy local recovery material: a differing local blob
    /// is kept as recovery.key.backup before the imported one is installed.
    #[test]
    fn persist_imported_recovery_backs_up_a_differing_local_blob() {
        let tmp = tempfile::tempdir().unwrap();

        // Local machine has its own blob (protects the LOCAL key).
        let local =
            crate::core::recovery::wrap_key(&crate::core::crypto::generate_secret(), "local-pass")
                .unwrap();
        crate::core::recovery::save_blob(tmp.path(), &local).unwrap();

        // Imported backup carries a different blob (the SOURCE machine's).
        let imported =
            crate::core::recovery::wrap_key(&crate::core::crypto::generate_secret(), "source-pass")
                .unwrap();
        let warnings =
            persist_imported_recovery(tmp.path(), &crate::core::recovery::to_code(&imported));

        assert!(
            !warnings.is_empty(),
            "replacing a local blob must be surfaced"
        );
        // The imported blob is now the active one…
        assert_eq!(
            crate::core::recovery::load_blob(tmp.path()).unwrap(),
            imported
        );
        // …and the local one survives as a backup.
        let backup = std::fs::read_to_string(tmp.path().join("recovery.key.backup")).unwrap();
        assert_eq!(
            crate::core::recovery::from_code(backup.trim()).unwrap(),
            local
        );
    }

    #[test]
    fn persist_imported_recovery_installs_when_no_local_blob() {
        let tmp = tempfile::tempdir().unwrap();

        let imported =
            crate::core::recovery::wrap_key(&crate::core::crypto::generate_secret(), "pp").unwrap();
        let warnings =
            persist_imported_recovery(tmp.path(), &crate::core::recovery::to_code(&imported));

        assert!(
            !warnings.is_empty(),
            "the user must be told how to use the installed blob"
        );
        assert_eq!(
            crate::core::recovery::load_blob(tmp.path()).unwrap(),
            imported
        );
        assert!(
            !tmp.path().join("recovery.key.backup").exists(),
            "no backup when nothing replaced"
        );
    }

    // ─── 0.8.6 phase 4 — default_model_tier ───────────────────────────

    #[test]
    fn default_config_seeds_default_model_tier_to_default_variant() {
        // Backwards-compat guard : the new field defaults to `Default`
        // (no behaviour change for users who never visit the new
        // dropdown). If a refactor moves the seed to `Reasoning` or
        // `Economy` by accident, every existing install would silently
        // start using a different tier on next disc create. Critical.
        let cfg = config::default_config();
        assert_eq!(
            cfg.server.default_model_tier,
            crate::models::ModelTier::Default
        );
    }

    #[test]
    fn server_config_default_model_tier_round_trips_through_toml() {
        // Serializing → deserializing → re-serializing must preserve
        // the chosen tier. The serde rename_all = "snake_case" on the
        // enum makes the wire format `economy` / `default` / `reasoning`
        // ; this test pins that contract.
        let mut cfg = config::default_config();
        cfg.server.default_model_tier = crate::models::ModelTier::Reasoning;
        let serialised = toml::to_string_pretty(&cfg).unwrap();
        assert!(
            serialised.contains("default_model_tier = \"reasoning\""),
            "expected snake_case 'reasoning' in TOML, got: {}",
            serialised,
        );
        let parsed: crate::models::AppConfig = toml::from_str(&serialised).unwrap();
        assert_eq!(
            parsed.server.default_model_tier,
            crate::models::ModelTier::Reasoning
        );
    }

    // ── 0.8.7 — anti_hallucination_mode ─────────────────────

    #[test]
    fn default_config_seeds_anti_hallucination_mode_to_warn() {
        // 0.8.7 ships in `warn` (visible but non-blocking). If a refactor
        // flips this to `off`, the whole feature silently stops on upgrade.
        let cfg = config::default_config();
        assert_eq!(cfg.server.anti_hallucination_mode, "warn");
    }

    #[test]
    fn anti_hallucination_mode_round_trips_through_toml() {
        let mut cfg = config::default_config();
        cfg.server.anti_hallucination_mode = "enforce".into();
        let serialised = toml::to_string_pretty(&cfg).unwrap();
        assert!(
            serialised.contains("anti_hallucination_mode = \"enforce\""),
            "expected mode in TOML, got: {}",
            serialised,
        );
        let parsed: crate::models::AppConfig = toml::from_str(&serialised).unwrap();
        assert_eq!(parsed.server.anti_hallucination_mode, "enforce");
    }

    #[test]
    fn spec_v1_constant_ships_with_binary() {
        // The Settings → Sourcing section links to this spec via
        // /api/conventions/agents-md-format-v1, which serves the constant
        // embedded at compile time. A regression that breaks the
        // include_str! path (file moved, renamed) would silently turn the
        // link into a 404 — fail the build instead.
        let spec = crate::core::anti_halluc::SPEC_AGENTS_MD_V1;
        assert!(spec.contains("Kronn `AGENTS.md` convention"));
        assert!(spec.contains("kronn:doc-version"));
        assert!(
            spec.len() > 1_000,
            "spec unexpectedly small: {} bytes",
            spec.len()
        );
    }

    #[test]
    fn spec_v1_repo_root_copy_matches_embedded_const() {
        // 0.8.7 — the spec lives at TWO locations in the repo:
        //   - `backend/docs/conventions/agents-md-format-v1.md` — the source
        //     `include_str!` reads at compile time + the Dockerfile COPYs.
        //   - `docs/conventions/agents-md-format-v1.md` — the repo-root copy
        //     so the link in `docs/AGENTS.md` § anti-hallu resolves both for
        //     Kronn itself AND for user projects that bootstrap the spec.
        //
        // The two MUST stay byte-identical. A `make sync-spec` target (or
        // the next bootstrap audit of Kronn itself) refreshes the root
        // copy from the embedded const. This test pins the invariant so a
        // PR that edits one without the other fails CI immediately.
        let root_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("backend has a parent")
            .join("docs/conventions/agents-md-format-v1.md");
        let root_copy = std::fs::read_to_string(&root_path)
            .expect("docs/conventions/agents-md-format-v1.md must exist at repo root");
        assert_eq!(
            root_copy,
            crate::core::anti_halluc::SPEC_AGENTS_MD_V1,
            "repo-root spec copy diverged from the embedded const — run `make sync-spec` or copy backend/docs/conventions/agents-md-format-v1.md to docs/conventions/agents-md-format-v1.md",
        );
    }

    #[test]
    fn missing_anti_hallucination_mode_in_toml_falls_back_to_warn() {
        // A pre-0.8.7 config.toml has no such key — serde(default) must
        // fill `warn` so the feature comes on after upgrade.
        let legacy_server_toml = r#"
host = "127.0.0.1"
port = 3140
max_concurrent_agents = 5
agent_stall_timeout_min = 5
global_context_mode = "always"
debug_mode = false
"#;
        let parsed: crate::models::ServerConfig = toml::from_str(legacy_server_toml).unwrap();
        assert_eq!(parsed.anti_hallucination_mode, "warn");
    }

    // ── 0.8.6 phase 4 — default_summary_strategy ─────────────

    #[test]
    fn default_config_seeds_default_summary_strategy_to_off() {
        // 0.8.6 phase 4 flipped the out-of-the-box default from `Auto`
        // (every disc auto-summarises after N msgs) to `Off`. Rationale :
        // modern agents have large context + MCP access to fetch older
        // history on demand, so auto-summary just burns Economy tokens.
        // Critical regression guard — a refactor that re-sets the seed
        // back to Auto would re-introduce the cost regression on every
        // new install.
        let cfg = config::default_config();
        assert_eq!(
            cfg.server.default_summary_strategy,
            crate::models::SummaryStrategy::Off,
            "0.8.6 baseline : auto-summary OFF out of the box. Re-seeding \
             to Auto regresses token cost on every new install.",
        );
    }

    #[test]
    fn server_config_default_summary_strategy_round_trips_through_toml() {
        let mut cfg = config::default_config();
        cfg.server.default_summary_strategy = crate::models::SummaryStrategy::Auto;
        let serialised = toml::to_string_pretty(&cfg).unwrap();
        assert!(
            serialised.contains("default_summary_strategy = \"Auto\""),
            "expected serialised summary strategy in TOML, got: {}",
            serialised,
        );
        let parsed: crate::models::AppConfig = toml::from_str(&serialised).unwrap();
        assert_eq!(
            parsed.server.default_summary_strategy,
            crate::models::SummaryStrategy::Auto
        );
    }

    #[test]
    fn missing_default_summary_strategy_in_toml_falls_back_to_off() {
        // Backwards-compat with PRE-0.8.6-phase-4 config.toml files.
        // serde must default to `Off` (new safer default) — NOT `Auto`
        // (the historical hardcoded value). If we ever silently bump
        // legacy users back to Auto, they'd suddenly pay for summaries
        // they didn't opt into. This pins the asymmetry.
        let legacy_server_toml = r#"
host = "127.0.0.1"
port = 3140
max_concurrent_agents = 5
agent_stall_timeout_min = 5
global_context_mode = "always"
debug_mode = false
"#;
        let parsed: crate::models::ServerConfig = toml::from_str(legacy_server_toml).unwrap();
        assert_eq!(
            parsed.default_summary_strategy,
            crate::models::SummaryStrategy::Off,
            "missing default_summary_strategy MUST serde-default to Off — \
             flipping legacy users to Auto silently would regress cost.",
        );
    }

    #[test]
    fn missing_default_model_tier_in_toml_falls_back_to_default_variant() {
        // Backwards-compat with config.toml files written BEFORE
        // 0.8.6 phase 4. Those don't have the field at all ; serde's
        // `#[serde(default)]` must kick in cleanly and seed `Default`.
        // If this test breaks, every legacy install will silently
        // re-default at restart. Scope kept to `ServerConfig` (not
        // the full `AppConfig`) so we don't have to mirror every
        // unrelated nested struct's required fields here.
        let legacy_server_toml = r#"
host = "127.0.0.1"
port = 3140
max_concurrent_agents = 5
agent_stall_timeout_min = 5
global_context_mode = "always"
debug_mode = false
"#;
        let parsed: crate::models::ServerConfig = toml::from_str(legacy_server_toml).unwrap();
        assert_eq!(
            parsed.default_model_tier,
            crate::models::ModelTier::Default,
            "missing default_model_tier must serde-default to Default",
        );
    }

    // ── mask_token contract ─────────────────────────────────────────────

    #[test]
    fn mask_token_short_token_fully_starred() {
        // ≤ 8 chars: every character becomes '*' (no info leak whatsoever).
        assert_eq!(mask_token(""), "");
        assert_eq!(mask_token("a"), "*");
        assert_eq!(mask_token("abc"), "***");
        assert_eq!(mask_token("12345678"), "********");
    }

    #[test]
    fn mask_token_long_token_keeps_first_and_last_four() {
        // > 8 chars: format is "first4...last4". Critical UI contract:
        // operators identify keys by the visible suffix.
        assert_eq!(mask_token("sk-test-key-abcdef1234"), "sk-t...1234");
        assert_eq!(mask_token("123456789"), "1234...6789");
    }

    #[test]
    fn mask_token_exactly_9_chars_is_long_branch() {
        // 9 = > 8, takes the long branch.
        let result = mask_token("123456789");
        assert!(
            result.contains("..."),
            "9-char token must enter long branch"
        );
        assert_eq!(result.len(), 11, "long-branch output is 4 + 3 + 4 = 11");
    }

    #[test]
    fn mask_token_does_not_leak_middle_chars() {
        let secret = "sk-ant-VERY_SECRET_MIDDLE-tail";
        let masked = mask_token(secret);
        assert!(
            !masked.contains("SECRET"),
            "middle bytes must NOT appear in mask"
        );
        assert!(
            !masked.contains("MIDDLE"),
            "middle bytes must NOT appear in mask"
        );
        assert!(masked.contains("tail"), "last 4 must be visible");
    }
}
