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

    let repos_detected = scanner::scan_paths_with_depth(&scan_paths, &config.scan.ignore, config.scan.scan_depth)
        .await
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

    // Future: Gemini CLI, Claude Code, etc. can be added here

    Json(ApiResponse::ok(synced))
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

    let discussions = match state.db.with_conn(crate::db::discussions::list_discussions).await {
        Ok(d) => d,
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    Json(ApiResponse::ok(DbExport {
        version: 1,
        exported_at: Utc::now(),
        projects,
        discussions,
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
            "DELETE FROM messages; DELETE FROM discussions; DELETE FROM mcp_config_projects; DELETE FROM mcp_configs; DELETE FROM mcp_servers; DELETE FROM projects;"
        )?;
        Ok(())
    }).await {
        return Json(ApiResponse::err(format!("Failed to clear DB: {}", e)));
    }

    // Import projects
    for project in &data.projects {
        let p = project.clone();
        if let Err(e) = state.db.with_conn(move |conn| crate::db::projects::insert_project(conn, &p)).await {
            return Json(ApiResponse::err(format!("Import project error: {}", e)));
        }
    }

    // Import discussions with their messages
    for disc in &data.discussions {
        let d = disc.clone();
        if let Err(e) = state.db.with_conn(move |conn| crate::db::discussions::insert_discussion(conn, &d)).await {
            return Json(ApiResponse::err(format!("Import discussion error: {}", e)));
        }
        let did = disc.id.clone();
        for msg in &disc.messages {
            let m = msg.clone();
            let id = did.clone();
            let _ = state.db.with_conn(move |conn| crate::db::discussions::insert_message(conn, &id, &m)).await;
        }
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
