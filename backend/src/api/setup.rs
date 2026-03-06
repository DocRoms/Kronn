use axum::{extract::State, Json};
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

    let repos_detected = scanner::scan_paths(&scan_paths, &config.scan.ignore)
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
/// Returns token config with masked values
pub async fn get_tokens(
    State(state): State<AppState>,
) -> Json<ApiResponse<SaveTokensRequest>> {
    let config = state.config.read().await;
    Json(ApiResponse::ok(SaveTokensRequest {
        anthropic: config.tokens.anthropic.as_ref().map(|t| mask_token(t)),
        openai: config.tokens.openai.as_ref().map(|t| mask_token(t)),
    }))
}

/// POST /api/config/tokens
/// Save API tokens
pub async fn save_tokens(
    State(state): State<AppState>,
    Json(req): Json<SaveTokensRequest>,
) -> Json<ApiResponse<()>> {
    let mut config = state.config.write().await;

    // Only update non-empty, non-masked values
    if let Some(ref v) = req.anthropic {
        if !v.is_empty() && !v.contains('*') {
            config.tokens.anthropic = Some(v.clone());
        }
    }
    if let Some(ref v) = req.openai {
        if !v.is_empty() && !v.contains('*') {
            config.tokens.openai = Some(v.clone());
        }
    }
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
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
        _ => return Json(ApiResponse::err("Agent does not support access flags")),
    }
    match config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
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
            task_count: count("tasks"),
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
    let projects = match state.db.with_conn(|conn| crate::db::projects::list_projects(conn)).await {
        Ok(p) => p,
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let discussions = match state.db.with_conn(|conn| crate::db::discussions::list_discussions(conn)).await {
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
            "DELETE FROM messages; DELETE FROM discussions; DELETE FROM mcp_config_projects; DELETE FROM mcp_configs; DELETE FROM mcp_servers; DELETE FROM tasks; DELETE FROM projects;"
        )?;
        Ok(())
    }).await {
        return Json(ApiResponse::err(format!("Failed to clear DB: {}", e)));
    }

    // Import projects with their MCPs and tasks
    for project in &data.projects {
        let p = project.clone();
        if let Err(e) = state.db.with_conn(move |conn| crate::db::projects::insert_project(conn, &p)).await {
            return Json(ApiResponse::err(format!("Import project error: {}", e)));
        }
        let pid = project.id.clone();
        // MCPs are now managed via mcp_configs system, not per-project
        for task in &project.tasks {
            let t = task.clone();
            let id = pid.clone();
            let _ = state.db.with_conn(move |conn| crate::db::projects::insert_task(conn, &id, &t)).await;
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
            "DELETE FROM messages; DELETE FROM discussions; DELETE FROM mcp_config_projects; DELETE FROM mcp_configs; DELETE FROM mcp_servers; DELETE FROM tasks; DELETE FROM projects;"
        )?;
        Ok(())
    }).await;

    Json(ApiResponse::ok(()))
}
