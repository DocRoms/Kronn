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
        .with_conn(move |conn| crate::db::audit_runs::latest_completed(conn, &id))
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
        .with_conn(move |conn| crate::db::audit_runs::list_audit_steps(conn, &run_id))
        .await;
    match result {
        Ok(steps) => Json(ApiResponse::ok(steps)),
        Err(e) => Json(ApiResponse::err(format!("db: {e}"))),
    }
}

/// 0.8.3 (#311) — fetch the most-recent resumable audit run for a project,
/// or `None`. Resumable = `status = 'Interrupted'` AND
/// the persisted checkpoint belongs to the latest run. The frontend uses this
/// to decide whether the "Lancer l'audit" button should become a dynamic
/// "Reprendre à l'étape N" CTA and sends the authoritative `resume_run_id`.
pub async fn audit_latest_resumable(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Option<crate::models::AuditRun>>> {
    let result = state
        .db
        .with_conn(move |conn| crate::db::audit_runs::latest_resumable(conn, &id))
        .await;
    match result {
        Ok(row) => Json(ApiResponse::ok(row)),
        Err(e) => Json(ApiResponse::err(format!("db: {e}"))),
    }
}
