//! KT-1438 — the gitignored paths a project's task worktrees are seeded with.
//!
//! A fresh task worktree holds the tracked tree only, so a generated asset
//! directory the code needs (`public/images`, `public/build`) is missing there.
//! A project declares those paths; Kronn copies them from the main checkout
//! into each worktree (`core::task_worker_lane_seeds`).
//!
//! A seed copies files from the main checkout to where a worker can read them
//! (a gitignored `.env` is a gitignored path too), so only a human declares
//! them: the change is a destructive request (`is_destructive`), and no bridge
//! token has the route. Kronn only *suggests* candidates.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::task_worker_lane_seeds::{self, LaneSeedCandidate};
use crate::models::{ApiErrorCode, ApiResponse};
use crate::AppState;

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ProjectLaneSeeds {
    /// Repository-relative gitignored paths copied into each task worktree.
    pub seeds: Vec<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetProjectLaneSeeds {
    pub seeds: Vec<String>,
}

fn not_found<T: Serialize>() -> Json<ApiResponse<T>> {
    Json(ApiResponse::err_coded(
        ApiErrorCode::NotFound,
        "Project not found",
    ))
}

/// GET /api/projects/{id}/lane-seeds
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<ProjectLaneSeeds>> {
    let found = state
        .db
        .with_read_conn(move |conn| {
            let exists = crate::db::projects::get_project(conn, &id)?.is_some();
            let seeds = crate::db::projects::lane_seeds(conn, &id)?;
            Ok(exists.then_some(seeds))
        })
        .await;
    match found {
        Ok(Some(seeds)) => Json(ApiResponse::ok(ProjectLaneSeeds { seeds })),
        Ok(None) => not_found(),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

/// PUT /api/projects/{id}/lane-seeds — replaces the list; an empty one clears it.
pub async fn set(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SetProjectLaneSeeds>,
) -> Json<ApiResponse<ProjectLaneSeeds>> {
    let seeds = match task_worker_lane_seeds::normalize_seed_list(&request.seeds) {
        Ok(seeds) => seeds,
        Err(reason) => return Json(ApiResponse::err(reason)),
    };
    let stored = seeds.clone();
    let changed = state
        .db
        .with_conn(move |conn| crate::db::projects::set_lane_seeds(conn, &id, &stored))
        .await;
    match changed {
        Ok(true) => Json(ApiResponse::ok(ProjectLaneSeeds { seeds })),
        Ok(false) => not_found(),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

/// GET /api/projects/{id}/lane-seeds/candidates — gitignored directories of
/// the main checkout that tracked code references, for the human to pick from.
pub async fn candidates(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<LaneSeedCandidate>>> {
    let project = state
        .db
        .with_read_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await;
    let project = match project {
        Ok(Some(project)) => project,
        Ok(None) => return not_found(),
        Err(error) => return Json(ApiResponse::err(error.to_string())),
    };
    let repo = crate::core::scanner::resolve_host_path(&project.path);
    match tokio::task::spawn_blocking(move || task_worker_lane_seeds::suggest_seeds(&repo)).await {
        Ok(Ok(found)) => Json(ApiResponse::ok(found)),
        Ok(Err(reason)) => Json(ApiResponse::err(reason)),
        Err(error) => Json(ApiResponse::err(format!(
            "the candidate scan did not complete: {error}"
        ))),
    }
}

#[cfg(test)]
#[path = "lane_seeds_test.rs"]
mod lane_seeds_test;
