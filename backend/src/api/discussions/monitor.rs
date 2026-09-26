use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::{
    db::discussion_monitor,
    models::{ApiResponse, DiscussionMonitorItem},
    AppState,
};

#[derive(Default, Deserialize)]
pub struct MonitorQuery {
    #[serde(default)]
    ids: String,
}

/// GET-only, bounded to explicitly selected rooms. It never starts, resumes,
/// acknowledges, or cancels agent work, including when recovering a checkpoint.
pub async fn get(
    State(state): State<AppState>,
    Query(query): Query<MonitorQuery>,
) -> (StatusCode, Json<ApiResponse<Vec<DiscussionMonitorItem>>>) {
    let mut ids = Vec::new();
    if query.ids.len() > 4096 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err("Discussion selection is too large")),
        );
    }
    for id in query
        .ids
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        if ids.iter().any(|known| known == id) {
            continue;
        }
        if id.len() > 128 || ids.len() >= discussion_monitor::MAX_MONITOR_DISCUSSIONS {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::err(
                    "Select at most 12 discussions with valid ids",
                )),
            );
        }
        ids.push(id.to_owned());
    }
    if ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err("Select at least one discussion")),
        );
    }
    let result = state.db.with_read_conn(move |conn| {
        Ok(ids.into_iter().map(|id| {
            match discussion_monitor::preview(conn, &id) {
                Ok(Some(preview)) => DiscussionMonitorItem { id, preview: Some(preview), error: None },
                Ok(None) => DiscussionMonitorItem { id, preview: None, error: Some("not_found".into()) },
                Err(error) => {
                    tracing::warn!(discussion_id = %id, %error, "Discussion monitor read failed");
                    DiscussionMonitorItem { id, preview: None, error: Some("unavailable".into()) }
                }
            }
        }).collect())
    }).await;
    match result {
        Ok(items) => (StatusCode::OK, Json(ApiResponse::ok(items))),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(format!("DB error: {error}"))),
        ),
    }
}
