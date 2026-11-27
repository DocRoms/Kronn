//! KT-1518 / KT-1693 — a discussion's own fallback, per mode.
//!
//! Without an override the fallback set on the row of the agent a turn starts on
//! (for the mode the turn runs in) applies. An override replaces it for one mode
//! of this discussion only; an empty one is "no fallback here". Both are read
//! when a turn fails or starts, so a change applies to the next turn, no restart.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::agent_fallback as fallback;
use crate::models::{AgentFallback, ApiResponse, FallbackMode};
use crate::AppState;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscussionFallback {
    /// The modes this discussion overrides. A mode that is absent follows the
    /// default of the agent the turn starts on; an empty chain is "no fallback".
    pub overrides: Vec<fallback::ModeOverride>,
    /// Each override as the editor shows it (gaps, inherited links, refusal).
    pub views: Vec<fallback::ChainView>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetDiscussionFallbackRequest {
    /// The mode whose override is set.
    pub mode: FallbackMode,
    /// `None` removes the override (back to the default); `Some([])` cuts it.
    pub chain_override: Option<Vec<AgentFallback>>,
}

fn present(
    agents: &crate::models::AgentsConfig,
    overrides: Vec<fallback::ModeOverride>,
) -> DiscussionFallback {
    let views = overrides
        .iter()
        .map(|entry| fallback::chain_view_of(agents, None, entry.mode, &entry.chain))
        .collect();
    DiscussionFallback { overrides, views }
}

/// GET /api/discussions/{id}/fallback
pub async fn get_discussion_fallback(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<DiscussionFallback>> {
    let agents = state.config.read().await.agents.clone();
    match state
        .db
        .with_read_conn(move |conn| fallback::get_discussion_override(conn, &id))
        .await
    {
        Ok(overrides) => Json(ApiResponse::ok(present(
            &agents,
            overrides.unwrap_or_default(),
        ))),
        Err(error) => Json(ApiResponse::err(format!("{error}"))),
    }
}

/// PUT /api/discussions/{id}/fallback
pub async fn set_discussion_fallback(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SetDiscussionFallbackRequest>,
) -> Json<ApiResponse<DiscussionFallback>> {
    let agents = state.config.read().await.agents.clone();
    // KT-1998 — each model is judged as sent, then stored trimmed.
    let chain_override: Option<Vec<AgentFallback>> = match req
        .chain_override
        .map(|chain| {
            chain
                .into_iter()
                .map(|mut link| {
                    link.model = crate::models::setup::clean_optional_model(link.model)?;
                    Ok(link)
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .transpose()
    {
        Ok(chain) => chain,
        Err(why) => {
            return Json(ApiResponse::err(
                fallback::ChainError::BadModel(why).to_string(),
            ))
        }
    };
    if let Some(chain) = &chain_override {
        if let Err(error) = fallback::validate_chain(&agents, None, Some(req.mode), chain) {
            return Json(ApiResponse::err(error.to_string()));
        }
    }
    let lookup = id.clone();
    let mode = req.mode;
    let saved = state
        .db
        .with_conn(move |conn| {
            anyhow::ensure!(
                crate::db::discussions::get_discussion(conn, &lookup)?.is_some(),
                "discussion not found"
            );
            fallback::set_discussion_override(conn, &lookup, mode, chain_override.as_deref())?;
            Ok(fallback::get_discussion_override(conn, &lookup)?.unwrap_or_default())
        })
        .await;
    match saved {
        Ok(overrides) => Json(ApiResponse::ok(present(&agents, overrides))),
        Err(error) => Json(ApiResponse::err(format!("{error}"))),
    }
}
