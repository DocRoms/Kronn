//! Shared dynamic model catalog HTTP surface (KT-531).

use axum::{extract::State, Json};

use crate::core::model_catalog;
use crate::db::{external_api_connections, model_catalog as store};
use crate::models::{
    AgentType, ApiErrorCode, ApiResponse, DeleteManualModelRequest, ModelCatalogSnapshot,
    ModelCatalogView, RefreshModelCatalogRequest, UpsertManualModelRequest,
};
use crate::AppState;

fn snapshot_agents() -> [AgentType; 8] {
    [
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::OpenCode,
        AgentType::GeminiCli,
        AgentType::Kiro,
        AgentType::CopilotCli,
        AgentType::Vibe,
        AgentType::Ollama,
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
    // KT-1998 — the shared rule: a catalogue id can be assigned to a tier and
    // reach `--model`.
    crate::models::setup::check_model_id(&request.model_id)
        .map_err(|error| format!("model_id: {error}"))?;
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
    let config = state.config.read().await.clone();
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
    for agent_type in snapshot_agents() {
        let runtime_target_id = store::agent_runtime_target_id(&agent_type);
        let target_label = format!("{agent_type:?}");
        match model_catalog::build_view(&state.db, runtime_target_id, agent_type).await {
            Ok(mut view) => {
                view.target_label = Some(target_label);
                model_catalog::populate_tier_verdicts(&mut view, &config.agents.model_tiers);
                if let Err(error) =
                    model_catalog::populate_reference_alerts(&state.db, &config, &mut view).await
                {
                    return Json(ApiResponse::err(format!(
                        "Failed to inspect model catalog references: {error}"
                    )));
                }
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
                if let Err(error) =
                    model_catalog::populate_reference_alerts(&state.db, &config, &mut view).await
                {
                    return Json(ApiResponse::err(format!(
                        "Failed to inspect model catalog references: {error}"
                    )));
                }
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
    let refresh = if req.agent_type == AgentType::Ollama {
        let base =
            crate::api::ollama::resolve_base_url_pub(state.ollama_base_url_override.as_deref());
        model_catalog::refresh_ollama_catalog_at(&state.db, &base)
            .await
            .map(|(view, _)| view)
    } else {
        model_catalog::refresh_if_stale(&state.db, req.agent_type, req.force).await
    };
    match refresh {
        Ok(mut view) => {
            let config = state.config.read().await.clone();
            model_catalog::populate_tier_verdicts(&mut view, &config.agents.model_tiers);
            if let Err(error) =
                model_catalog::populate_reference_alerts(&state.db, &config, &mut view).await
            {
                return Json(ApiResponse::err(format!(
                    "Failed to inspect model catalog references: {error}"
                )));
            }
            Json(ApiResponse::ok(view))
        }
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

#[cfg(test)]
mod model_id_guard_tests {
    use super::*;
    use crate::models::AgentType;

    /// KT-1998 — a manual catalogue id that starts with a dash is refused
    /// and not stored: a catalogue id can be assigned to a tier.
    #[tokio::test]
    async fn model_id_guard_manual_catalogue_refuses_a_dash_led_model() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db,
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let target = store::agent_runtime_target_id(&AgentType::OpenCode);
        let before = state.db.with_conn(store::list_all).await.unwrap().len();
        let request: UpsertManualModelRequest = serde_json::from_value(serde_json::json!({
            "runtime_target_id": target,
            "agent_type": "OpenCode",
            "model_id": "--help",
            "display_name": "Help",
        }))
        .unwrap();
        let Json(refused) = create_manual(State(state.clone()), Json(request)).await;
        let error = refused.error.expect("a dash-led id is refused");
        assert!(error.contains("must not start with '-'"), "{error}");
        let after = state.db.with_conn(store::list_all).await.unwrap().len();
        assert_eq!(before, after, "nothing is written");
    }

    fn manual_request(model_id: &str) -> UpsertManualModelRequest {
        serde_json::from_value(serde_json::json!({
            "runtime_target_id": store::agent_runtime_target_id(&AgentType::OpenCode),
            "agent_type": "OpenCode",
            "model_id": model_id,
            "display_name": "Guarded",
        }))
        .unwrap()
    }

    /// KT-1998 — the manual catalogue uses the shared rule on create and on
    /// update: a dash-led id, line breaks or a tab around an id, and a long
    /// run of spaces are refused and nothing is written. Its limit is the
    /// shared one (256 characters), so an id it accepts can be put in a tier,
    /// a Quick Prompt or a discussion.
    #[tokio::test]
    async fn model_id_guard_manual_catalogue_update_and_shared_limit() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db,
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        // Written through the store directly: the handler's success path also
        // refreshes the process-wide runtime projection, which other tests read.
        let longest = "m".repeat(crate::models::setup::MAX_MODEL_ID_CHARS);
        validate_manual_request(&state, &manual_request(&longest))
            .await
            .expect("the shared limit is accepted by the catalogue");
        let saved = manual_request(&longest);
        state
            .db
            .with_conn(move |conn| store::create_manual(conn, &saved))
            .await
            .unwrap()
            .expect("stored");
        for chosen in [
            crate::models::setup::check_model_id(&longest),
            crate::models::setup::normalize_exact_model(&longest).map(|_| ()),
        ] {
            chosen.expect("a catalogue id is accepted by every other save");
        }
        let too_long = "m".repeat(crate::models::setup::MAX_MODEL_ID_CHARS + 1);
        let before = state.db.with_conn(store::list_all).await.unwrap();
        let padded = format!("{}gpt-5", " ".repeat(100_000));
        for bad in [
            "--help",
            " -c",
            "\ngpt-5\n",
            "\tgpt-5",
            padded.as_str(),
            too_long.as_str(),
        ] {
            let Json(refused) =
                update_manual(State(state.clone()), Json(manual_request(bad))).await;
            let error = refused.error.expect("update refuses");
            assert!(error.starts_with("model_id: model "), "{bad:?}: {error}");
            let Json(refused) =
                create_manual(State(state.clone()), Json(manual_request(bad))).await;
            assert!(refused.error.is_some(), "{bad:?}: create refuses");
            let refused = manual_request(bad);
            let stored = state
                .db
                .with_conn(move |conn| store::create_manual(conn, &refused))
                .await;
            assert!(stored.is_err(), "{bad:?}: the store refuses too");
        }
        let after = state.db.with_conn(store::list_all).await.unwrap();
        assert_eq!(before.len(), after.len(), "nothing is written");
    }
}
