//! KT-971 — where Kronn writes a project's agent files, and switching it.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::models::{AgentFilesPolicy, ApiErrorCode, ApiResponse};
use crate::AppState;

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ProjectAgentFiles {
    pub policy: AgentFilesPolicy,
    /// Where the files are when they are outside the repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub outside_dir: Option<String>,
    /// Repository files Kronn took its entries back out of during this change.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleaned: Vec<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetProjectAgentFiles {
    pub policy: AgentFilesPolicy,
}

fn view(project_path: &str, policy: AgentFilesPolicy, cleaned: Vec<String>) -> ProjectAgentFiles {
    ProjectAgentFiles {
        policy,
        outside_dir: (policy == AgentFilesPolicy::Outside)
            .then(|| crate::core::mcp_scanner::outside_agent_files_dir(project_path))
            .flatten()
            .map(|dir| dir.to_string_lossy().into_owned()),
        cleaned,
    }
}

/// GET /api/projects/{id}/agent-files
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<ProjectAgentFiles>> {
    let found = state
        .db
        .with_read_conn(move |conn| {
            let project = crate::db::projects::get_project(conn, &id)?;
            let policy = crate::db::projects::agent_files_policy(conn, &id)?;
            Ok(project.map(|project| (project.path, policy)))
        })
        .await;
    match found {
        Ok(Some((path, policy))) => Json(ApiResponse::ok(view(&path, policy, Vec::new()))),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Project not found",
        )),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

/// PUT /api/projects/{id}/agent-files — moving the files outside takes back
/// what Kronn wrote in the repository; moving them back in drops the outside
/// copy so Claude never reads a stale one. Either way the project is resynced.
pub async fn set(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SetProjectAgentFiles>,
) -> Json<ApiResponse<ProjectAgentFiles>> {
    let policy = request.policy;
    let lookup = id.clone();
    let path = match state
        .db
        .with_conn(move |conn| {
            let Some(project) = crate::db::projects::get_project(conn, &lookup)? else {
                return Ok(None);
            };
            crate::db::projects::set_agent_files_policy(conn, &lookup, policy)?;
            Ok(Some(project.path))
        })
        .await
    {
        Ok(Some(path)) => path,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Project not found",
            ))
        }
        Err(error) => return Json(ApiResponse::err(error.to_string())),
    };

    let cleaned = match policy {
        AgentFilesPolicy::Outside => {
            match crate::core::mcp_scanner::remove_kronn_agent_files(&path) {
                Ok(cleaned) => cleaned,
                Err(error) => return Json(ApiResponse::err(error)),
            }
        }
        AgentFilesPolicy::Repo => {
            if let Some(dir) = crate::core::mcp_scanner::outside_agent_files_dir(&path) {
                let _ = std::fs::remove_dir_all(dir);
            }
            Vec::new()
        }
    };
    super::resync_project_assets(&state, &id).await;
    Json(ApiResponse::ok(view(&path, policy, cleaned)))
}
