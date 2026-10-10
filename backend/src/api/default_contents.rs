//! KT-1030 — content Kronn ships by default: where it stands, and an explicit
//! reinstall. Absent from the bridge-token list: only a human reinstalls.

use axum::{extract::State, Json};

use crate::core::default_todo::{self, DefaultTodoStatus};
use crate::models::{ApiErrorCode, ApiResponse};
use crate::AppState;

/// GET /api/defaults/todo
pub async fn todo_status(State(state): State<AppState>) -> Json<ApiResponse<DefaultTodoStatus>> {
    match state.db.with_read_conn(default_todo::status).await {
        Ok(status) => Json(ApiResponse::ok(status)),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

/// POST /api/defaults/todo/install — a fresh copy of Kronn's board.
pub async fn install_todo(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
) -> Json<ApiResponse<DefaultTodoStatus>> {
    if bridge.is_some() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Only a human can reinstall Kronn's todo board",
        ));
    }
    let language = state.config.read().await.language.clone();
    match state
        .db
        .with_conn(move |conn| default_todo::reinstall(conn, &language))
        .await
    {
        Ok(status) => Json(ApiResponse::ok(status)),
        Err(error) if error.to_string().contains("already installed") => Json(
            ApiResponse::err_coded(ApiErrorCode::Conflict, error.to_string()),
        ),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}
