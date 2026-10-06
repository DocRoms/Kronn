// Audit run read-model endpoints: live status polling, run history,
// resumable lookup, per-step timeline. The legacy `POST /ai-audit`
// launcher that lived here (pre-0.8.2 9-step pipeline, no lease, no
// drop-guard, no audit_runs row) was removed — `full_audit` in full.rs
// is the only launch path besides `partial_audit`.

use axum::{
    extract::{Path, State},
    Json,
};

use crate::models::*;
use crate::AppState;

/// GET /api/projects/:id/audit-status
///
/// Returns the current in-flight audit progress for this project, or `None`
/// if no audit is running. The UI polls this endpoint every ~2 s while its
/// `kronn:audit:<projectId>` localStorage entry is set, so the progress bar
/// survives tab/page navigation (the server-side audit process keeps
/// running whether or not an SSE client is attached).
///
/// Progress entries are written by `partial_audit` and `full_audit` as
/// they advance, and cleared on done / cancelled / error.
pub async fn audit_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Option<AuditProgress>>> {
    let snapshot = match state.audit_tracker.lock() {
        Ok(t) => t.get_progress(&id),
        Err(_) => return Json(ApiResponse::err("audit tracker lock poisoned")),
    };
    Json(ApiResponse::ok(snapshot))
}

/// 0.8.3 (#288) — list ALL audits currently in progress across every
/// project. Powers the `ActiveAuditsPopover` on the Projets nav button,
/// same UX as `ActiveRunsPopover` for workflows: one badge with the
/// running count, click intercepts navigation to surface the list +
/// per-audit Stop button. Returns an empty Vec when no audit is
/// running (the popover then hides itself; the nav button keeps the
/// normal click-to-navigate behavior).
pub async fn audit_status_all(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<AuditProgress>>> {
    let snapshot = match state.audit_tracker.lock() {
        Ok(t) => t.progress.values().cloned().collect::<Vec<_>>(),
        Err(_) => return Json(ApiResponse::err("audit tracker lock poisoned")),
    };
    Json(ApiResponse::ok(snapshot))
}

/// GET /api/audit/steps — the Full audit's steps in run order, from the same
/// chain the pipeline executes, so the UI never keeps its own copy.
pub async fn audit_steps() -> Json<ApiResponse<Vec<AuditStepInfo>>> {
    let steps = super::assemble_chained_steps(crate::models::AuditKind::Full)
        .into_iter()
        .enumerate()
        .map(|(i, step)| AuditStepInfo {
            index: i as u32 + 1,
            target_file: step.target_file.to_string(),
        })
        .collect();
    Json(ApiResponse::ok(steps))
}

/// 0.8.4 (#298) — fetch the most-recent **completed** audit run for a
/// project, or `None`. Sister of `audit_latest_resumable` (which only
/// returns Interrupted rows); this one returns Completed rows so the
/// ProjectCard recap panel knows which `audit_run_id` to feed to
/// `audit_run_steps` below.
pub async fn audit_latest(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Option<crate::models::AuditRun>>> {
    let result = state
        .db
        .with_read_conn(move |conn| crate::db::audit_runs::latest_completed(conn, &id))
        .await;
    match result {
        Ok(row) => Json(ApiResponse::ok(row)),
        Err(e) => Json(ApiResponse::err(format!("db: {e}"))),
    }
}

/// 0.8.4 (#317 / B1) — admin cleanup: force-mark every `Running`
/// audit_run as Interrupted, regardless of age. Used by the recap-
/// panel "Nettoyer l'historique" button when the operator KNOWS
/// nothing is actually running (just rebuilt docker, mass-killed
/// stuck audits, etc.). Returns the count of rows touched.
///
/// Boot-time reconcile (30-min threshold) is automatic in
/// `Database::open`. This endpoint is the manual escape hatch.
pub async fn audit_runs_cleanup(State(state): State<AppState>) -> Json<ApiResponse<u64>> {
    let result = state
        .db
        .with_conn(crate::db::audit_runs::reconcile_all_running)
        .await;
    match result {
        Ok(n) => Json(ApiResponse::ok(n)),
        Err(e) => Json(ApiResponse::err(format!("db: {e}"))),
    }
}

/// 0.8.4 (#298) — history of recent audit runs for a project, newest
/// first. Powers the audit-history chip strip on the ProjectCard recap
/// panel: each chip = one row from `audit_runs`, click switches the
/// per-step table to that run's data. Capped at 20 to avoid heavy
/// renders on projects with hundreds of historical audits.
pub async fn audit_history(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<crate::models::AuditRun>>> {
    let result = state
        .db
        .with_conn(move |conn| crate::db::audit_runs::list_recent(conn, &id, 20))
        .await;
    match result {
        Ok(runs) => Json(ApiResponse::ok(runs)),
        Err(e) => Json(ApiResponse::err(format!("db: {e}"))),
    }
}

/// 0.8.4 (#298) — list per-step metrics for a finished (or running)
/// audit run. Powers the "▾ Détails du dernier audit" collapsed panel
/// on ProjectCard: one row per step with file label, duration_ms,
/// step_tokens, cumulative_tokens, success/warning. Ordered by
/// step_index ASC so the UI can render the timeline directly.
///
/// Returns an empty Vec for run_ids with no recorded steps yet (which
/// is also the legacy case — runs that completed before 0.8.4 don't
/// have an `audit_run_steps` row).
pub async fn audit_run_steps(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Json<ApiResponse<Vec<crate::models::AuditRunStep>>> {
    let result = state
        .db
        .with_conn(move |conn| {
            let mut steps = crate::db::audit_runs::list_audit_steps(conn, &run_id)?;
            crate::db::audit_runs::price_steps(conn, &mut steps)?;
            Ok(steps)
        })
        .await;
    match result {
        Ok(steps) => Json(ApiResponse::ok(steps)),
        Err(e) => Json(ApiResponse::err(format!("db: {e}"))),
    }
}

/// Runs whose steps the timeline merges (newest first): a resume or a partial
/// run records only the steps it ran.
const TIMELINE_STEP_RUNS: usize = 12;
const TIMELINE_HISTORY: u32 = 20;

/// Everything the audit timeline draws, in one request.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AuditTimelineData {
    /// Latest runs of every kind, newest first.
    pub runs: Vec<AuditRun>,
    /// Steps of the newest Full and Partial runs, grouped by run in `runs` order.
    pub steps: Vec<AuditRunStep>,
    /// Audits the branch's `docs/.kronn.json` records (another instance, an
    /// attestation, legacy evidence), oldest first. Empty without the file.
    pub recorded_audits: Vec<crate::core::kronn_state::AuditEntry>,
    pub recorded_validated_at: Option<String>,
    /// The validation discussion linked to the latest run when that run is
    /// Completed, archived or not: the timeline offers to validate once it
    /// has finished.
    pub latest_validation: Option<AuditTimelineValidation>,
}

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AuditTimelineValidation {
    pub discussion_id: String,
    /// Same terminal-signal parser as the validate-audit gate.
    pub finished: bool,
    pub archived: bool,
}

/// GET /api/projects/{id}/audit-timeline
pub async fn audit_timeline(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AuditTimelineData>> {
    let result = state
        .db
        .with_read_conn(move |conn| {
            let project = crate::db::projects::get_project(conn, &id)?;
            let runs = crate::db::audit_runs::list_recent(conn, &id, TIMELINE_HISTORY)?;
            let mut steps = Vec::new();
            for run in runs
                .iter()
                .filter(|run| run.kind == "Full" || run.kind == "Partial")
                .take(TIMELINE_STEP_RUNS)
            {
                steps.extend(crate::db::audit_runs::list_audit_steps(conn, &run.id)?);
            }
            crate::db::audit_runs::price_steps(conn, &mut steps)?;
            // The latest run of any kind, as the validate-audit gate reads it.
            let latest_validation = match runs.first() {
                Some(run) if run.status == "Completed" => match &run.validation_discussion_id {
                    Some(disc_id) => crate::db::discussions::get_discussion(conn, disc_id)?
                        .filter(|disc| disc.project_id.as_deref() == Some(id.as_str()))
                        .map(|disc| AuditTimelineValidation {
                            finished: super::validate::validation_discussion_finished(&disc),
                            archived: disc.archived,
                            discussion_id: disc.id,
                        }),
                    None => None,
                },
                _ => None,
            };
            Ok((project, runs, steps, latest_validation))
        })
        .await;
    let (project, runs, steps, latest_validation) = match result {
        Ok((Some(project), runs, steps, latest)) => (project, runs, steps, latest),
        Ok((None, _, _, _)) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("db: {e}"))),
    };
    let recorded = tokio::task::spawn_blocking(move || {
        let root = crate::core::scanner::resolve_host_path(&project.path);
        crate::core::kronn_state::read(&root)
    })
    .await
    .ok()
    .flatten();
    let (recorded_audits, recorded_validated_at) = recorded
        .map(|state| (state.audits, state.validated_at))
        .unwrap_or_default();
    Json(ApiResponse::ok(AuditTimelineData {
        runs,
        steps,
        recorded_audits,
        recorded_validated_at,
        latest_validation,
    }))
}

/// 0.8.3 (#311) — fetch the most-recent resumable audit run for a project,
/// or `None`. Resumable = `status = 'Interrupted'` AND
/// the persisted checkpoint belongs to the latest run. The frontend uses this
/// to decide whether the "Lancer l'audit" button should become a dynamic
/// "Reprendre à l'étape N" CTA and sends the authoritative `resume_run_id`.
pub async fn audit_latest_resumable(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Option<ResumableAudit>>> {
    let result = state
        .db
        .with_conn(move |conn| {
            let Some(run) = crate::db::audit_runs::latest_resumable(conn, &id)? else {
                return Ok(None);
            };
            let steps = crate::db::audit_runs::list_audit_steps(conn, &run.id)?;
            Ok(Some(ResumableAudit::new(run, &steps)))
        })
        .await;
    match result {
        Ok(row) => Json(ApiResponse::ok(row)),
        Err(e) => Json(ApiResponse::err(format!("db: {e}"))),
    }
}

/// An interrupted run and what a resume would run (KT-931). `last_completed_step`
/// is a count of successful steps, not a position: the button names the steps
/// that are left, which a resume re-runs and nothing else.
#[derive(serde::Serialize)]
pub struct ResumableAudit {
    #[serde(flatten)]
    run: crate::models::AuditRun,
    /// 1-based steps a resume runs: the ones that failed and the ones that
    /// never ran. Empty when the run's kind is unknown.
    steps_to_redo: Vec<u32>,
}

impl ResumableAudit {
    fn new(run: crate::models::AuditRun, steps: &[crate::models::AuditRunStep]) -> Self {
        let steps_to_redo = match crate::models::AuditKind::from_label(&run.kind) {
            Some(kind) => {
                let done = super::full::already_succeeded_step_indices(steps);
                let total = super::assemble_chained_steps(kind).len() as u32;
                (1..=total).filter(|step| !done.contains(step)).collect()
            }
            None => Vec::new(),
        };
        Self { run, steps_to_redo }
    }
}

#[cfg(test)]
mod resumable_tests {
    use super::ResumableAudit;

    fn step(index: u32, ok: bool) -> crate::models::AuditRunStep {
        serde_json::from_value(serde_json::json!({
            "audit_run_id": "r", "step_index": index, "file_label": "x",
            "started_at": "2026-10-01T00:00:00Z", "ended_at": "2026-10-01T00:00:01Z",
            "cli_success": ok,
        }))
        .unwrap()
    }

    fn run(kind: &str) -> crate::models::AuditRun {
        serde_json::from_value(serde_json::json!({
            "id": "r", "project_id": "p", "kind": kind, "agent_type": "ClaudeCode",
            "started_at": "2026-10-01T00:00:00Z", "status": "Interrupted",
            "last_completed_step": 15,
        }))
        .unwrap()
    }

    #[test]
    fn the_resume_names_the_failed_step_and_the_steps_that_never_ran() {
        let total =
            crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full).len() as u32;
        // Step 5 failed; the stream then ended after step 14: 15 and 16 never ran.
        let steps: Vec<_> = (1..=14).map(|i| step(i, i != 5)).collect();
        let resumable = ResumableAudit::new(run("Full"), &steps);
        assert_eq!(total, 16);
        assert_eq!(resumable.steps_to_redo, vec![5, 15, 16]);
        // Flattened: the client keeps reading the run's own fields.
        let json = serde_json::to_value(&resumable).unwrap();
        assert_eq!(json["id"], "r");
        assert_eq!(json["last_completed_step"], 15);
        assert_eq!(json["steps_to_redo"], serde_json::json!([5, 15, 16]));
    }

    #[test]
    fn a_run_of_an_unknown_kind_names_nothing() {
        assert!(ResumableAudit::new(run("Nonsense"), &[])
            .steps_to_redo
            .is_empty());
    }
}

#[cfg(test)]
mod audit_steps_tests {
    #[tokio::test]
    async fn lists_the_full_chain_in_run_order() {
        let steps = super::audit_steps().await.0.data.expect("steps");
        assert_eq!(steps.len(), 16);
        assert!(steps
            .iter()
            .enumerate()
            .all(|(i, s)| s.index as usize == i + 1));
        assert_eq!(steps[0].target_file, "docs/AGENTS.md");
        // The consolidation's position may move between versions; the UI
        // groups steps by target file, so only its presence is pinned.
        assert_eq!(
            steps
                .iter()
                .filter(|s| s.target_file == "docs/decisions.md")
                .count(),
            1
        );
    }
}
