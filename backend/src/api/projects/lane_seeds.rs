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
//! token has the route. Kronn only *suggests* candidates, and a newly declared
//! path that is, or holds, something that looks like a secret file (`.env`, a
//! key) is refused until the human has seen the files and confirmed it. The
//! confirmation is stored per file: a `.env` or key that shows up in an accepted
//! directory later is not covered, and holds that seed back (`seed_worktree`)
//! until the human confirms it too.
//!
//! The declaration lives in Kronn (the human's word) and is mirrored into the
//! `[lane_seeds]` table of the main checkout's `kronn/project.toml` when the
//! project has one. That table never applies by itself: it reaches the
//! repository through a pull request, not through the human gate, so it is only
//! *proposed* here (`profile_seeds`), and adopting it is a declaration like any
//! other, secret check and confirmation included.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::task_worker_lane_seeds::{
    self, ConfirmedSecrets, LaneSeedCandidate, LaneSeedSecretFinding, LaneSeedsRecord,
    LaneSeedsSource, SeedStatus,
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
    /// The paths seeded into each task worktree: what a human declared here.
    pub seeds: Vec<String>,
    pub source: LaneSeedsSource,
    /// What `kronn/project.toml` on the default branch proposes, when it does.
    /// Nothing is seeded from it until a human adopts it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub profile_seeds: Option<Vec<String>>,
    /// Why that file could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub profile_error: Option<String>,
    /// Newly declared paths that are, or hold, a secret-looking file and were NOT
    /// saved: send their `path` back in `confirm_secret_paths` once the human
    /// has seen the files and accepted them.
    pub needs_confirmation: Vec<LaneSeedSecretFinding>,
    /// Secret-looking files the declared seeds hold right now. A finding is
    /// `held` when some of its files are not confirmed: that seed is not copied
    /// into task worktrees until the human confirms them.
    pub secret_findings: Vec<LaneSeedSecretFinding>,
    /// Why the declared seeds could not be checked for secrets.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub secret_scan_error: Option<String>,
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
    /// Newly declared seeds holding secret-looking files that the human confirmed.
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
    needs_confirmation: Vec<LaneSeedSecretFinding>,
    file_write: Result<bool, String>,
) -> Result<ProjectLaneSeeds, String> {
    let id = project.id.clone();
    let (declared, confirmed, last_run) = state
        .db
        .with_read_conn(move |conn| {
            Ok((
                crate::db::projects::lane_seeds(conn, &id)?,
                crate::db::projects::lane_seeds_confirmed(conn, &id)?,
                crate::db::orchestration::latest_lane_seeds_for_project(conn, &id)?,
            ))
        })
        .await
        .map_err(|error| error.to_string())?;
    let source = if declared.is_some() {
        LaneSeedsSource::Project
    } else {
        LaneSeedsSource::None
    };
    let seeds = declared.unwrap_or_default();
    let repo = crate::core::scanner::resolve_host_path(&project.path);
    let to_scan = seeds.clone();
    let (profile, findings) = tokio::task::spawn_blocking(move || {
        (
            crate::core::project_profile::declared_lane_seeds(&repo),
            task_worker_lane_seeds::secret_findings_against(&repo, &to_scan, &confirmed),
        )
    })
    .await
    .map_err(|error| format!("the declaration could not be read: {error}"))?;
    let (profile_seeds, profile_error) = match profile {
        Ok(found) => (found, None),
        Err(error) => (None, Some(error)),
    };
    let (secret_findings, secret_scan_error) = match findings {
        Ok(found) => (found, None),
        Err(error) => (Vec::new(), Some(error)),
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
        secret_findings,
        secret_scan_error,
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

/// What a save does with the secret-looking files its seeds hold.
struct SecretJudgement {
    /// Newly declared seeds holding files nobody confirmed: not saved.
    blocked: Vec<LaneSeedSecretFinding>,
    /// The confirmations to store with the seeds.
    confirmed: ConfirmedSecrets,
}

/// Judge every seed of a save against what is confirmed so far. A seed the human
/// confirms in this request has all the files it holds now confirmed. A seed that
/// was already declared and holds a file nobody confirmed is kept, and stays
/// held until someone confirms it; a newly declared one is blocked. Fails closed:
/// a tree that cannot be scanned is an error.
fn judge_secrets(
    repo: &std::path::Path,
    seeds: &[String],
    previous: &[String],
    stored: &ConfirmedSecrets,
    confirm_now: &[String],
) -> Result<SecretJudgement, String> {
    let mut blocked = Vec::new();
    let mut confirmed = ConfirmedSecrets::new();
    for seed in seeds {
        let found = task_worker_lane_seeds::scan_secret_files(repo, seed)?;
        if found.is_empty() {
            continue;
        }
        let unconfirmed =
            task_worker_lane_seeds::unconfirmed_secret_files(&found, stored.get(seed));
        if unconfirmed.is_empty() || confirm_now.contains(seed) {
            confirmed.insert(seed.clone(), found);
        } else if previous.contains(seed) {
            // Held: keep what was confirmed and still exists.
            let still: Vec<String> = found
                .into_iter()
                .filter(|file| !unconfirmed.contains(file))
                .collect();
            if !still.is_empty() {
                confirmed.insert(seed.clone(), still);
            }
        } else {
            blocked.push(LaneSeedSecretFinding::new(seed, found, true));
        }
    }
    Ok(SecretJudgement { blocked, confirmed })
}

/// PUT /api/projects/{id}/lane-seeds — replaces the list; an empty one clears it.
/// A newly declared seed that is, or holds, a secret-looking file is not saved
/// until the request names it in `confirm_secret_paths`: the answer then lists
/// it, with the files found, in `needs_confirmation`. A tree that cannot be
/// checked refuses the save (fail closed).
pub async fn set(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SetProjectLaneSeeds>,
) -> Json<ApiResponse<ProjectLaneSeeds>> {
    let seeds = match task_worker_lane_seeds::normalize_seed_list(&request.seeds) {
        Ok(seeds) => seeds,
        Err(reason) => return Json(ApiResponse::err(reason)),
    };
    let confirm_now =
        match task_worker_lane_seeds::normalize_seed_list(&request.confirm_secret_paths) {
            Ok(confirm_now) => confirm_now,
            Err(reason) => return Json(ApiResponse::err(reason)),
        };
    let project = match project_of(&state, id).await {
        Ok(Some(project)) => project,
        Ok(None) => return not_found(),
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let repo = crate::core::scanner::resolve_host_path(&project.path);

    let project_id = project.id.clone();
    let (previous, stored) = match state
        .db
        .with_read_conn(move |conn| {
            Ok((
                crate::db::projects::lane_seeds(conn, &project_id)?,
                crate::db::projects::lane_seeds_confirmed(conn, &project_id)?,
            ))
        })
        .await
    {
        Ok((previous, stored)) => (previous.unwrap_or_default(), stored),
        Err(error) => return Json(ApiResponse::err(error.to_string())),
    };
    let judged = {
        let (repo, seeds) = (repo.clone(), seeds.clone());
        tokio::task::spawn_blocking(move || {
            judge_secrets(&repo, &seeds, &previous, &stored, &confirm_now)
        })
        .await
    };
    let judged = match judged {
        Ok(Ok(judged)) => judged,
        Ok(Err(reason)) => {
            return Json(ApiResponse::err(format!(
                "the seeds could not be checked for secrets, nothing was saved: {reason}"
            )))
        }
        Err(error) => {
            return Json(ApiResponse::err(format!(
                "the secret check did not complete, nothing was saved: {error}"
            )))
        }
    };
    if !judged.blocked.is_empty() {
        return match view(&state, &project, judged.blocked, Ok(false)).await {
            Ok(seeds) => Json(ApiResponse::ok(seeds)),
            Err(error) => Json(ApiResponse::err(error)),
        };
    }

    let (stored, confirmed, project_id) = (seeds.clone(), judged.confirmed, project.id.clone());
    let changed = state
        .db
        .with_conn(move |conn| {
            crate::db::projects::set_lane_seeds_confirmed(conn, &project_id, &stored, &confirmed)
        })
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
