//! Discussion storage weight — backs the sidebar indicator and gives the
//! future cleanup something to decide on.
//!
//! Batch-only by design: summing message content scans the messages table, so
//! the caller passes the ids actually on screen. There is deliberately no
//! "give me everything" form.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;

use crate::db::discussion_weight as store;
use crate::models::{
    ApiResponse, DiscussionWeightView, DiscussionWeightsResponse, WeightThresholds,
};
use crate::AppState;

#[derive(Debug, Default, Deserialize)]
pub struct WeightsQuery {
    /// Comma-separated discussion ids. Required: without it there is nothing
    /// to bound the scan to.
    pub discussion_ids: Option<String>,
}

/// Thresholds in force, plus whether the stored pair was unusable. Reported
/// rather than hidden so a bad config stays visible.
async fn thresholds(state: &AppState) -> (WeightThresholds, bool) {
    state
        .config
        .read()
        .await
        .server
        .discussion_weight
        .effective_thresholds()
}

/// Weight of the requested discussions. Sparse: ids holding nothing are
/// absent from the map.
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<WeightsQuery>,
) -> Json<ApiResponse<DiscussionWeightsResponse>> {
    let raw = query.discussion_ids.unwrap_or_default();
    let ids: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    if ids.is_empty() {
        return Json(ApiResponse::err(
            "discussion_ids is required — this endpoint never scans every discussion".to_string(),
        ));
    }
    if ids.len() > store::MAX_BATCH_IDS {
        return Json(ApiResponse::err(format!(
            "too many discussion_ids: {} requested, {} max",
            ids.len(),
            store::MAX_BATCH_IDS
        )));
    }

    let lookup = ids.clone();
    let weights = match state
        .db
        .with_read_conn(move |conn| store::for_ids(conn, &lookup))
        .await
    {
        Ok(weights) => weights,
        Err(e) => return Json(ApiResponse::err(format!("Failed to compute weights: {e}"))),
    };

    let (thresholds, from_defaults) = thresholds(&state).await;
    let weights = weights
        .into_iter()
        .map(|(id, weight)| (id, DiscussionWeightView::of(weight, &thresholds)))
        .collect();

    Json(ApiResponse::ok(DiscussionWeightsResponse {
        weights,
        thresholds,
        thresholds_from_defaults: from_defaults,
    }))
}

/// Weight of one discussion. An empty discussion answers with zeros rather
/// than a 404, since the caller named it explicitly.
pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<DiscussionWeightView>> {
    let lookup = id.clone();
    let weight = match state
        .db
        .with_read_conn(move |conn| store::one(conn, &lookup))
        .await
    {
        Ok(weight) => weight,
        Err(e) => return Json(ApiResponse::err(format!("Failed to compute weight: {e}"))),
    };

    let (thresholds, _) = thresholds(&state).await;
    Json(ApiResponse::ok(DiscussionWeightView::of(
        weight,
        &thresholds,
    )))
}
