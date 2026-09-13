//! KT-619 — read side of the important cards.
//!
//! Read-only on purpose: publication happens through a `kronn-important` fence
//! on an append, where the caller's own room is known. There is no route here
//! that takes a message id, so none can convert an ordinary message.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use ts_rs::TS;

use crate::db::discussion_important::{self, ImportantCategory, ImportantMessageList};
use crate::models::{ApiErrorCode, ApiResponse};
use crate::AppState;

#[derive(Debug, Clone, Default, Deserialize, TS)]
#[ts(export)]
pub struct ImportantQuery {
    /// One closed category, or absent for every card.
    #[serde(default)]
    pub category: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Path(discussion_id): Path<String>,
    Query(query): Query<ImportantQuery>,
) -> (StatusCode, Json<ApiResponse<ImportantMessageList>>) {
    // An unknown category is a caller error, not "everything": silently
    // widening the filter would show cards the user asked to hide.
    let category = match query.category.as_deref() {
        None | Some("") => None,
        Some(name) => match ImportantCategory::parse(name) {
            Some(category) => Some(category),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse::err_coded(
                        ApiErrorCode::Validation,
                        format!("Unknown important category: {name}"),
                    )),
                )
            }
        },
    };

    let result = state
        .db
        .with_read_conn(move |conn| {
            // Polled alongside the transcript: existence must not load messages.
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM discussions WHERE id=?1)",
                [&discussion_id],
                |row| row.get(0),
            )?;
            if !exists {
                return Ok(None);
            }
            discussion_important::list(conn, &discussion_id, category).map(Some)
        })
        .await;

    match result {
        Ok(Some(list)) => (StatusCode::OK, Json(ApiResponse::ok(list))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Discussion not found",
            )),
        ),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(error.to_string())),
        ),
    }
}
