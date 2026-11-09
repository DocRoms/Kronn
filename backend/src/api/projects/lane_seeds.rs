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
//! token has the route. Kronn only *suggests* candidates, and a declared path
//! that looks like a secret file is refused until the human confirms it.
//!
//! The declaration lives in Kronn (the human's word) and is mirrored into the
//! `[lane_seeds]` table of the main checkout's `kronn/project.toml` when the
//! project has one; a project nobody declared seeds for in Kronn falls back to
//! the table its default branch carries.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::task_worker_lane_seeds::{
    self, LaneSeedCandidate, LaneSeedsRecord, LaneSeedsSource, SeedStatus,
};
use crate::models::{ApiErrorCode, ApiResponse, Project};
use crate::AppState;

/// What the last task worktree seeding did with one path.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct LaneSeedReport {
    pub path: String,
    /// `seeded`, `up_to_date` or `skipped`.
    pub outcome: String,
    /// Why a path was skipped, or what a seeded one left out.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detail: Option<String>,
}

/// The latest seeding of a task worktree of this project.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct LaneSeedsLastRun {
    pub execution_id: String,
    pub at: String,
    pub seeds: Vec<LaneSeedReport>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ProjectLaneSeeds {
    /// The paths seeded into each task worktree, whichever way they are declared.
    pub seeds: Vec<String>,
    pub source: LaneSeedsSource,
    /// What `kronn/project.toml` on the default branch declares, when it does.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub profile_seeds: Option<Vec<String>>,
    /// Why that file could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub profile_error: Option<String>,
    /// Declared paths that look like a secret file and were NOT saved: send them
    /// back in `confirm_secret_paths` once the human accepted them.
    pub needs_confirmation: Vec<String>,
    /// A save rewrote the working copy of `kronn/project.toml` (uncommitted).
    pub profile_file_written: bool,
    /// Why the working copy of `kronn/project.toml` could not be kept in step.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub profile_file_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_run: Option<LaneSeedsLastRun>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetProjectLaneSeeds {
    pub seeds: Vec<String>,
    /// Secret-looking paths of `seeds` the human confirmed.
    #[serde(default)]
    pub confirm_secret_paths: Vec<String>,
}

fn not_found<T: Serialize>() -> Json<ApiResponse<T>> {
    Json(ApiResponse::err_coded(
        ApiErrorCode::NotFound,
        "Project not found",
    ))
}

fn reports(record: &LaneSeedsRecord) -> Vec<LaneSeedReport> {
    record
        .seeds
        .iter()
        .map(|seed| {
            let (outcome, detail) = match &seed.status {
                SeedStatus::Seeded { dropped_links, .. } => (
                    "seeded",
                    (!dropped_links.is_empty()).then(|| {
                        format!(
                            "symlinks pointing out of it were left out: {}",
                            dropped_links.join(", ")
                        )
                    }),
                ),
                SeedStatus::UpToDate => ("up_to_date", None),
                SeedStatus::Skipped { reason } => ("skipped", Some(reason.clone())),
            };
            LaneSeedReport {
                path: seed.path.clone(),
                outcome: outcome.to_string(),
                detail,
            }
        })
        .collect()
}

/// What is read about a project's seeds, shared by every response.
async fn view(
    state: &AppState,
    project: &Project,
    needs_confirmation: Vec<String>,
    file_write: Result<bool, String>,
) -> Result<ProjectLaneSeeds, String> {
    let id = project.id.clone();
    let (declared, last_run) = state
        .db
        .with_read_conn(move |conn| {
            Ok((
                crate::db::projects::lane_seeds(conn, &id)?,
                crate::db::orchestration::latest_lane_seeds_for_project(conn, &id)?,
            ))
        })
        .await
        .map_err(|error| error.to_string())?;
    let repo = crate::core::scanner::resolve_host_path(&project.path);
    let profile = tokio::task::spawn_blocking(move || {
        let profile = crate::core::project_profile::declared_lane_seeds(&repo);
        let (seeds, source) = match &profile {
            Ok(found) => {
                task_worker_lane_seeds::effective_lane_seeds(declared, found.clone(), &repo)
            }
            Err(_) => task_worker_lane_seeds::effective_lane_seeds(declared, None, &repo),
        };
        (profile, seeds, source)
    })
    .await
    .map_err(|error| format!("the declaration could not be read: {error}"))?;
    let (profile, seeds, source) = profile;
    let (profile_seeds, profile_error) = match profile {
        Ok(found) => (found, None),
        Err(error) => (None, Some(error)),
    };
    let (profile_file_written, profile_file_error) = match file_write {
        Ok(written) => (written, None),
        Err(error) => (false, Some(error)),
    };
    Ok(ProjectLaneSeeds {
        seeds,
        source,
        profile_seeds,
        profile_error,
        needs_confirmation,
        profile_file_written,
        profile_file_error,
        last_run: last_run.map(|(execution_id, at, record)| LaneSeedsLastRun {
            execution_id,
            at,
            seeds: reports(&record),
        }),
    })
}

async fn project_of(state: &AppState, id: String) -> Result<Option<Project>, String> {
    state
        .db
        .with_read_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
        .map_err(|error| error.to_string())
}

/// GET /api/projects/{id}/lane-seeds
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<ProjectLaneSeeds>> {
    let project = match project_of(&state, id).await {
        Ok(Some(project)) => project,
        Ok(None) => return not_found(),
        Err(error) => return Json(ApiResponse::err(error)),
    };
    match view(&state, &project, Vec::new(), Ok(false)).await {
        Ok(seeds) => Json(ApiResponse::ok(seeds)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

/// PUT /api/projects/{id}/lane-seeds — replaces the list; an empty one clears it.
/// A path that looks like a secret file is not saved until the request names it
/// in `confirm_secret_paths`: the answer then lists it in `needs_confirmation`.
pub async fn set(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SetProjectLaneSeeds>,
) -> Json<ApiResponse<ProjectLaneSeeds>> {
    let seeds = match task_worker_lane_seeds::normalize_seed_list(&request.seeds) {
        Ok(seeds) => seeds,
        Err(reason) => return Json(ApiResponse::err(reason)),
    };
    let confirmed = match task_worker_lane_seeds::normalize_seed_list(&request.confirm_secret_paths)
    {
        Ok(confirmed) => confirmed,
        Err(reason) => return Json(ApiResponse::err(reason)),
    };
    let project = match project_of(&state, id).await {
        Ok(Some(project)) => project,
        Ok(None) => return not_found(),
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let repo = crate::core::scanner::resolve_host_path(&project.path);

    let to_check = (repo.clone(), seeds.clone());
    let secret = tokio::task::spawn_blocking(move || {
        task_worker_lane_seeds::secret_looking(&to_check.0, &to_check.1)
    })
    .await
    .unwrap_or_default();
    let unconfirmed: Vec<String> = secret
        .into_iter()
        .filter(|path| !confirmed.contains(path))
        .collect();
    if !unconfirmed.is_empty() {
        return match view(&state, &project, unconfirmed, Ok(false)).await {
            Ok(seeds) => Json(ApiResponse::ok(seeds)),
            Err(error) => Json(ApiResponse::err(error)),
        };
    }

    let (stored, project_id) = (seeds.clone(), project.id.clone());
    let changed = state
        .db
        .with_conn(move |conn| crate::db::projects::set_lane_seeds(conn, &project_id, &stored))
        .await;
    match changed {
        Ok(true) => {}
        Ok(false) => return not_found(),
        Err(error) => return Json(ApiResponse::err(error.to_string())),
    }
    let file_write = tokio::task::spawn_blocking(move || {
        crate::core::project_profile::write_lane_seeds(&repo, &seeds)
    })
    .await
    .unwrap_or_else(|error| Err(format!("the file edit did not complete: {error}")));
    match view(&state, &project, Vec::new(), file_write).await {
        Ok(seeds) => Json(ApiResponse::ok(seeds)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

/// GET /api/projects/{id}/lane-seeds/candidates — gitignored directories of
/// the main checkout that tracked code references, for the human to pick from.
pub async fn candidates(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<LaneSeedCandidate>>> {
    let project = match project_of(&state, id).await {
        Ok(Some(project)) => project,
        Ok(None) => return not_found(),
        Err(error) => return Json(ApiResponse::err(error)),
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
