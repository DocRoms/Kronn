use axum::{Json, extract::State};
use crate::models::*;
use crate::agents;
use crate::AppState;

/// GET /api/agents
/// Detect all agents on the system, with enabled/disabled status from config
pub async fn detect(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<AgentDetection>>> {
    let mut detected = agents::detect_all().await;
    let config = state.config.read().await;
    for agent in &mut detected {
        agent.enabled = !config.disabled_agents.contains(&agent.agent_type);
    }
    Json(ApiResponse::ok(detected))
}

/// POST /api/agents/install
/// Install a specific agent
pub async fn install(
    Json(agent_type): Json<AgentType>,
) -> Json<ApiResponse<String>> {
    match agents::install_agent(&agent_type).await {
        Ok(output) => Json(ApiResponse::ok(output)),
        Err(e) => Json(ApiResponse::err(format!("{}", e))),
    }
}

/// POST /api/agents/uninstall
/// Uninstall a specific agent
pub async fn uninstall(
    Json(agent_type): Json<AgentType>,
) -> Json<ApiResponse<String>> {
    match agents::uninstall_agent(&agent_type).await {
        Ok(output) => Json(ApiResponse::ok(output)),
        Err(e) => Json(ApiResponse::err(format!("{}", e))),
    }
}

/// POST /api/agents/toggle
/// Enable or disable an agent (without uninstalling)
pub async fn toggle(
    State(state): State<AppState>,
    Json(agent_type): Json<AgentType>,
) -> Json<ApiResponse<bool>> {
    let mut config = state.config.write().await;
    let was_disabled = config.disabled_agents.contains(&agent_type);
    if was_disabled {
        config.disabled_agents.retain(|a| a != &agent_type);
    } else {
        config.disabled_agents.push(agent_type);
    }
    let enabled = was_disabled; // toggled: if was disabled, now enabled
    match crate::core::config::save(&config).await {
        Ok(_) => Json(ApiResponse::ok(enabled)),
        Err(e) => Json(ApiResponse::err(format!("Failed to save: {}", e))),
    }
}
