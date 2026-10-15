//! The audit run, portable by the repository (KT-1199): the writer that
//! projects the local run onto `<docs>/.kronn-audit-run.json`, the read side
//! the UI and the validation gate use when this instance did not run the
//! audit (or the repository is newer), and the rebuild of a local run from a
//! committed file so a colleague can resume it.
//!
//! The file format and its trust rules live in `core::audit_run_state`.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::core::audit_run_state::{
    self as repo_state, Baselines, RepoProbe, RunState, StepState, StepStatus, StepVerdict,
};
use crate::models::{ApiResponse, AuditRun, AuditRunStep};
use crate::AppState;

/// What the writer knows once at the start of a run.
pub(crate) struct RunStateMeta {
    pub started_by: String,
    pub resumed_from: Option<String>,
    pub template_version: Option<String>,
    pub briefing_digest: Option<String>,
}

impl RunStateMeta {
    pub(crate) fn capture(
        project: &std::path::Path,
        started_by: String,
        resumed_from: Option<String>,
    ) -> Self {
        let docs = crate::core::docs_location::DocsRoot::of(project);
        Self {
            started_by,
            resumed_from,
            template_version: crate::core::kronn_state::read(project)
                .and_then(|state| state.template)
                .and_then(|template| template.version),
            briefing_digest: std::fs::read(docs.file(project, "briefing.md"))
                .ok()
                .map(|bytes| repo_state::sha256_hex(&bytes)),
        }
    }
}

/// Projects one run of the local database onto the repository file. The DB
/// rows and the digests this writer holds are authoritative: nothing here
/// ever reads the file back, so an agent editing it mid-run changes nothing
/// and the next step rewrites it (KT-1199 §6).
pub(crate) struct RunStateWriter {
    project: PathBuf,
    run_id: String,
    meta: RunStateMeta,
    /// 1-based step index and the target label of every step of the chain.
    planned: Vec<(u32, String)>,
    /// The digest of each finished step's file, as it was when it finished.
    digests: Mutex<BTreeMap<u32, String>>,
}

impl RunStateWriter {
    pub(crate) fn new(
        project: PathBuf,
        run_id: String,
        meta: RunStateMeta,
        planned: Vec<(u32, String)>,
    ) -> Self {
        Self {
            project,
            run_id,
            meta,
            planned,
            digests: Mutex::new(BTreeMap::new()),
        }
    }

    /// Rewrite the file from the run's rows. `refresh` re-reads every
    /// finished step's file: the end of the run, when the documents are final.
    /// A failure to write never fails the run: it is logged.
    pub(crate) fn sync(&self, conn: &rusqlite::Connection, refresh: bool) {
        if let Err(e) = self.try_sync(conn, refresh) {
            tracing::warn!(
                "Audit run state of {} not written to the repository: {e}",
                self.run_id
            );
        }
    }

    fn try_sync(&self, conn: &rusqlite::Connection, refresh: bool) -> Result<(), String> {
        // A first audit has no docs folder before its template is installed:
        // the file is written once the folder exists, never ahead of it.
        let docs = crate::core::docs_location::DocsRoot::of(&self.project).dir(&self.project);
        if !docs.is_dir() {
            return Ok(());
        }
        let run = crate::db::audit_runs::get_by_id(conn, &self.run_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "run row not found".to_string())?;
        let rows = crate::db::audit_runs::list_audit_steps(conn, &self.run_id)
            .map_err(|e| e.to_string())?;
        let baseline =
            crate::db::audit_runs::adr_baseline(conn, &self.run_id).map_err(|e| e.to_string())?;
        let state = {
            let mut digests = self
                .digests
                .lock()
                .map_err(|_| "digest lock poisoned".to_string())?;
            self.build(&run, &rows, baseline, &mut digests, refresh)
        };
        let text = state.render()?;
        let path = repo_state::state_path(&self.project);
        if std::fs::read_to_string(&path).is_ok_and(|current| current == text) {
            return Ok(());
        }
        repo_state::write(&self.project, &state)
    }

    fn build(
        &self,
        run: &AuditRun,
        rows: &[AuditRunStep],
        baseline: Option<crate::core::adr::StartOfRun>,
        digests: &mut BTreeMap<u32, String>,
        refresh: bool,
    ) -> RunState {
        let mut steps = BTreeMap::new();
        let mut updated = run.ended_at.unwrap_or(run.started_at).max(run.started_at);
        let mut entries: Vec<(u32, String)> = self.planned.clone();
        for row in rows {
            if !entries.iter().any(|(index, _)| *index == row.step_index) {
                entries.push((row.step_index, row.file_label.clone()));
            }
        }
        entries.sort();
        for (index, label) in entries {
            let row = rows.iter().find(|row| row.step_index == index);
            let status = row.map(step_status).unwrap_or(StepStatus::Pending);
            if let Some(ended) = row.and_then(|row| row.ended_at) {
                updated = updated.max(ended);
            }
            let digest = if status.is_done() && repo_state::is_file_target(&label) {
                let fresh = || repo_state::file_digest(&self.project, &label);
                let kept = if refresh {
                    fresh()
                } else {
                    digests.get(&index).cloned().or_else(fresh)
                };
                match &kept {
                    Some(digest) => digests.insert(index, digest.clone()),
                    None => digests.remove(&index),
                };
                kept
            } else {
                digests.remove(&index);
                None
            };
            steps.insert(
                repo_state::step_key(index),
                StepState {
                    status,
                    target: label,
                    digest,
                    by: row.and_then(|row| row.accepted_by.clone()),
                },
            );
        }
        RunState {
            schema: repo_state::RUN_STATE_SCHEMA,
            run_id: run.id.clone(),
            kind: run.kind.clone(),
            status: run.status.clone(),
            started_at: run.started_at.to_rfc3339(),
            ended_at: run.ended_at.map(|at| at.to_rfc3339()),
            updated_at: updated.to_rfc3339(),
            started_by: self.meta.started_by.clone(),
            resumed_from: self.meta.resumed_from.clone(),
            kronn_version: env!("CARGO_PKG_VERSION").to_string(),
            template_version: self.meta.template_version.clone(),
            briefing_digest: self.meta.briefing_digest.clone(),
            total_steps: steps.len() as u32,
            baselines: baseline
                .map(|baseline| Baselines::from_text(&baseline.decisions, &baseline.records)),
            steps,
        }
    }
}

fn step_status(row: &AuditRunStep) -> StepStatus {
    if row.accepted_at.is_some() {
        StepStatus::Accepted
    } else if row.ended_at.is_some() && row.cli_success {
        if row.unchanged {
            StepStatus::Unchanged
        } else {
            StepStatus::Done
        }
    } else if row.ended_at.is_some() {
        StepStatus::Failed
    } else {
        StepStatus::Pending
    }
}

/// Whether the repository's run is the project's state rather than the
/// local database's: this instance has no run, or the file describes a
/// different run that was updated after the local one ended. A run this
/// instance is still running is never superseded.
pub(crate) fn repo_is_newer(state: &RunState, local: Option<&AuditRun>) -> bool {
    let Some(local) = local else {
        return true;
    };
    if local.id == state.run_id || local.status == "Running" {
        return false;
    }
    let updated = DateTime::parse_from_rfc3339(&state.updated_at).map(|at| at.with_timezone(&Utc));
    let local_at = local.ended_at.unwrap_or(local.started_at);
    updated.is_ok_and(|updated| updated > local_at)
}

/// What the UI shows of an audit run that the repository describes.
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct RepoAuditRun {
    pub run_id: String,
    pub kind: String,
    /// The run's own status in the file: `Running` means an interruption or
    /// a run still going on another machine.
    pub status: String,
    pub started_by: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub updated_at: String,
    pub total_steps: u32,
    /// Finished steps whose produced file still matches.
    pub done_steps: Vec<u32>,
    /// Failed and pending steps: what a resume runs.
    pub resume_steps: Vec<u32>,
    /// Finished steps whose file changed since: « à revérifier », replayed.
    pub recheck_steps: Vec<u32>,
    /// The file is the committed one: a resume or a validation may rely on it.
    pub committed: bool,
    /// This is the project's state: no local run, or the repository is newer.
    pub newer_than_local: bool,
    /// A committed, unfinished run with steps left to run: can be resumed here.
    pub resumable: bool,
    pub briefing_changed: bool,
    pub td_total: u32,
    pub td_settled: u32,
    pub validated_at: Option<String>,
    pub validated_by: Option<String>,
}

/// The repository's run of `project`, against the local latest run.
pub(crate) fn repo_view(
    project: &std::path::Path,
    local_latest: Option<&AuditRun>,
) -> Option<RepoAuditRun> {
    let RepoProbe::Found { state, committed } = repo_state::probe(project) else {
        return None;
    };
    let assessed = repo_state::assess(project, &state);
    let indexes = |wanted: &dyn Fn(&repo_state::StepAssessment) -> bool| -> Vec<u32> {
        assessed
            .iter()
            .filter(|step| wanted(step))
            .map(|step| step.index)
            .collect()
    };
    let done_steps = indexes(&|s| s.verdict == StepVerdict::Trusted);
    let resume_steps =
        indexes(&|s| matches!(s.verdict, StepVerdict::Failed | StepVerdict::Pending));
    let recheck_steps = indexes(&|s| s.verdict == StepVerdict::Recheck);
    let items = crate::core::td_review::validation_items(project);
    let td_settled = items
        .iter()
        .filter(|item| crate::core::td_review::is_settled(item.state))
        .count() as u32;
    let kronn = crate::core::kronn_state::read(project);
    let briefing_changed = state.briefing_digest.as_deref().is_some_and(|recorded| {
        let docs = crate::core::docs_location::DocsRoot::of(project);
        std::fs::read(docs.file(project, "briefing.md"))
            .map(|bytes| repo_state::sha256_hex(&bytes) != recorded)
            .unwrap_or(true)
    });
    let unfinished = state.status != "Completed";
    Some(RepoAuditRun {
        newer_than_local: repo_is_newer(&state, local_latest),
        resumable: committed
            && unfinished
            && (!resume_steps.is_empty() || !recheck_steps.is_empty()),
        run_id: state.run_id.clone(),
        kind: state.kind.clone(),
        status: state.status.clone(),
        started_by: state.started_by.clone(),
        started_at: state.started_at.clone(),
        ended_at: state.ended_at.clone(),
        updated_at: state.updated_at.clone(),
        total_steps: state.total_steps,
        done_steps,
        resume_steps,
        recheck_steps,
        committed,
        briefing_changed,
        td_total: items.len() as u32,
        td_settled,
        validated_at: kronn.as_ref().and_then(|k| k.validated_at.clone()),
        validated_by: kronn.as_ref().and_then(|k| k.validated_by.clone()),
    })
}

/// The committed run a colleague finished, checked against the same gates as
/// a local run (KT-1199 §4).
pub(crate) struct RepoValidation {
    pub state: RunState,
}

/// `Err` carries the reason the repository state cannot validate the project.
pub(crate) fn repo_validation(project: &std::path::Path) -> Result<RepoValidation, RepoRefusal> {
    let RepoProbe::Found { state, committed } = repo_state::probe(project) else {
        return Err(RepoRefusal::NoRun);
    };
    if !committed {
        return Err(RepoRefusal::NotCommitted);
    }
    if state.status != "Completed" {
        return Err(RepoRefusal::NotCompleted(state.status.clone()));
    }
    let to_verify: Vec<u32> = repo_state::assess(project, &state)
        .iter()
        .filter(|step| step.verdict != StepVerdict::Trusted)
        .map(|step| step.index)
        .collect();
    if !to_verify.is_empty() {
        return Err(RepoRefusal::StepsToRecheck(to_verify));
    }
    let td_items = crate::core::td_review::validation_items(project);
    if !crate::core::td_review::gate(&td_items).met {
        return Err(RepoRefusal::TdOpen);
    }
    Ok(RepoValidation { state: *state })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RepoRefusal {
    NoRun,
    NotCommitted,
    NotCompleted(String),
    StepsToRecheck(Vec<u32>),
    TdOpen,
}

impl RepoRefusal {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::NoRun => "No audit run recorded for this project on this instance and no audit run state in the repository — run the audit first.".into(),
            Self::NotCommitted => format!(
                "The audit run state of the repository is not committed — commit {} (or fetch the colleague's commit) before validating from the repository.",
                repo_state::RUN_STATE_FILENAME
            ),
            Self::NotCompleted(status) => format!(
                "The audit run in the repository is {status} — only a Completed run can be validated (resume or re-run it first)."
            ),
            Self::StepsToRecheck(steps) => format!(
                "Steps to re-verify: their files changed since the audit ({}) — resume the audit to re-verify them before validating.",
                steps.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
            ),
            Self::TdOpen => "Without the audit run on this instance, the repository state validates only once every Critical and High tech-debt sheet is settled.".into(),
        }
    }
}

/// After a validation: the human accepted the documents as they stand, so
/// the run's digests follow them (the validation may have settled sheets
/// that are step targets). Only the run that was validated is touched.
pub(crate) fn refresh_validated_digests(project: &std::path::Path, run_id: &str) {
    let RepoProbe::Found { state, .. } = repo_state::probe(project) else {
        return;
    };
    if state.run_id != run_id {
        return;
    }
    let mut state = *state;
    for step in state.steps.values_mut() {
        if step.status.is_done() && repo_state::is_file_target(&step.target) {
            step.digest = repo_state::file_digest(project, &step.target);
        }
    }
    if let Err(e) = repo_state::write(project, &state) {
        tracing::warn!("Audit run state not refreshed after validation: {e}");
    }
}

/// Rebuild, from the committed file, the local run a colleague left
/// interrupted, so a normal resume continues it (KT-1199 §3). A run this
/// instance already has is left as is. The steps whose produced file still
/// matches are inherited; the pending, failed and changed ones are replayed.
/// Returns the number of steps inherited.
pub(crate) fn adopt_repo_run(
    conn: &rusqlite::Connection,
    project_id: &str,
    project: &std::path::Path,
    run_id: &str,
) -> Result<u32, String> {
    if crate::db::audit_runs::get_by_id(conn, run_id)
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Ok(0);
    }
    let not_found = || format!("resume_run_id {run_id} not found");
    let RepoProbe::Found { state, committed } = repo_state::probe(project) else {
        return Err(not_found());
    };
    if state.run_id != run_id {
        return Err(not_found());
    }
    if !committed {
        return Err(format!(
            "resume_run_id {run_id}: the audit run state of the repository is not committed — commit {} (or fetch it) to resume from it",
            repo_state::RUN_STATE_FILENAME
        ));
    }
    let latest = crate::db::audit_runs::list_recent(conn, project_id, 1)
        .map_err(|e| e.to_string())?
        .into_iter()
        .next();
    if !repo_is_newer(&state, latest.as_ref()) {
        return Err(format!(
            "resume_run_id {run_id} is not the project's most recent run — a newer attempt supersedes it; launch a fresh audit instead"
        ));
    }
    if state.status == "Completed" {
        return Err(format!(
            "resume_run_id {run_id} is not resumable (status: Completed)"
        ));
    }
    let started = DateTime::parse_from_rfc3339(&state.started_at)
        .map_err(|e| format!("started_at of the repository run: {e}"))?
        .with_timezone(&Utc);
    let ended = DateTime::parse_from_rfc3339(&state.updated_at)
        .map(|at| at.with_timezone(&Utc))
        .unwrap_or(started)
        .max(started);
    let inherited: Vec<crate::db::audit_runs::AdoptedStep> = repo_state::assess(project, &state)
        .into_iter()
        .filter(|step| step.status.is_done() && step.verdict == StepVerdict::Trusted)
        .map(|step| crate::db::audit_runs::AdoptedStep {
            index: step.index,
            label: step.target.clone(),
            unchanged: step.status == StepStatus::Unchanged,
            accepted_by: (step.status == StepStatus::Accepted).then(|| {
                state
                    .step(step.index)
                    .and_then(|s| s.by.clone())
                    .unwrap_or_else(|| state.started_by.clone())
            }),
        })
        .collect();
    let baseline = state
        .baselines
        .as_ref()
        .map(|b| crate::core::adr::StartOfRun {
            decisions: b.decisions_text(),
            records: b.records_text(),
        });
    crate::db::audit_runs::adopt_repo_run(
        conn,
        run_id,
        project_id,
        &state.kind,
        started,
        ended,
        &format!(
            "rebuilt from the repository (audit started by {})",
            state.started_by
        ),
        &inherited,
        baseline.as_ref(),
    )
    .map_err(|e| e.to_string())?;
    Ok(inherited.len() as u32)
}

/// GET /api/projects/{id}/audit-repo-state
///
/// The audit run the repository describes (a colleague's run after a fetch
/// or a rebase), with whether it is the project's state here. `None` when
/// the repository carries no readable run state.
pub async fn get_repo_audit_state(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Option<RepoAuditRun>>> {
    let project = match state
        .db
        .with_conn({
            let id = id.clone();
            move |conn| crate::db::projects::get_project(conn, &id)
        })
        .await
    {
        Ok(Some(project)) => project,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("db: {e}"))),
    };
    let latest = match state
        .db
        .with_read_conn(move |conn| {
            Ok(crate::db::audit_runs::list_recent(conn, &id, 1)?
                .into_iter()
                .next())
        })
        .await
    {
        Ok(latest) => latest,
        Err(e) => return Json(ApiResponse::err(format!("db: {e}"))),
    };
    let path = crate::core::scanner::resolve_host_path(&project.path);
    match tokio::task::spawn_blocking(move || repo_view(&path, latest.as_ref())).await {
        Ok(view) => Json(ApiResponse::ok(view)),
        Err(e) => Json(ApiResponse::err(format!(
            "repository state task failed: {e}"
        ))),
    }
}

#[cfg(test)]
#[path = "run_state_tests.rs"]
mod tests;
