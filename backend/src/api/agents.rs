use axum::{Json, extract::State};
use crate::models::*;
use crate::agents;
use crate::AppState;

/// GET /api/agents
/// Detect all agents on the system, with enabled/disabled status from config.
/// Non-installed agents that are only runtime_available require a configured
/// API key (or env var) to be considered enabled — this prevents phantom agents
/// from appearing usable when they were never set up (fixes #6, #2, #10).
pub async fn detect(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<AgentDetection>>> {
    let mut detected = agents::detect_all().await;
    let config = state.config.read().await;
    for agent in &mut detected {
        // Apply explicit disable/enable from config
        agent.enabled = !config.disabled_agents.contains(&agent.agent_type);

        // If not installed locally and only runtime_available,
        // additionally require an API key to be considered usable
        if !agent.installed && agent.runtime_available && agent.enabled {
            let env_var = match agent.agent_type {
                AgentType::ClaudeCode => Some(("anthropic", "ANTHROPIC_API_KEY")),
                AgentType::Codex => Some(("openai", "OPENAI_API_KEY")),
                AgentType::GeminiCli => Some(("google", "GEMINI_API_KEY")),
                _ => None,
            };
            let has_key = env_var.map_or(false, |(provider, env)| {
                // Check multi-key system first
                config.tokens.active_key_for(provider).is_some()
                // Then check environment variable
                || std::env::var(env).is_ok()
            });
            if !has_key {
                agent.enabled = false;
            }
        }
    }
    Json(ApiResponse::ok(detected))
}

/// POST /api/agents/install
/// Install a specific agent, and auto-enable it in config
pub async fn install(
    State(state): State<AppState>,
    Json(agent_type): Json<AgentType>,
) -> Json<ApiResponse<String>> {
    match agents::install_agent(&agent_type).await {
        Ok(output) => {
            // Auto-enable after install: remove from disabled_agents if present
            let mut config = state.config.write().await;
            config.disabled_agents.retain(|a| a != &agent_type);
            let _ = crate::core::config::save(&config).await;
            Json(ApiResponse::ok(output))
        }
        Err(e) => Json(ApiResponse::err(format!("{}", e))),
    }
}

/// POST /api/agents/uninstall
/// Uninstall a specific agent, and auto-disable it in config so that
/// runtime_available (npx/uvx fallback) doesn't keep it appearing as usable
pub async fn uninstall(
    State(state): State<AppState>,
    Json(agent_type): Json<AgentType>,
) -> Json<ApiResponse<String>> {
    match agents::uninstall_agent(&agent_type).await {
        Ok(output) => {
            // Auto-disable after uninstall so runtime_available doesn't keep it "usable"
            let mut config = state.config.write().await;
            if !config.disabled_agents.contains(&agent_type) {
                config.disabled_agents.push(agent_type);
                let _ = crate::core::config::save(&config).await;
            }
            Json(ApiResponse::ok(output))
        }
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
