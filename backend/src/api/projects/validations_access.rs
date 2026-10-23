//! KT-1433 — how far a project's integration validations reach beyond the lane
//! sandbox.
//!
//! A Docker socket is the host's root and `full` drops the sandbox entirely, so
//! only a human widens it: the change is a destructive request
//! (`is_destructive`), and no bridge token has the route.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::models::{ApiErrorCode, ApiResponse, ValidationsAccess};
use crate::AppState;

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ProjectValidationsAccess {
    pub access: ValidationsAccess,
    /// The Docker socket a validation would reach on this host.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub docker_socket: Option<String>,
    /// Why no Docker socket is usable on this host.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub docker_error: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetProjectValidationsAccess {
    pub access: ValidationsAccess,
}

async fn view(access: ValidationsAccess) -> ProjectValidationsAccess {
    let socket = tokio::task::spawn_blocking(crate::core::task_worker_sandbox::host_docker_socket)
        .await
        .unwrap_or_else(|error| Err(error.to_string()));
    let (docker_socket, docker_error) = match socket {
        Ok(path) => (Some(path.to_string_lossy().into_owned()), None),
        Err(error) => (None, Some(error)),
    };
    ProjectValidationsAccess {
        access,
        docker_socket,
        docker_error,
    }
}

/// GET /api/projects/{id}/validations-access
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<ProjectValidationsAccess>> {
    let found = state
        .db
        .with_read_conn(move |conn| {
            let exists = crate::db::projects::get_project(conn, &id)?.is_some();
            let access = crate::db::projects::validations_access(conn, &id)?;
            Ok(exists.then_some(access))
        })
        .await;
    match found {
        Ok(Some(access)) => Json(ApiResponse::ok(view(access).await)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Project not found",
        )),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

/// PUT /api/projects/{id}/validations-access
pub async fn set(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SetProjectValidationsAccess>,
) -> Json<ApiResponse<ProjectValidationsAccess>> {
    let access = request.access;
    let changed = state
        .db
        .with_conn(move |conn| crate::db::projects::set_validations_access(conn, &id, access))
        .await;
    match changed {
        Ok(true) => Json(ApiResponse::ok(view(access).await)),
        Ok(false) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Project not found",
        )),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}
