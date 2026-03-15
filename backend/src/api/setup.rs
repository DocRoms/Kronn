use axum::{extract::{Path, State}, Json};
use chrono::Utc;
use crate::models::*;
use crate::core::{config, scanner};
use crate::agents;
use crate::AppState;

/// Resolve the default scan path.
/// In Docker: use KRONN_HOST_HOME parent (= host's ~/.. = where repos live).
/// Otherwise: parent of current working dir.
fn default_scan_path() -> Option<String> {
    // In Docker, KRONN_HOST_HOME points to the mounted host home
    if let Ok(host_home) = std::env::var("KRONN_HOST_HOME") {
        return Some(host_home);
    }
    // Fallback: parent of cwd
    std::env::current_dir()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_string_lossy().to_string()))
}

/// GET /api/setup/status
/// Returns current setup state with auto-detected repos
pub async fn get_status(
    State(state): State<AppState>,
) -> Json<ApiResponse<SetupStatus>> {
    let is_first = config::is_first_run().await.unwrap_or(true);
    let agents_detected = agents::detect_all().await;

    let config = state.config.read().await;
    let scan_paths_set = !config.scan.paths.is_empty();

    // Auto-scan: use configured paths, or default scan path
    let scan_paths = if scan_paths_set {
        config.scan.paths.clone()
    } else {
        default_scan_path().into_iter().collect()
    };

    // Scan with a timeout to avoid blocking on slow filesystems (e.g. macOS Library via Docker)
    let repos_detected = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        scanner::scan_paths_with_depth(&scan_paths, &config.scan.ignore, config.scan.scan_depth),
    )
    .await
    .unwrap_or_else(|_| {
        tracing::warn!("Repo scan timed out after 10s — returning empty list");
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
        auth_enabled: config.server.auth_enabled && config.server.auth_token.is_some(),
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
            match std::fs::write(&codex_auth_path, serde_json::to_string_pretty(&content).unwrap()) {
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

/// GET /api/config/export
pub async fn export_data(
    State(state): State<AppState>,
) -> Json<ApiResponse<DbExport>> {
    let projects = match state.db.with_conn(crate::db::projects::list_projects).await {
        Ok(p) => p,
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let discussions = match state.db.with_conn(crate::db::discussions::list_discussions_with_messages).await {
        Ok(d) => d,
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let (workflows, mcp_servers, mcp_configs) = match state.db.with_conn(|conn| {
        let wf = crate::db::workflows::list_workflows(conn)?;
        let servers = crate::db::mcps::list_servers(conn)?;
        let configs = crate::db::mcps::list_configs(conn)?;
        Ok((wf, servers, configs))
    }).await {
        Ok(data) => data,
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    // Custom skills/directives/profiles (file-based)
    let custom_skills: Vec<_> = crate::core::skills::list_all_skills()
        .into_iter().filter(|s| !s.is_builtin).collect();
    let custom_directives: Vec<_> = crate::core::directives::list_all_directives()
        .into_iter().filter(|d| !d.is_builtin).collect();
    let custom_profiles: Vec<_> = crate::core::profiles::list_all_profiles()
        .into_iter().filter(|p| !p.is_builtin).collect();

    Json(ApiResponse::ok(DbExport {
        version: 2,
        exported_at: Utc::now(),
        projects,
        discussions,
        workflows,
        mcp_servers,
        mcp_configs,
        custom_skills,
        custom_directives,
        custom_profiles,
    }))
}

/// POST /api/config/import
pub async fn import_data(
    State(state): State<AppState>,
    Json(data): Json<DbExport>,
) -> Json<ApiResponse<()>> {
    // Clear existing data
    if let Err(e) = state.db.with_conn(|conn| {
        conn.execute_batch(
            "DELETE FROM messages; DELETE FROM discussions; \
             DELETE FROM mcp_config_projects; DELETE FROM mcp_configs; DELETE FROM mcp_servers; \
             DELETE FROM workflow_runs; DELETE FROM workflows; \
             DELETE FROM projects;"
        )?;
        Ok(())
    }).await {
        return Json(ApiResponse::err(format!("Failed to clear DB: {}", e)));
    }

    // Import projects
    for project in &data.projects {
        let p = project.clone();
        if let Err(e) = state.db.with_conn(move |conn| crate::db::projects::insert_project(conn, &p)).await {
            tracing::warn!("Import project error: {}", e);
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
            let _ = state.db.with_conn(move |conn| crate::db::discussions::insert_message(conn, &id, &m)).await;
        }
    }

    // Import MCP servers & configs
    for server in &data.mcp_servers {
        let s = server.clone();
        let _ = state.db.with_conn(move |conn| crate::db::mcps::upsert_server(conn, &s)).await;
    }
    for config in &data.mcp_configs {
        let c = config.clone();
        let _ = state.db.with_conn(move |conn| crate::db::mcps::insert_config(conn, &c)).await;
    }

    // Import workflows
    for wf in &data.workflows {
        let w = wf.clone();
        let _ = state.db.with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &w)).await;
    }

    // Import custom skills/directives/profiles (file-based)
    for skill in &data.custom_skills {
        let _ = crate::core::skills::save_custom_skill(
            &skill.name, &skill.description, &skill.icon, &skill.category, &skill.content,
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

    Json(ApiResponse::ok(()))
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
    let _ = state.db.with_conn(|conn| {
        conn.execute_batch(
            "DELETE FROM messages; DELETE FROM discussions; DELETE FROM mcp_config_projects; DELETE FROM mcp_configs; DELETE FROM mcp_servers; DELETE FROM projects;"
        )?;
        Ok(())
    }).await;

    Json(ApiResponse::ok(()))
}
