use axum::Json;
use crate::models::*;
use crate::agents;

/// GET /api/agents
/// Detect all agents on the system
pub async fn detect() -> Json<ApiResponse<Vec<AgentDetection>>> {
    let detected = agents::detect_all().await;
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
