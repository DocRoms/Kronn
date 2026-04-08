use axum::{
    body::Body,
    extract::{Multipart, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use crate::models::*;
use crate::core::{config, scanner};
use crate::agents;
use crate::AppState;

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
    directories::UserDirs::new()
        .map(|d| d.home_dir().to_string_lossy().to_string())
}

/// GET /api/setup/status
/// Returns current setup state with auto-detected repos.
/// Fast path: if config exists and scan_paths are set, skip the expensive
/// agent detection + filesystem scan — setup is already complete.
pub async fn get_status(
    State(state): State<AppState>,
) -> Json<ApiResponse<SetupStatus>> {
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
        let agents_detected = agents::detect_all().await;
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

    let has_wsl_paths = scan_paths.iter().any(|p| {
        p.starts_with(r"\\wsl.localhost\") || p.starts_with(r"\\wsl$\")
    });
    let scan_timeout = if has_wsl_paths { 15 } else { 5 };

    // Parallel: detect agents + scan repos simultaneously
    let (agents_detected, repos_result) = tokio::join!(
        agents::detect_all(),
        tokio::time::timeout(
            std::time::Duration::from_secs(scan_timeout),
            scanner::scan_paths_with_depth(&scan_paths, &scan_ignore, scan_depth),
        )
    );

    let repos_detected = repos_result
        .unwrap_or_else(|_| {
            tracing::warn!("Repo scan timed out after {}s — returning empty list", scan_timeout);
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
pub async fn get_scan_paths(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<String>>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(config.scan.paths.clone()))
}

/// GET /api/config/scan-ignore
/// Returns the configured scan ignore patterns
pub async fn get_scan_ignore(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<String>>> {
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
pub async fn install_agent(
    Json(agent_type): Json<AgentType>,
) -> Json<ApiResponse<String>> {
    match agents::install_agent(&agent_type).await {
        Ok(output) => Json(ApiResponse::ok(output)),
        Err(e) => Json(ApiResponse::err(format!("Install failed: {}", e))),
    }
}

/// POST /api/setup/complete
/// Mark setup as complete
pub async fn complete(
    State(state): State<AppState>,
) -> Json<ApiResponse<()>> {
    let config = state.config.read().await;
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to finalize: {}", e))),
    }
}

/// GET /api/config/tokens
/// Returns all API keys (masked) grouped by provider
pub async fn get_tokens(
    State(state): State<AppState>,
) -> Json<ApiResponse<ApiKeysResponse>> {
    let config = state.config.read().await;
    let keys = config.tokens.keys.iter().map(|k| ApiKeyDisplay {
        id: k.id.clone(),
        name: k.name.clone(),
        provider: k.provider.clone(),
        masked_value: mask_token(&k.value),
        active: k.active,
    }).collect();
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
        let is_first = !config.tokens.keys.iter().any(|k| k.provider == req.provider);
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
            if let Some(next) = config.tokens.keys.iter_mut().find(|k| k.provider == removed.provider) {
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

    let provider = config.tokens.keys.iter()
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
        sync_codex_auth(
            if is_now_enabled { config.tokens.active_key_for("openai") } else { None },
        );
    }
    if provider == "google" {
        crate::core::key_discovery::write_gemini_key(
            if is_now_enabled { config.tokens.active_key_for("google") } else { None },
        );
    }

    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(is_now_enabled)),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// GET /api/config/language
pub async fn get_language(
    State(state): State<AppState>,
) -> Json<ApiResponse<String>> {
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

/// GET /api/config/scan-depth
pub async fn get_scan_depth(
    State(state): State<AppState>,
) -> Json<ApiResponse<usize>> {
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
pub async fn get_agent_access(
    State(state): State<AppState>,
) -> Json<ApiResponse<AgentsConfig>> {
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
pub async fn get_model_tiers(
    State(state): State<AppState>,
) -> Json<ApiResponse<ModelTiersConfig>> {
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
    }))
}

/// POST /api/config/server
pub async fn set_server_config(
    State(state): State<AppState>,
    Json(req): Json<UpdateServerConfigRequest>,
) -> Json<ApiResponse<()>> {
    let mut config = state.config.write().await;
    if let Some(domain) = req.domain {
        config.server.domain = if domain.is_empty() { None } else { Some(domain) };
    }
    if let Some(max) = req.max_concurrent_agents {
        config.server.max_concurrent_agents = max.clamp(1, 20);
    }
    if let Some(timeout) = req.agent_stall_timeout_min {
        config.server.agent_stall_timeout_min = timeout.clamp(1, 60) as u32;
    }
    if let Some(pseudo) = req.pseudo {
        config.server.pseudo = if pseudo.is_empty() { None } else { Some(pseudo) };
    }
    if let Some(email) = req.avatar_email {
        config.server.avatar_email = if email.is_empty() { None } else { Some(email) };
    }
    if let Some(bio) = req.bio {
        config.server.bio = if bio.is_empty() { None } else { Some(bio) };
    }
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}

/// POST /api/config/auth-token/regenerate
pub async fn regenerate_auth_token(
    State(state): State<AppState>,
) -> Json<ApiResponse<String>> {
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
pub async fn get_auth_token(
    State(state): State<AppState>,
) -> Json<ApiResponse<Option<String>>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(config.server.auth_token.clone()))
}

/// Write or remove the OpenAI key from ~/.codex/auth.json
fn sync_codex_auth(key: Option<&str>) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let codex_dir = std::path::PathBuf::from(home).join(".codex");
    let codex_auth_path = codex_dir.join("auth.json");

    match key {
        Some(k) => {
            // Create .codex dir if needed
            let _ = std::fs::create_dir_all(&codex_dir);
            let content = serde_json::json!({
                "auth_mode": "apikey",
                "OPENAI_API_KEY": k,
            });
            // Safety: serializing a serde_json::Value literal cannot fail
            match std::fs::write(&codex_auth_path, serde_json::to_string_pretty(&content).expect("JSON Value serialization cannot fail")) {
                Ok(_) => tracing::info!("Synced OpenAI key to {}", codex_auth_path.display()),
                Err(e) => tracing::warn!("Failed to write {}: {}", codex_auth_path.display(), e),
            }
        }
        None => {
            // Delete the auth file so Codex falls back to its own login/subscription
            match std::fs::remove_file(&codex_auth_path) {
                Ok(_) => tracing::info!("Removed {} (Codex will use local auth)", codex_auth_path.display()),
                Err(e) => tracing::warn!("Failed to remove {}: {}", codex_auth_path.display(), e),
            }
        }
    }
}

/// POST /api/config/sync-agent-tokens
/// Write API tokens into agent-specific auth files (e.g. ~/.codex/auth.json)
pub async fn sync_agent_tokens(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<String>>> {
    let config = state.config.read().await;
    let mut synced: Vec<String> = Vec::new();

    // ── Codex: ~/.codex/auth.json ──
    if let Some(openai_key) = config.tokens.active_key_for("openai") {
        if !config.tokens.disabled_overrides.contains(&"openai".to_string()) {
            sync_codex_auth(Some(openai_key));
            synced.push("Codex".into());
        }
    }

    // ── Gemini CLI: ~/.gemini/settings.json ──
    if let Some(google_key) = config.tokens.active_key_for("google") {
        if !config.tokens.disabled_overrides.contains(&"google".to_string()) {
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
        let _ = config::save(&config).await;
        tracing::info!("Auto-imported {} API key(s)", imported_count);
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
    format!("{}...{}", &token[..4], &token[token.len()-4..])
}

/// GET /api/config/db-info
pub async fn db_info(
    State(state): State<AppState>,
) -> Json<ApiResponse<DbInfo>> {
    let size_bytes = std::fs::metadata(state.db.path())
        .map(|m| m.len())
        .unwrap_or(0);

    // Count custom skills/directives/profiles (file-based, not in DB)
    let custom_skill_count = crate::core::skills::list_all_skills()
        .iter().filter(|s| !s.is_builtin).count() as u32;
    let custom_profile_count = crate::core::profiles::list_all_profiles()
        .iter().filter(|p| !p.is_builtin).count() as u32;
    let custom_directive_count = crate::core::directives::list_all_directives()
        .iter().filter(|d| !d.is_builtin).count() as u32;

    match state.db.with_conn(move |conn| {
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
    }).await {
        Ok(info) => Json(ApiResponse::ok(info)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// Build the DbExport from current state
async fn build_export(state: &AppState) -> Result<DbExport, String> {
    let projects = state.db.with_conn(crate::db::projects::list_projects).await
        .map_err(|e| format!("DB error: {}", e))?;
    let discussions = state.db.with_conn(crate::db::discussions::list_discussions_with_messages).await
        .map_err(|e| format!("DB error: {}", e))?;
    let (workflows, mcp_servers, mcp_configs) = state.db.with_conn(|conn| {
        let wf = crate::db::workflows::list_workflows(conn)?;
        let servers = crate::db::mcps::list_servers(conn)?;
        let configs = crate::db::mcps::list_configs(conn)?;
        Ok((wf, servers, configs))
    }).await.map_err(|e| format!("DB error: {}", e))?;
    let contacts = state.db.with_conn(crate::db::contacts::list_contacts).await
        .map_err(|e| format!("DB error: {}", e))?;
    let quick_prompts = state.db.with_conn(crate::db::quick_prompts::list_quick_prompts).await
        .map_err(|e| format!("DB error: {}", e))?;

    let custom_skills: Vec<_> = crate::core::skills::list_all_skills()
        .into_iter().filter(|s| !s.is_builtin).collect();
    let custom_directives: Vec<_> = crate::core::directives::list_all_directives()
        .into_iter().filter(|d| !d.is_builtin).collect();
    let custom_profiles: Vec<_> = crate::core::profiles::list_all_profiles()
        .into_iter().filter(|p| !p.is_builtin).collect();

    Ok(DbExport {
        version: 3,
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
pub async fn export_data(
    State(state): State<AppState>,
) -> Response {
    let db_export = match build_export(&state).await {
        Ok(e) => e,
        Err(msg) => return (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response(),
    };

    let data_json = match serde_json::to_string_pretty(&db_export) {
        Ok(j) => j,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("JSON error: {}", e)).into_response(),
    };

    let config = state.config.read().await;
    let export_cfg = build_export_config(&config);
    let config_toml = match toml::to_string_pretty(&export_cfg) {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("TOML error: {}", e)).into_response(),
    };
    drop(config);

    // Build ZIP in memory
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        if let Err(e) = zip.start_file("data.json", options) {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("ZIP error: {}", e)).into_response();
        }
        if let Err(e) = std::io::Write::write_all(&mut zip, data_json.as_bytes()) {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("ZIP write error: {}", e)).into_response();
        }

        if let Err(e) = zip.start_file("config.toml", options) {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("ZIP error: {}", e)).into_response();
        }
        if let Err(e) = std::io::Write::write_all(&mut zip, config_toml.as_bytes()) {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("ZIP write error: {}", e)).into_response();
        }

        if let Err(e) = zip.finish() {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("ZIP finish error: {}", e)).into_response();
        }
    }

    let bytes = buf.into_inner();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"kronn-export.zip\"")
        .body(Body::from(bytes))
        .unwrap()
}

/// Import DB data from a DbExport struct. Returns warnings and invalid paths.
async fn do_import_db(state: &AppState, data: &DbExport) -> Result<ImportResult, String> {
    let mut warnings = Vec::new();
    let mut invalid_paths = Vec::new();

    // Clear existing data
    state.db.with_conn(|conn| {
        conn.execute_batch(
            "DELETE FROM messages; DELETE FROM discussions; \
             DELETE FROM mcp_config_projects; DELETE FROM mcp_configs; DELETE FROM mcp_servers; \
             DELETE FROM workflow_runs; DELETE FROM workflows; \
             DELETE FROM contacts; \
             DELETE FROM quick_prompts; \
             DELETE FROM projects;"
        )?;
        Ok(())
    }).await.map_err(|e| format!("Failed to clear DB: {}", e))?;

    // Import projects (check path validity)
    for project in &data.projects {
        let p = project.clone();
        let path = project.path.clone();
        if let Err(e) = state.db.with_conn(move |conn| crate::db::projects::insert_project(conn, &p)).await {
            tracing::warn!("Import project error: {}", e);
        }
        if !std::path::Path::new(&path).exists() {
            invalid_paths.push(path);
        }
    }

    // Import discussions with their messages
    for disc in &data.discussions {
        let d = disc.clone();
        if let Err(e) = state.db.with_conn(move |conn| crate::db::discussions::insert_discussion(conn, &d)).await {
            tracing::warn!("Import discussion error: {}", e);
        }
        let did = disc.id.clone();
        for msg in &disc.messages {
            let m = msg.clone();
            let id = did.clone();
            if let Err(e) = state.db.with_conn(move |conn| crate::db::discussions::insert_message(conn, &id, &m)).await {
                tracing::error!("Failed to import discussion message: {e}");
            }
        }
    }

    // Import MCP servers & configs
    for server in &data.mcp_servers {
        let s = server.clone();
        if let Err(e) = state.db.with_conn(move |conn| crate::db::mcps::upsert_server(conn, &s)).await {
            tracing::error!("Failed to import MCP server: {e}");
        }
    }
    for config_entry in &data.mcp_configs {
        let c = config_entry.clone();
        if let Err(e) = state.db.with_conn(move |conn| crate::db::mcps::insert_config(conn, &c)).await {
            tracing::error!("Failed to import MCP config: {e}");
        }
    }

    // Import workflows
    for wf in &data.workflows {
        let w = wf.clone();
        if let Err(e) = state.db.with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &w)).await {
            tracing::error!("Failed to import workflow: {e}");
        }
    }

    // Import custom skills/directives/profiles (file-based)
    for skill in &data.custom_skills {
        let _ = crate::core::skills::save_custom_skill(
            &skill.name, &skill.description, &skill.icon, &skill.category, &skill.content,
            skill.license.as_deref(), skill.allowed_tools.as_deref(),
        );
    }
    for directive in &data.custom_directives {
        let _ = crate::core::directives::save_custom_directive(
            &directive.name, &directive.description, &directive.icon, &directive.category,
            &directive.content, &directive.conflicts,
        );
    }
    for profile in &data.custom_profiles {
        let _ = crate::core::profiles::save_custom_profile(
            &crate::core::profiles::CustomProfileData {
                name: &profile.name,
                persona_name: &profile.persona_name,
                role: &profile.role,
                avatar: &profile.avatar,
                color: &profile.color,
                category: &profile.category,
                persona_prompt: &profile.persona_prompt,
                default_engine: profile.default_engine.as_deref(),
            }
        );
    }

    // Import contacts
    for contact in &data.contacts {
        let c = contact.clone();
        if let Err(e) = state.db.with_conn(move |conn| crate::db::contacts::insert_contact(conn, &c)).await {
            tracing::warn!("Import contact error: {}", e);
        }
    }

    // Import quick prompts
    for qp in &data.quick_prompts {
        let q = qp.clone();
        if let Err(e) = state.db.with_conn(move |conn| crate::db::quick_prompts::insert_quick_prompt(conn, &q)).await {
            tracing::warn!("Import quick prompt error: {}", e);
        }
    }

    if !invalid_paths.is_empty() {
        warnings.push(format!("{} project(s) have invalid paths — remap them in the Projects page", invalid_paths.len()));
    }

    Ok(ImportResult { warnings, invalid_paths })
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
    warnings.push("MCP secrets are encrypted with a different key — reconfigure them in the Plugins page".to_string());

    if let Err(e) = config::save(&config).await {
        tracing::error!("Failed to save merged config: {}", e);
        warnings.push(format!("Failed to save config: {}", e));
    }

    warnings
}

/// Extract data.json and config.toml from a ZIP file (synchronous, no await)
fn extract_zip(file_bytes: &[u8]) -> Result<(DbExport, Option<AppConfig>), String> {
    let cursor = std::io::Cursor::new(file_bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| format!("Invalid ZIP: {}", e))?;

    // Read data.json (required)
    let data: DbExport = {
        let mut f = archive.by_name("data.json")
            .map_err(|e| format!("data.json not found in ZIP: {}", e))?;
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut f, &mut contents)
            .map_err(|e| format!("Failed to read data.json: {}", e))?;
        serde_json::from_str(&contents)
            .map_err(|e| format!("Invalid data.json: {}", e))?
    };

    // Read config.toml (optional)
    let imported_config = if let Ok(mut f) = archive.by_name("config.toml") {
        let mut contents = String::new();
        if std::io::Read::read_to_string(&mut f, &mut contents).is_ok() {
            toml::from_str::<AppConfig>(&contents).ok()
        } else {
            None
        }
    } else {
        None
    };

    Ok((data, imported_config))
}

/// POST /api/config/import — accepts ZIP (multipart) or JSON (legacy)
pub async fn import_data(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Json<ApiResponse<ImportResult>> {
    // Read the uploaded file
    let file_bytes = match multipart.next_field().await {
        Ok(Some(field)) => {
            match field.bytes().await {
                Ok(b) => b,
                Err(e) => return Json(ApiResponse::err(format!("Failed to read upload: {}", e))),
            }
        }
        Ok(None) => return Json(ApiResponse::err("No file uploaded".to_string())),
        Err(e) => return Json(ApiResponse::err(format!("Multipart error: {}", e))),
    };

    // Detect format: ZIP (starts with PK\x03\x04) or JSON legacy
    let is_zip = file_bytes.len() >= 4 && file_bytes[0] == b'P' && file_bytes[1] == b'K'
        && file_bytes[2] == 0x03 && file_bytes[3] == 0x04;

    if is_zip {
        // Extract ZIP (sync — no await needed, avoids Send issues with zip reader)
        let (data, imported_config) = match extract_zip(&file_bytes) {
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
pub async fn reset(
    State(state): State<AppState>,
) -> Json<ApiResponse<()>> {
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
            version: 3,
            exported_at: Utc::now(),
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
        let (extracted_data, extracted_config) = extract_zip(&bytes).unwrap();
        assert_eq!(extracted_data.version, 3);
        assert!(extracted_config.is_some());
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
            version: 3,
            exported_at: Utc::now(),
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

        let (extracted_data, extracted_config) = extract_zip(&bytes).unwrap();
        assert_eq!(extracted_data.version, 3);
        assert!(extracted_config.is_none(), "config.toml should be optional");
    }
}
