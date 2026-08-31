use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use serde::Deserialize;

use crate::{models::ApiResponse, AppState};

pub async fn metadata(
    State(state): State<AppState>,
    Path((run_kind, run_id)): Path<(String, String)>,
) -> Json<ApiResponse<crate::db::execution_variable_snapshots::SnapshotMetadata>> {
    match state
        .db
        .with_conn(move |conn| {
            if !crate::db::execution_variable_snapshots::has_live_owner(conn, &run_kind, &run_id)? {
                return Ok(None);
            }
            crate::db::execution_variable_snapshots::metadata(conn, &run_kind, &run_id)
        })
        .await
    {
        Ok(Some(value)) => Json(ApiResponse::ok(value)),
        Ok(None) => Json(ApiResponse::err("Execution variable snapshot not found")),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

#[derive(Deserialize)]
pub struct RevealRequest {
    pub variable: String,
}

pub async fn reveal(
    State(state): State<AppState>,
    Path((run_kind, run_id)): Path<(String, String)>,
    Json(request): Json<RevealRequest>,
) -> Json<ApiResponse<String>> {
    let (secret, actor) = {
        let config = state.config.read().await;
        let Some(secret) = config.encryption_secret.clone() else {
            return Json(ApiResponse::err("Snapshot key unavailable"));
        };
        (
            secret,
            config
                .server
                .pseudo
                .clone()
                .unwrap_or_else(|| "local_operator".into()),
        )
    };
    let key = match crate::core::crypto::parse_secret(&secret) {
        Ok(key) => key,
        Err(_) => return Json(ApiResponse::err("Snapshot key unavailable")),
    };
    let variable = request.variable;
    match state
        .db
        .with_conn(move |conn| {
            if !crate::db::execution_variable_snapshots::has_live_owner(conn, &run_kind, &run_id)? {
                return Ok(None);
            }
            let Some(snapshot_id) = crate::db::execution_variable_snapshots::snapshot_id_for_run(
                conn, &run_kind, &run_id,
            )?
            else {
                return Ok(None);
            };
            crate::db::execution_variable_snapshots::reveal(
                conn,
                &snapshot_id,
                &variable,
                &actor,
                &key,
                Utc::now(),
            )
        })
        .await
    {
        Ok(Some(value)) => Json(ApiResponse::ok(value)),
        Ok(None) => Json(ApiResponse::err("Variable unavailable or expired")),
        Err(error) => Json(ApiResponse::err(format!("Reveal failed: {error}"))),
    }
}

#[derive(Deserialize)]
pub struct ExtendRequest {
    pub days: u32,
}

pub async fn extend(
    State(state): State<AppState>,
    Path((run_kind, run_id)): Path<(String, String)>,
    Json(request): Json<ExtendRequest>,
) -> Json<ApiResponse<()>> {
    if request.days == 0 {
        return Json(ApiResponse::err(
            "Retention extension must be at least one day",
        ));
    }
    let actor = state
        .config
        .read()
        .await
        .server
        .pseudo
        .clone()
        .unwrap_or_else(|| "local_operator".into());
    match state
        .db
        .with_conn(move |conn| {
            if !crate::db::execution_variable_snapshots::has_live_owner(conn, &run_kind, &run_id)? {
                return Ok(false);
            }
            let Some(snapshot_id) = crate::db::execution_variable_snapshots::snapshot_id_for_run(
                conn, &run_kind, &run_id,
            )?
            else {
                return Ok(false);
            };
            crate::db::execution_variable_snapshots::extend_retention(
                conn,
                &snapshot_id,
                request.days,
                &actor,
                Utc::now(),
            )
        })
        .await
    {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err(
            "Execution variable snapshot not found or unauthorized",
        )),
        Err(error) => Json(ApiResponse::err(format!(
            "Retention extension failed: {error}"
        ))),
    }
}
