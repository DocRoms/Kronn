//! KT-1518 — a discussion's own fallback chain.
//!
//! Without an override the default chain of the agent a turn starts on applies.
//! An override replaces it for this discussion only; an empty one is "no
//! fallback here". Both are read when a turn fails or starts, so a change
//! applies to the next turn, no restart.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::agent_fallback as fallback;
use crate::models::{AgentFallback, ApiResponse};
use crate::AppState;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscussionFallback {
    /// `None`: the agent's default chain applies; `Some([])`: no fallback.
    #[ts(optional)]
    pub chain_override: Option<Vec<AgentFallback>>,
    /// The override as the editor shows it (gaps, refusal), when there is one.
    #[ts(optional)]
    pub view: Option<fallback::ChainView>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetDiscussionFallbackRequest {
    /// `None` removes the override (back to the default chain).
    pub chain_override: Option<Vec<AgentFallback>>,
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
        Ok(chain_override) => Json(ApiResponse::ok(DiscussionFallback {
            view: chain_override
                .as_deref()
                .map(|chain| fallback::chain_view_of(&agents, None, chain)),
            chain_override,
        })),
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
    let chain_override: Option<Vec<AgentFallback>> = req.chain_override.map(|chain| {
        chain
            .into_iter()
            .map(|mut link| {
                link.model = link
                    .model
                    .map(|model| model.trim().to_string())
                    .filter(|model| !model.is_empty());
                link
            })
            .collect()
    });
    if let Some(chain) = &chain_override {
        if let Err(error) = fallback::validate_chain(&agents, None, chain) {
            return Json(ApiResponse::err(error.to_string()));
        }
    }
    let stored = chain_override.clone();
    let lookup = id.clone();
    let saved = state
        .db
        .with_conn(move |conn| {
            anyhow::ensure!(
                crate::db::discussions::get_discussion(conn, &lookup)?.is_some(),
                "discussion not found"
            );
            fallback::set_discussion_override(conn, &lookup, stored.as_deref())
        })
        .await;
    match saved {
        Ok(()) => Json(ApiResponse::ok(DiscussionFallback {
            view: chain_override
                .as_deref()
                .map(|chain| fallback::chain_view_of(&agents, None, chain)),
            chain_override,
        })),
        Err(error) => Json(ApiResponse::err(format!("{error}"))),
    }
}
