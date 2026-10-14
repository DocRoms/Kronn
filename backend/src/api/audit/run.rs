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
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
) -> Json<ApiResponse<Option<AuditProgress>>> {
    let mut snapshot = match state.audit_tracker.lock() {
        Ok(t) => t.get_progress(&id),
        Err(_) => return Json(ApiResponse::err("audit tracker lock poisoned")),
    };
    // An agent keeps the counts-only view: the audit agent's recent actions
    // are for the user's panel.
    if bridge.is_some() {
        if let Some(progress) = snapshot.as_mut() {
            progress.recent_activity = None;
        }
    }
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

/// What a client needs to tell where a project's audits stand.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AuditLatest {
    /// The newest run of any status: a Cancelled, Failed or Interrupted run
    /// after a Completed one is the project's state, not history.
    pub latest: Option<AuditRun>,
    /// The newest Completed run, the health snapshot.
    pub latest_completed: Option<AuditRun>,
    /// 1-based steps a resume of `latest` runs; empty when it is not resumable.
    pub steps_to_redo: Vec<u32>,
    /// The lease is held by a run nobody can settle: every launch is refused
    /// until the cleanup action releases it (KT-1134).
    pub lease_orphaned: bool,
}

/// GET /api/projects/{id}/audit-latest
pub async fn audit_latest(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AuditLatest>> {
    let lease_orphaned = match state.audit_tracker.lock() {
        Ok(t) => t.lease_orphaned(&id),
        Err(_) => return Json(ApiResponse::err("audit tracker lock poisoned")),
    };
    let result = state
        .db
        .with_read_conn(move |conn| {
            let latest = crate::db::audit_runs::list_recent(conn, &id, 1)?
                .into_iter()
                .next();
            let latest_completed = crate::db::audit_runs::latest_completed(conn, &id)?;
            let steps_to_redo = match crate::db::audit_runs::latest_resumable(conn, &id)? {
                Some(run) => {
                    let steps = crate::db::audit_runs::list_audit_steps(conn, &run.id)?;
                    ResumableAudit::new(run, &steps).steps_to_redo
                }
                None => Vec::new(),
            };
            Ok(AuditLatest {
                latest,
                latest_completed,
                steps_to_redo,
                lease_orphaned,
            })
        })
        .await;
    match result {
        Ok(state) => Json(ApiResponse::ok(state)),
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
///
/// KT-1134 — the run of a live audit is spared, and a lease left held by a
/// run that could not be settled (`AuditTracker::orphaned_leases`) is released
/// once its rows are: the UI offers this action when a project is stuck.
pub async fn audit_runs_cleanup(State(state): State<AppState>) -> Json<ApiResponse<u64>> {
    let (live, orphaned): (Vec<String>, Vec<String>) = match state.audit_tracker.lock() {
        Ok(t) => (
            t.leased
                .iter()
                .filter(|id| !t.orphaned_leases.contains(*id))
                .cloned()
                .collect(),
            t.orphaned_leases.iter().cloned().collect(),
        ),
        Err(_) => return Json(ApiResponse::err("audit tracker lock poisoned")),
    };
    let result = state
        .db
        .with_conn(move |conn| crate::db::audit_runs::reconcile_running_except(conn, &live))
        .await;
    match result {
        Ok(settled) => {
            if let Ok(mut t) = state.audit_tracker.lock() {
                for project_id in &orphaned {
                    if t.orphaned_leases.contains(project_id) {
                        t.release_lease(project_id);
                        t.clear_progress(project_id);
                        tracing::warn!(
                            "Audit cleanup released the orphaned lease of project {project_id}"
                        );
                    }
                }
            }
            Json(ApiResponse::ok(settled.len() as u64))
        }
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

/// KT-1084 — an audit run's tokens and cost per phase (briefing, audit,
/// validation) and in total. Aggregation of what each phase stored.
pub async fn audit_run_cost(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Json<ApiResponse<crate::models::AuditRunCost>> {
    let result = state
        .db
        .with_read_conn(move |conn| crate::db::audit_run_cost::for_run(conn, &run_id))
        .await;
    match result {
        Ok(Some(cost)) => Json(ApiResponse::ok(cost)),
        Ok(None) => Json(ApiResponse::err("Audit run not found")),
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
    /// Who validated and who opened the validation, from `.kronn.json`
    /// (only while `recorded_validated_at` is set).
    pub recorded_validated_by: Option<String>,
    pub recorded_validation_started_by: Option<String>,
    /// The validation discussion linked to the latest run when that run is
    /// Completed, archived or not: the timeline offers to validate once it
    /// has finished.
    pub latest_validation: Option<AuditTimelineValidation>,
    /// The project's audit lease is held by a run that could not be settled:
    /// every launch is refused until the cleanup action releases it (KT-1134).
    pub lease_orphaned: bool,
    /// Why the server would refuse a validation now (`None` = it accepts):
    /// the UI shows the reason instead of offering the action.
    pub validation_block: Option<super::validate::ValidationBlock>,
}

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AuditTimelineValidation {
    pub discussion_id: String,
    /// The server's verdict, from the durable completion stamp: the one
    /// parser of the signal lives in the backend.
    pub finished: bool,
    pub archived: bool,
    pub completed_at: Option<String>,
    /// Pseudo of the operator whose audit opened the validation.
    pub started_by: Option<String>,
    pub validated_by: Option<String>,
    pub validated_at: Option<String>,
}

/// GET /api/projects/{id}/audit-timeline
pub async fn audit_timeline(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AuditTimelineData>> {
    let lease_orphaned = state
        .audit_tracker
        .lock()
        .map(|t| t.lease_orphaned(&id))
        .unwrap_or(false);
    let audit_running = state
        .audit_tracker
        .lock()
        .map(|t| t.leased.contains(&id))
        .unwrap_or(true);
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
            let state = super::validate::validation_state(conn, &id, audit_running)?;
            let foreign_or_missing = matches!(
                state.blocked,
                Some(
                    super::validate::ValidationBlock::ForeignDiscussion
                        | super::validate::ValidationBlock::DiscussionMissing
                )
            );
            let latest_validation = match (runs.first(), state.discussion_id.clone()) {
                (Some(run), Some(discussion_id))
                    if run.status == "Completed" && !foreign_or_missing =>
                {
                    Some(AuditTimelineValidation {
                        discussion_id,
                        finished: state.finished,
                        archived: state.archived,
                        completed_at: state.completed_at.clone(),
                        started_by: state.started_by.clone(),
                        validated_by: state.validated_by.clone(),
                        validated_at: state.validated_at.clone(),
                    })
                }
                _ => None,
            };
            Ok((project, runs, steps, (latest_validation, state.blocked)))
        })
        .await;
    let (project, runs, steps, (latest_validation, validation_block)) = match result {
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
    let recorded = recorded.unwrap_or_default();
    let validated = recorded.validated_at.is_some();
    let recorded_validated_by = recorded.validated_by.filter(|_| validated);
    let recorded_validation_started_by = recorded.validation_started_by.filter(|_| validated);
    Json(ApiResponse::ok(AuditTimelineData {
        runs,
        steps,
        recorded_audits: recorded.audits,
        recorded_validated_at: recorded.validated_at,
        recorded_validated_by,
        recorded_validation_started_by,
        latest_validation,
        lease_orphaned,
        validation_block,
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

#[cfg(test)]
mod audit_status_tests {
    use super::*;

    /// An agent's bridge token reads the counts, never the recent actions.
    #[tokio::test]
    async fn a_bridge_caller_never_reads_the_recent_actions() {
        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap()),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        {
            let mut tracker = state.audit_tracker.lock().unwrap();
            tracker.start_progress("p1", 16, "full_audit");
            tracker.update_chips("p1", None, None, Some("Bash".into()));
            tracker.set_recent_activity(
                "p1",
                crate::models::AuditRecentActivity {
                    entries: vec![crate::models::AuditActivityEntry {
                        category: crate::models::ActivityCategory::Execute,
                        at: chrono::Utc::now(),
                    }],
                },
            );
        }
        let read = |bridge: bool| {
            let state = state.clone();
            async move {
                let caller = bridge.then(|| {
                    axum::Extension(crate::core::bridge_token::BridgeCaller {
                        token_id: "t".into(),
                        project: Some("p1".into()),
                        own_discussions: Vec::new(),
                        own_run: None,
                        agent: None,
                    })
                });
                audit_status(State(state), Path("p1".into()), caller)
                    .await
                    .0
                    .data
                    .flatten()
                    .unwrap()
            }
        };
        let user = read(false).await;
        assert!(
            user.recent_activity.is_some(),
            "the user's panel reads them"
        );
        let agent = read(true).await;
        assert_eq!(agent.recent_activity, None);
        assert_eq!(agent.current_tool_call_count, Some(1), "the counts stay");
    }
}

#[cfg(test)]
#[path = "run_states_tests.rs"]
mod run_states_tests;
