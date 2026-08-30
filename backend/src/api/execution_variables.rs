use axum::{extract::{Path, State}, Json};
use chrono::Utc;
use serde::Deserialize;

use crate::{models::ApiResponse, AppState};

pub async fn metadata(
    State(state): State<AppState>,
    Path((run_kind, run_id)): Path<(String, String)>,
) -> Json<ApiResponse<crate::db::execution_variable_snapshots::SnapshotMetadata>> {
    match state.db.with_conn(move |conn| {
        crate::db::execution_variable_snapshots::metadata(conn, &run_kind, &run_id)
    }).await {
        Ok(Some(value)) => Json(ApiResponse::ok(value)),
        Ok(None) => Json(ApiResponse::err("Execution variable snapshot not found")),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

#[derive(Deserialize)]
pub struct RevealRequest { pub variable: String }

pub async fn reveal(
    State(state): State<AppState>,
    Path((run_kind, run_id)): Path<(String, String)>,
    Json(request): Json<RevealRequest>,
) -> Json<ApiResponse<String>> {
    let secret = match state.config.read().await.encryption_secret.clone() {
        Some(secret) => secret,
        None => return Json(ApiResponse::err("Snapshot key unavailable")),
    };
    let key = match crate::core::crypto::parse_secret(&secret) {
        Ok(key) => key,
        Err(_) => return Json(ApiResponse::err("Snapshot key unavailable")),
    };
    let variable = request.variable;
    match state.db.with_conn(move |conn| {
        let Some(snapshot_id) = crate::db::execution_variable_snapshots::snapshot_id_for_run(conn, &run_kind, &run_id)? else {
            return Ok(None);
        };
        crate::db::execution_variable_snapshots::reveal(conn, &snapshot_id, &variable, "authenticated_ui", &key, Utc::now())
    }).await {
        Ok(Some(value)) => Json(ApiResponse::ok(value)),
        Ok(None) => Json(ApiResponse::err("Variable unavailable or expired")),
        Err(error) => Json(ApiResponse::err(format!("Reveal failed: {error}"))),
    }
}
