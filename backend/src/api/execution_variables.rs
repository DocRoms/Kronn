use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::{models::ApiResponse, AppState};

#[derive(Debug, Deserialize)]
pub struct PreviewRequest {
    pub project_id: Option<String>,
    pub variables: Vec<crate::models::PromptVariable>,
}

#[derive(Debug, Serialize)]
pub struct PreviewResponse {
    pub run_kind: String,
    pub run_id: String,
    pub metadata: crate::db::execution_variable_snapshots::SnapshotMetadata,
}

/// Resolve only project/context-provided variables for the launch form. The
/// encrypted preview is opaque, expires after ten minutes and is revealed one
/// value at a time through the same audited endpoint as a real execution.
/// The actual launch resolves again immediately before dispatch, so a project
/// environment change is never hidden by stale form state.
pub async fn preview(
    State(state): State<AppState>,
    Json(request): Json<PreviewRequest>,
) -> Json<ApiResponse<PreviewResponse>> {
    let variables: Vec<_> = request
        .variables
        .into_iter()
        .filter(|variable| !variable.requires_user_input())
        .collect();
    if variables.is_empty() {
        return Json(ApiResponse::err("No project-provided variable to preview"));
    }
    if let Some(error) = variables
        .iter()
        .find_map(|variable| variable.validate_source().err())
    {
        return Json(ApiResponse::err(error));
    }
    let secret = match state.config.read().await.encryption_secret.clone() {
        Some(secret) => secret,
        None => return Json(ApiResponse::err("Snapshot key unavailable")),
    };
    let run_id = uuid::Uuid::new_v4().to_string();
    let project_id = request.project_id;
    let prepared_run_id = run_id.clone();
    let prepared = state
        .db
        .with_conn(move |conn| {
            let prepared = crate::core::execution_variables::prepare(
                conn,
                crate::core::execution_variables::PrepareRequest {
                    declarations: &variables,
                    supplied: &std::collections::HashMap::new(),
                    context: &std::collections::HashMap::new(),
                    project_id: project_id.as_deref(),
                    discussion_id: None,
                    environment_ref: "project_mcp_configs",
                    run_kind: "preview",
                    run_id: &prepared_run_id,
                    encryption_secret: &secret,
                    retention_days: 1,
                },
            )?;
            let prepared = match prepared {
                Ok(prepared) => prepared,
                Err(failures) => return Ok(Err(failures)),
            };
            crate::db::execution_variable_snapshots::set_preview_expiry(
                conn,
                &prepared.snapshot_id,
                Utc::now() + Duration::minutes(10),
            )?;
            let metadata = crate::db::execution_variable_snapshots::metadata(
                conn,
                "preview",
                &prepared_run_id,
            )?
            .ok_or_else(|| anyhow::anyhow!("preview snapshot metadata missing"))?;
            Ok(Ok(metadata))
        })
        .await;
    match prepared {
        Ok(Ok(metadata)) => Json(ApiResponse::ok(PreviewResponse {
            run_kind: "preview".into(),
            run_id,
            metadata,
        })),
        Ok(Err(failures)) => Json(ApiResponse::err(format!(
            "preflight_failed:{}",
            serde_json::to_string(&failures).unwrap_or_default()
        ))),
        Err(error) => Json(ApiResponse::err(format!("Preview failed: {error}"))),
    }
}

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
    if run_kind == "preview" {
        return Json(ApiResponse::err("Launch previews cannot be extended"));
    }
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
