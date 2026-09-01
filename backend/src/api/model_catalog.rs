//! Shared dynamic model catalog HTTP surface (KT-531).

use axum::{extract::State, Json};

use crate::core::model_catalog;
use crate::db::{external_api_connections, model_catalog as store};
use crate::models::{
    AgentType, ApiErrorCode, ApiResponse, DeleteManualModelRequest, ModelCatalogSnapshot,
    ModelCatalogView, RefreshModelCatalogRequest, UpsertManualModelRequest,
};
use crate::AppState;

fn cli_agents() -> [AgentType; 7] {
    [
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::OpenCode,
        AgentType::GeminiCli,
        AgentType::Kiro,
        AgentType::CopilotCli,
        AgentType::Vibe,
    ]
}

fn connection_agent(connection: &crate::models::ExternalApiConnection) -> AgentType {
    use crate::models::ExternalApiConnectionPreset;
    match connection.origin_preset {
        ExternalApiConnectionPreset::LiteLlm => AgentType::LiteLlm,
        ExternalApiConnectionPreset::Nvidia => AgentType::Nvidia,
        ExternalApiConnectionPreset::OpenRouter | ExternalApiConnectionPreset::Other => {
            AgentType::Custom
        }
    }
}

async fn validate_manual_request(
    state: &AppState,
    request: &UpsertManualModelRequest,
) -> Result<(), String> {
    store::validate_runtime_target_id(&request.runtime_target_id)
        .map_err(|error| error.to_string())?;
    if request.model_id.trim().is_empty()
        || request.model_id.len() > 256
        || request.model_id.chars().any(char::is_control)
    {
        return Err("model_id must contain 1 to 256 printable characters".into());
    }
    if request.display_name.trim().is_empty()
        || request.display_name.len() > 256
        || request.display_name.chars().any(char::is_control)
    {
        return Err("display_name must contain 1 to 256 printable characters".into());
    }
    if request.capabilities.len() > 16
        || request.reasoning_modes.len() > 16
        || request
            .capabilities
            .iter()
            .chain(&request.reasoning_modes)
            .any(|value| {
                value.trim().is_empty() || value.len() > 64 || value.chars().any(char::is_control)
            })
    {
        return Err(
            "capabilities and reasoning modes are limited to 16 printable values of 64 characters"
                .into(),
        );
    }

    if request.runtime_target_id.starts_with("agent:") {
        let expected = store::agent_runtime_target_id(&request.agent_type);
        if request.runtime_target_id != expected {
            return Err(format!(
                "runtime target {} does not project to {:?}",
                request.runtime_target_id, request.agent_type
            ));
        }
        return Ok(());
    }

    let connection_id = request
        .runtime_target_id
        .strip_prefix("http:")
        .ok_or_else(|| "invalid runtime target".to_string())?
        .to_string();
    let connection = state
        .db
        .with_read_conn(move |conn| external_api_connections::get(conn, &connection_id))
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "HTTP connection not found".to_string())?;
    let actual_agent = connection_agent(&connection);
    if actual_agent != request.agent_type {
        return Err(format!(
            "HTTP connection projects to {actual_agent:?}, not {:?}",
            request.agent_type
        ));
    }
    Ok(())
}

/// One snapshot consumed by every selector. Reads never pretend to be live:
/// freshness/provenance is returned with each target.
pub async fn list(State(state): State<AppState>) -> Json<ApiResponse<ModelCatalogSnapshot>> {
    let connections = match state
        .db
        .with_read_conn(external_api_connections::list)
        .await
    {
        Ok(value) => value,
        Err(error) => {
            return Json(ApiResponse::err(format!(
                "Failed to list model targets: {error}"
            )))
        }
    };
    let mut targets = Vec::new();
    for agent_type in cli_agents() {
        let runtime_target_id = store::agent_runtime_target_id(&agent_type);
        let target_label = format!("{agent_type:?}");
        match model_catalog::build_view(&state.db, runtime_target_id, agent_type).await {
            Ok(mut view) => {
                view.target_label = Some(target_label);
                targets.push(view);
            }
            Err(error) => {
                return Json(ApiResponse::err(format!(
                    "Failed to read model catalog: {error}"
                )))
            }
        }
    }
    for connection in connections {
        let runtime_target_id = store::http_runtime_target_id(&connection.id);
        match model_catalog::build_view(&state.db, runtime_target_id, connection_agent(&connection))
            .await
        {
            Ok(mut view) => {
                view.target_label = Some(connection.display_name.clone());
                targets.push(view);
            }
            Err(error) => {
                return Json(ApiResponse::err(format!(
                    "Failed to read model catalog: {error}"
                )))
            }
        }
    }
    Json(ApiResponse::ok(ModelCatalogSnapshot { targets }))
}

/// CLI targets actively discover; HTTP targets are refreshed by the existing
/// bounded connection test because it owns credential lookup and the codec.
pub async fn refresh(
    State(state): State<AppState>,
    Json(req): Json<RefreshModelCatalogRequest>,
) -> Json<ApiResponse<ModelCatalogView>> {
    let expected = store::agent_runtime_target_id(&req.agent_type);
    if req.runtime_target_id != expected || req.runtime_target_id.starts_with("http:") {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "HTTP catalogs are refreshed by testing their named connection",
        ));
    }
    match model_catalog::refresh_if_stale(&state.db, req.agent_type, req.force).await {
        Ok(view) => Json(ApiResponse::ok(view)),
        Err(error) => Json(ApiResponse::err(format!(
            "Failed to refresh model catalog: {error}"
        ))),
    }
}

pub async fn create_manual(
    State(state): State<AppState>,
    Json(req): Json<UpsertManualModelRequest>,
) -> Json<ApiResponse<crate::models::CatalogModelEntry>> {
    if let Err(error) = validate_manual_request(&state, &req).await {
        return Json(ApiResponse::err_coded(ApiErrorCode::Validation, error));
    }
    let request = req.clone();
    match state
        .db
        .with_conn(move |conn| store::create_manual(conn, &request))
        .await
    {
        Ok(Some(entry)) => {
            if let Err(error) = model_catalog::refresh_runtime_cache(&state.db).await {
                return Json(ApiResponse::err(format!(
                    "Model saved, but runtime cache refresh failed: {error}"
                )));
            }
            Json(ApiResponse::ok(entry))
        }
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            "A model with this target identity already exists",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            error.to_string(),
        )),
    }
}

pub async fn update_manual(
    State(state): State<AppState>,
    Json(req): Json<UpsertManualModelRequest>,
) -> Json<ApiResponse<crate::models::CatalogModelEntry>> {
    if let Err(error) = validate_manual_request(&state, &req).await {
        return Json(ApiResponse::err_coded(ApiErrorCode::Validation, error));
    }
    let target = req.runtime_target_id.clone();
    let model_id = req.model_id.clone();
    match state
        .db
        .with_conn(move |conn| store::update_manual(conn, &target, &model_id, &req))
        .await
    {
        Ok(Some(entry)) => {
            if let Err(error) = model_catalog::refresh_runtime_cache(&state.db).await {
                return Json(ApiResponse::err(format!(
                    "Model saved, but runtime cache refresh failed: {error}"
                )));
            }
            Json(ApiResponse::ok(entry))
        }
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Model not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            error.to_string(),
        )),
    }
}

pub async fn delete_manual(
    State(state): State<AppState>,
    Json(req): Json<DeleteManualModelRequest>,
) -> Json<ApiResponse<()>> {
    match state
        .db
        .with_conn(move |conn| store::delete_manual(conn, &req.runtime_target_id, &req.model_id))
        .await
    {
        Ok(true) => {
            if let Err(error) = model_catalog::refresh_runtime_cache(&state.db).await {
                return Json(ApiResponse::err(format!(
                    "Model deleted, but runtime cache refresh failed: {error}"
                )));
            }
            Json(ApiResponse::ok(()))
        }
        Ok(false) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Model not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            error.to_string(),
        )),
    }
}
