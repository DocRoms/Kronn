//! KT-373 — disk maintenance surface.
//!
//! Two endpoints, deliberately asymmetric: listing is free and answers "what
//! could be reclaimed, and what is holding it"; reclaiming acts only on the
//! targets a human has just been shown. There is no "clean everything" verb,
//! because the 2026-08-21 recovery was a sequence of individually reasonable
//! deletions, one of which was wrong.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use crate::core::{scanner, worktree};
use crate::models::ApiResponse;
use crate::AppState;

/// One reclaimable target, with everything a human needs to decide.
#[derive(Debug, Serialize)]
pub struct ReclaimCandidate {
    pub worktree_path: String,
    pub target_path: String,
    /// Bytes measured, floored at the scan budget.
    pub bytes: u64,
    /// True when `bytes` is a floor rather than the whole figure — an exact
    /// walk of a large Rust target costs minutes, so it is never promised.
    pub bytes_are_partial: bool,
    /// Seconds since the target was last modified, when the filesystem says.
    pub idle_seconds: Option<u64>,
    /// `reclaimable` when durable state authorises it; otherwise the reason.
    pub state: String,
}

#[derive(Debug, Serialize)]
pub struct ReclaimPlan {
    pub project_id: String,
    pub candidates: Vec<ReclaimCandidate>,
    /// Free space right now, so the operator can judge urgency without a shell.
    pub available_gib: u64,
}

#[derive(Debug, Deserialize)]
pub struct ReclaimQuery {
    pub project_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ReclaimRequest {
    pub project_id: String,
    /// The exact targets to reclaim, as listed by the dry run. Nothing is
    /// deleted that the caller did not name: a plan computed a minute ago must
    /// not authorise a deletion decided now.
    pub target_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ReclaimResult {
    pub target_path: String,
    pub reclaimed: bool,
    pub bytes: u64,
    /// Present when the target was refused, naming why.
    pub refused: Option<String>,
}

async fn project_repo_path(
    state: &AppState,
    project_id: &str,
) -> Result<std::path::PathBuf, String> {
    let pid = project_id.to_string();
    let project = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("project {project_id} not found"))?;
    Ok(scanner::resolve_host_path(&project.path))
}

/// `GET /api/maintenance/build-artifacts` — the dry run.
///
/// Read-only, and it lists refused targets too. A maintenance view that hides
/// what it cannot touch invites someone to go delete it by hand, which is how
/// a live worktree got cleaned during the incident.
pub async fn list_build_artifacts(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<ReclaimQuery>,
) -> Json<ApiResponse<ReclaimPlan>> {
    let repo_path = match project_repo_path(&state, &query.project_id).await {
        Ok(path) => path,
        Err(error) => return Json(ApiResponse::err(error)),
    };

    let found = worktree::scan_build_artifacts(&repo_path);
    let now = std::time::SystemTime::now();
    let mut candidates = Vec::with_capacity(found.len());
    for target in found {
        let canonical = target.worktree_path.to_string_lossy().to_string();
        // A filesystem-level refusal outranks the durable verdict: there is no
        // point asking whether an execution is finished when the target is a
        // symlink we would never follow anyway.
        if let Some(reason) = target.refusal {
            candidates.push(ReclaimCandidate {
                worktree_path: canonical,
                target_path: target.target_path.to_string_lossy().to_string(),
                bytes: target.bytes,
                bytes_are_partial: target.size_is_partial,
                idle_seconds: None,
                state: reason,
            });
            continue;
        }
        let liveness = state
            .db
            .with_conn({
                let canonical = canonical.clone();
                move |conn| crate::db::orchestration::worktree_cleanup_liveness(conn, &canonical)
            })
            .await;
        let state_label = match liveness {
            Ok(worktree::ExecutionLiveness::Terminal) => "reclaimable".to_string(),
            Ok(worktree::ExecutionLiveness::Active(reason))
            | Ok(worktree::ExecutionLiveness::Unknown(reason)) => reason,
            Err(error) => format!("durable state unreadable: {error}"),
        };
        candidates.push(ReclaimCandidate {
            worktree_path: canonical,
            target_path: target.target_path.to_string_lossy().to_string(),
            bytes: target.bytes,
            bytes_are_partial: target.size_is_partial,
            idle_seconds: target
                .modified
                .and_then(|modified| now.duration_since(modified).ok())
                .map(|elapsed| elapsed.as_secs()),
            state: state_label,
        });
    }

    let available_gib = fs2::available_space(&repo_path).unwrap_or(0) / (1024 * 1024 * 1024);
    Json(ApiResponse::ok(ReclaimPlan {
        project_id: query.project_id,
        candidates,
        available_gib,
    }))
}

/// `POST /api/maintenance/build-artifacts/reclaim` — act on named targets only.
///
/// Each target is re-checked against durable state at the moment of deletion.
/// The dry run informs the human; it never authorises the action, because the
/// worktree may have been picked up again between the two calls.
pub async fn reclaim_build_artifacts(
    State(state): State<AppState>,
    Json(request): Json<ReclaimRequest>,
) -> Json<ApiResponse<Vec<ReclaimResult>>> {
    let repo_path = match project_repo_path(&state, &request.project_id).await {
        Ok(path) => path,
        Err(error) => return Json(ApiResponse::err(error)),
    };

    let mut results = Vec::with_capacity(request.target_paths.len());
    for target_path in request.target_paths {
        // The caller names `target/`; ownership is decided on its worktree.
        let worktree_path = match std::path::Path::new(&target_path).parent() {
            Some(parent) => parent.to_path_buf(),
            None => {
                results.push(ReclaimResult {
                    target_path,
                    reclaimed: false,
                    bytes: 0,
                    refused: Some("not a target directory inside a worktree".into()),
                });
                continue;
            }
        };
        let canonical = worktree_path.to_string_lossy().to_string();
        let liveness = match state
            .db
            .with_conn(move |conn| {
                crate::db::orchestration::worktree_cleanup_liveness(conn, &canonical)
            })
            .await
        {
            Ok(liveness) => liveness,
            Err(error) => {
                results.push(ReclaimResult {
                    target_path,
                    reclaimed: false,
                    bytes: 0,
                    refused: Some(format!("durable state unreadable: {error}")),
                });
                continue;
            }
        };
        let outcome =
            worktree::clean_worktree_build_artifacts(&repo_path, &worktree_path, liveness);
        // Audited before the result is assembled, so an operator-driven reclaim
        // leaves the same durable trace as the automatic one. A deletion that
        // took gigabytes off the disk must stay answerable after the logs go.
        let audited = match &outcome {
            Ok(report) => Ok((report.bytes_reclaimed, report.bytes_are_partial)),
            Err(reason) => Err(reason.clone()),
        };
        let canonical = worktree_path.to_string_lossy().to_string();
        if let Err(error) = state
            .db
            .with_conn(move |conn| {
                crate::db::orchestration::record_artifact_reclaim(conn, &canonical, audited)
            })
            .await
        {
            tracing::warn!(target = %target_path, "reclaim audit failed: {error}");
        }
        match outcome {
            Ok(report) => results.push(ReclaimResult {
                target_path,
                reclaimed: true,
                bytes: report.bytes_reclaimed,
                refused: None,
            }),
            // One refusal stops that target and nothing else: the next one is
            // judged on its own state, never on this one's outcome.
            Err(reason) => results.push(ReclaimResult {
                target_path,
                reclaimed: false,
                bytes: 0,
                refused: Some(reason),
            }),
        }
    }
    Json(ApiResponse::ok(results))
}

/// One `target/wt-*` directory in the lane-target dry run (KT-1256).
#[derive(Debug, Serialize)]
pub struct LaneTargetEntry {
    pub target_path: String,
    /// `reclaimable`, `claimed`, `kept` or `unmarked`.
    pub state: String,
    /// Why a lane is claimed or kept, or what an unmarked directory is.
    pub reason: Option<String>,
    /// The execution Kronn provisioned the lane for, when it is marked.
    pub execution_id: Option<String>,
    /// Bytes measured (reclaimable lanes only), floored at the scan budget.
    pub bytes: Option<u64>,
    pub bytes_are_partial: bool,
}

#[derive(Debug, Serialize)]
pub struct LaneTargetPlan {
    pub project_id: String,
    pub entries: Vec<LaneTargetEntry>,
    pub available_gib: u64,
}

/// `GET /api/maintenance/lane-targets?project_id=…` — the dry run.
///
/// Lists every `target/wt-*` of the project, including the ones Kronn will
/// never touch (no ownership marker) and the ones it keeps, each with why.
/// Read-only: nothing is deleted and nothing is measured but reclaimable lanes.
pub async fn list_lane_targets(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<ReclaimQuery>,
) -> Json<ApiResponse<LaneTargetPlan>> {
    let repo_path = match project_repo_path(&state, &query.project_id).await {
        Ok(path) => path,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let sweep = match crate::api::orchestration::sweep_project_lane_targets(
        &state.db,
        &query.project_id,
        &repo_path,
        worktree::LaneSweepScope::ReportOnly,
    )
    .await
    {
        Ok(sweep) => sweep,
        Err(error) => return Json(ApiResponse::err(error)),
    };

    let mut entries = Vec::new();
    for (owner, report) in sweep.would_remove {
        entries.push(LaneTargetEntry {
            target_path: report.target_path.to_string_lossy().to_string(),
            state: "reclaimable".to_string(),
            reason: None,
            execution_id: Some(owner.execution_id),
            bytes: Some(report.bytes_reclaimed),
            bytes_are_partial: report.bytes_are_partial,
        });
    }
    for (path, reason) in sweep.claimed {
        entries.push(LaneTargetEntry {
            target_path: path.to_string_lossy().to_string(),
            state: "claimed".to_string(),
            reason: Some(reason),
            execution_id: None,
            bytes: None,
            bytes_are_partial: false,
        });
    }
    for (path, reason) in sweep.kept {
        entries.push(LaneTargetEntry {
            target_path: path.to_string_lossy().to_string(),
            state: "kept".to_string(),
            reason: Some(reason),
            execution_id: None,
            bytes: None,
            bytes_are_partial: false,
        });
    }
    for path in sweep.unmarked {
        entries.push(LaneTargetEntry {
            target_path: path.to_string_lossy().to_string(),
            state: "unmarked".to_string(),
            reason: Some(format!(
                "no {} marker: Kronn did not create it and never removes it",
                worktree::LANE_OWNER_MARKER
            )),
            execution_id: None,
            bytes: None,
            bytes_are_partial: false,
        });
    }
    entries.sort_by(|a, b| a.target_path.cmp(&b.target_path));

    let available_gib = fs2::available_space(&repo_path).unwrap_or(0) / (1024 * 1024 * 1024);
    Json(ApiResponse::ok(LaneTargetPlan {
        project_id: query.project_id,
        entries,
        available_gib,
    }))
}

/// `POST /api/maintenance/lane-targets/reclaim` — act on named lane targets only.
///
/// Each named path is re-judged now: marked by Kronn, its execution finished,
/// its worktree gone or clean. Anything else is refused with its reason.
pub async fn reclaim_lane_targets(
    State(state): State<AppState>,
    Json(request): Json<ReclaimRequest>,
) -> Json<ApiResponse<Vec<ReclaimResult>>> {
    let repo_path = match project_repo_path(&state, &request.project_id).await {
        Ok(path) => path,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let targets = request
        .target_paths
        .iter()
        .map(std::path::PathBuf::from)
        .collect();
    let outcomes = match crate::api::orchestration::reclaim_named_lane_targets(
        &state.db,
        &request.project_id,
        &repo_path,
        targets,
    )
    .await
    {
        Ok(outcomes) => outcomes,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let results = outcomes
        .into_iter()
        .map(|(path, outcome)| {
            let target_path = path.to_string_lossy().to_string();
            match outcome {
                worktree::LaneTargetOutcome::Removed(_, report) => ReclaimResult {
                    target_path,
                    reclaimed: true,
                    bytes: report.bytes_reclaimed,
                    refused: None,
                },
                worktree::LaneTargetOutcome::WouldRemove(..) => ReclaimResult {
                    target_path,
                    reclaimed: false,
                    bytes: 0,
                    refused: Some("not removed: outside the reclaim scope".to_string()),
                },
                worktree::LaneTargetOutcome::Claimed(_, reason)
                | worktree::LaneTargetOutcome::Kept(_, reason) => ReclaimResult {
                    target_path,
                    reclaimed: false,
                    bytes: 0,
                    refused: Some(reason),
                },
                worktree::LaneTargetOutcome::Unmarked => ReclaimResult {
                    target_path,
                    reclaimed: false,
                    bytes: 0,
                    refused: Some(format!(
                        "no {} marker: Kronn did not create this directory",
                        worktree::LANE_OWNER_MARKER
                    )),
                },
            }
        })
        .collect();
    Json(ApiResponse::ok(results))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dry run must show refused targets too. A maintenance view that hides
    /// what it cannot touch invites someone to go delete it by hand — which is
    /// precisely what happened on 2026-08-21.
    #[test]
    fn a_refused_candidate_is_listed_with_its_reason_not_omitted() {
        let refused = ReclaimCandidate {
            worktree_path: "/repo/.kronn/worktrees/KT-1-aaaa".into(),
            target_path: "/repo/.kronn/worktrees/KT-1-aaaa/target".into(),
            bytes: 12_000_000_000,
            bytes_are_partial: true,
            idle_seconds: Some(86_400),
            state: "session 42 is still attached".into(),
        };
        let plan = ReclaimPlan {
            project_id: "p1".into(),
            candidates: vec![refused],
            available_gib: 3,
        };
        let json = serde_json::to_value(&plan).unwrap();
        assert_eq!(
            json["candidates"][0]["state"],
            "session 42 is still attached"
        );
        // The size is offered as a floor, and says so: an exact walk of a large
        // Rust target costs minutes and is never promised.
        assert_eq!(json["candidates"][0]["bytes_are_partial"], true);
        // Free space travels with the plan so urgency is judged without a shell.
        assert_eq!(json["available_gib"], 3);
    }

    #[test]
    fn a_reclaim_result_says_which_targets_were_refused_and_why() {
        let results = vec![
            ReclaimResult {
                target_path: "/repo/.kronn/worktrees/KT-1-aaaa/target".into(),
                reclaimed: true,
                bytes: 4_000_000_000,
                refused: None,
            },
            ReclaimResult {
                target_path: "/repo/.kronn/worktrees/KT-2-bbbb/target".into(),
                reclaimed: false,
                bytes: 0,
                refused: Some("an unreleased worker lease holds it".into()),
            },
        ];
        let json = serde_json::to_value(&results).unwrap();
        // One refusal stops that target and nothing else: the second entry is
        // judged on its own state, never on the first one's outcome.
        assert_eq!(json[0]["reclaimed"], true);
        assert_eq!(json[1]["reclaimed"], false);
        assert!(json[1]["refused"].as_str().unwrap().contains("lease"));
    }

    #[test]
    fn a_reclaim_request_names_its_targets_explicitly() {
        // There is no "clean everything" verb. The 2026-08-21 recovery was a
        // sequence of individually reasonable deletions, one of which was wrong.
        let request: ReclaimRequest = serde_json::from_value(serde_json::json!({
            "project_id": "p1",
            "target_paths": ["/repo/.kronn/worktrees/KT-1-aaaa/target"],
        }))
        .unwrap();
        assert_eq!(request.target_paths.len(), 1);

        let without_targets = serde_json::from_value::<ReclaimRequest>(serde_json::json!({
            "project_id": "p1",
        }));
        assert!(
            without_targets.is_err(),
            "a request that names nothing must not deserialise into one that cleans everything",
        );
    }
}
