// Audit lifecycle markers: validate (post-audit user signoff) and
// mark-bootstrapped (post-bootstrap-discussion finalization).
//
// 0.8.4 — state is recorded in `docs/.kronn.json` (see
// `core::kronn_state`). Previously we injected `<!-- KRONN:VALIDATED:date -->`
// and `<!-- KRONN:BOOTSTRAPPED:date -->` HTML comments into
// `docs/AGENTS.md`; those polluted the agent prompt and were easy to
// remove "as noise". `detect_audit_status` still reads the legacy markers
// so existing projects keep their badge without re-running anything.

use axum::{
    extract::{Path, State},
    Json,
};

use rusqlite::OptionalExtension;

use crate::core::{kronn_state, scanner};
use crate::db::audit_validation as record;
use crate::models::*;
use crate::AppState;

/// Whether a validation discussion has signalled completion in its messages:
/// some Agent message of the main channel ends on the terminal signal. A
/// later message never undoes it (KT-1122) — the durable stamp on the run
/// (`db::audit_validation`) is the same fact recorded at the time.
pub(crate) fn validation_discussion_finished(discussion: &Discussion) -> bool {
    discussion.messages.iter().any(|message| {
        matches!(message.role, MessageRole::Agent)
            && matches!(message.channel, MessageChannel::Main)
            && record::is_validation_signal(&message.content)
    })
}

/// Why validating the project's latest audit run is not possible now. The
/// timeline and the discussion banners show this reason instead of offering
/// an action the server would refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ValidationBlock {
    /// This instance never ran the audit (a P2P peer, another clone) and the
    /// repository carries no run state either: read-only unless the
    /// repository carries a committed, Completed run state that passes the
    /// gates (KT-1199) — then it validates from that state.
    NoLocalRun,
    AuditRunning,
    RunNotCompleted,
    NoLinkedDiscussion,
    DiscussionMissing,
    ForeignDiscussion,
    NotFinished,
    /// The run state of the repository is not committed (KT-1199).
    RepoRunNotCommitted,
    /// A finished step's file changed since the repository's run (KT-1199).
    RepoStepsToRecheck,
    /// Critical or High tech-debt sheets are still open (KT-1199).
    RepoTdOpen,
}

impl ValidationBlock {
    pub(crate) fn message(self, run_status: Option<&str>) -> String {
        match self {
            Self::NoLocalRun => "No audit run recorded for this project on this instance and no audit run state in the repository — read-only until the repository carries a committed, Completed run state that passes the gates (run the audit here, or fetch the commit of the colleague who ran it).".into(),
            Self::AuditRunning => "An audit is currently running on this project — validate once it has finished.".into(),
            Self::RunNotCompleted => format!(
                "The latest audit run is {} — only a Completed run can be validated (resume or re-run it first).",
                run_status.unwrap_or("unknown")
            ),
            Self::NoLinkedDiscussion => "The latest completed run carries no linked validation discussion — only the discussion the audit itself created (and linked in the same transaction) can validate it, unless every Critical and High tech-debt sheet is settled.".into(),
            Self::DiscussionMissing => "The linked validation discussion no longer exists — re-run the audit.".into(),
            Self::ForeignDiscussion => "The linked validation discussion belongs to another project — refusing.".into(),
            Self::RepoRunNotCommitted => super::run_state::RepoRefusal::NotCommitted.message(),
            Self::RepoStepsToRecheck => "Some steps of the audit run in the repository are « à revérifier »: their files changed since the run — resume the audit to re-verify them before validating.".into(),
            Self::RepoTdOpen => super::run_state::RepoRefusal::TdOpen.message(),
            Self::NotFinished => "The validation discussion has not finished — the agent must end on KRONN:VALIDATION_COMPLETE, or every Critical and High tech-debt sheet must be settled, before the project can be marked validated.".into(),
        }
    }
}

/// Where a validation state comes from (KT-1199): the run this instance
/// recorded, or the committed run state of the repository (a colleague's run
/// after a fetch or a rebase).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ValidationSource {
    Local,
    Repo,
}

/// The validation of a project's latest audit run, and what can be done.
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ValidationState {
    pub project_id: String,
    pub run_id: Option<String>,
    pub run_status: Option<String>,
    /// The discussion linked to the latest run (never a title match).
    pub discussion_id: Option<String>,
    pub archived: bool,
    /// The linked discussion has signalled completion; never undone.
    pub finished: bool,
    pub completed_at: Option<String>,
    pub completed_message_id: Option<String>,
    /// Pseudo of the operator whose audit opened the validation.
    pub started_by: Option<String>,
    pub validated_by: Option<String>,
    pub validated_at: Option<String>,
    pub source: ValidationSource,
    /// The server accepts a validation now (the tech-debt sheet path, which
    /// reads the filesystem, is decided by the POST itself).
    pub can_validate: bool,
    pub blocked: Option<ValidationBlock>,
}

impl ValidationState {
    fn empty(project_id: &str, blocked: ValidationBlock) -> Self {
        Self {
            project_id: project_id.to_string(),
            run_id: None,
            run_status: None,
            discussion_id: None,
            archived: false,
            finished: false,
            completed_at: None,
            completed_message_id: None,
            started_by: None,
            validated_by: None,
            validated_at: None,
            source: ValidationSource::Local,
            can_validate: false,
            blocked: Some(blocked),
        }
    }
}

/// The single read-side gate of the timeline and the validation-state
/// endpoint; the POST below applies the same durable completion.
pub(crate) fn validation_state(
    conn: &rusqlite::Connection,
    project_id: &str,
    audit_running: bool,
) -> anyhow::Result<ValidationState> {
    let Some(run) = crate::db::audit_runs::list_recent(conn, project_id, 1)?
        .into_iter()
        .next()
    else {
        return Ok(ValidationState::empty(
            project_id,
            ValidationBlock::NoLocalRun,
        ));
    };
    let stored = record::record(conn, &run.id)?;
    let mut state = ValidationState::empty(project_id, ValidationBlock::NoLinkedDiscussion);
    state.run_id = Some(run.id.clone());
    state.run_status = Some(run.status.clone());
    state.started_by = stored.started_by;
    state.validated_by = stored.validated_by;
    state.validated_at = stored.validated_at;
    state.discussion_id = run.validation_discussion_id.clone();

    let linked: Option<(Option<String>, bool)> = match run.validation_discussion_id.as_deref() {
        Some(disc_id) => conn
            .query_row(
                "SELECT project_id, archived FROM discussions WHERE id = ?1",
                [disc_id],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, bool>(1)?)),
            )
            .optional()?,
        None => None,
    };
    if let (Some(disc_id), Some((_, archived))) =
        (run.validation_discussion_id.as_deref(), linked.as_ref())
    {
        state.archived = *archived;
        if let Some((message_id, at)) = record::effective_completion(conn, &run.id, disc_id)? {
            state.finished = true;
            state.completed_message_id = Some(message_id);
            state.completed_at = Some(at);
        }
    }
    state.blocked = if audit_running {
        Some(ValidationBlock::AuditRunning)
    } else if run.status != "Completed" {
        Some(ValidationBlock::RunNotCompleted)
    } else if run.validation_discussion_id.is_none() {
        Some(ValidationBlock::NoLinkedDiscussion)
    } else {
        match &linked {
            None => Some(ValidationBlock::DiscussionMissing),
            Some((owner, _)) if owner.as_deref() != Some(project_id) => {
                Some(ValidationBlock::ForeignDiscussion)
            }
            Some(_) if !state.finished => Some(ValidationBlock::NotFinished),
            Some(_) => None,
        }
    };
    state.can_validate = state.blocked.is_none();
    Ok(state)
}

fn audit_running(state: &AppState, project_id: &str) -> bool {
    state
        .audit_tracker
        .lock()
        .map(|tracker| tracker.leased.contains(project_id))
        .unwrap_or(true)
}

/// The pseudo recorded as who started or validated, from this instance.
pub(crate) async fn operator_pseudo(state: &AppState) -> String {
    let config = state.config.read().await;
    config
        .server
        .pseudo
        .as_deref()
        .map(str::trim)
        .filter(|pseudo| !pseudo.is_empty())
        .unwrap_or("anonymous")
        .to_string()
}

/// The validation state the repository describes, when it is the project's
/// state here: this instance has no run, or the committed one is newer
/// (KT-1199). `None` when the local run stays authoritative.
pub(crate) fn repo_validation_state(
    project_path: &std::path::Path,
    project_id: &str,
    latest: Option<&AuditRun>,
    audit_running: bool,
) -> Option<ValidationState> {
    use super::run_state::{repo_is_newer, repo_validation, RepoRefusal};
    let crate::core::audit_run_state::RepoProbe::Found { state, .. } =
        crate::core::audit_run_state::probe(project_path)
    else {
        return None;
    };
    if !repo_is_newer(&state, latest) {
        return None;
    }
    let kronn = kronn_state::read(project_path);
    let mut found = ValidationState::empty(project_id, ValidationBlock::NoLocalRun);
    found.source = ValidationSource::Repo;
    found.run_id = Some(state.run_id.clone());
    found.run_status = Some(state.status.clone());
    found.started_by = Some(state.started_by.clone());
    found.completed_at = state.ended_at.clone();
    found.validated_by = kronn.as_ref().and_then(|k| k.validated_by.clone());
    found.validated_at = kronn.as_ref().and_then(|k| k.validated_at.clone());
    found.blocked = if audit_running {
        Some(ValidationBlock::AuditRunning)
    } else {
        match repo_validation(project_path) {
            Ok(_) => None,
            Err(RepoRefusal::NoRun) => Some(ValidationBlock::NoLocalRun),
            Err(RepoRefusal::NotCommitted) => Some(ValidationBlock::RepoRunNotCommitted),
            Err(RepoRefusal::NotCompleted(_)) => Some(ValidationBlock::RunNotCompleted),
            Err(RepoRefusal::StepsToRecheck(_)) => Some(ValidationBlock::RepoStepsToRecheck),
            Err(RepoRefusal::TdOpen) => Some(ValidationBlock::RepoTdOpen),
        }
    };
    found.can_validate = found.blocked.is_none();
    Some(found)
}

/// GET /api/projects/{id}/validation-state
pub async fn get_validation_state(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<ValidationState>> {
    let running = audit_running(&state, &id);
    let local = {
        let id = id.clone();
        state
            .db
            .with_read_conn(move |conn| {
                let found = validation_state(conn, &id, running)?;
                let latest = crate::db::audit_runs::list_recent(conn, &id, 1)?
                    .into_iter()
                    .next();
                Ok((found, latest))
            })
            .await
    };
    let (local, latest) = match local {
        Ok(found) => found,
        Err(e) => return Json(ApiResponse::err(format!("db: {e}"))),
    };
    // The repository may describe a newer (or the only) run of the project.
    let project = state
        .db
        .with_conn({
            let id = id.clone();
            move |conn| crate::db::projects::get_project(conn, &id)
        })
        .await
        .ok()
        .flatten();
    let Some(project) = project else {
        return Json(ApiResponse::ok(local));
    };
    let path = scanner::resolve_host_path(&project.path);
    let repo = tokio::task::spawn_blocking(move || {
        repo_validation_state(&path, &id, latest.as_ref(), running)
    })
    .await
    .ok()
    .flatten();
    Json(ApiResponse::ok(repo.unwrap_or(local)))
}

/// POST /api/projects/:id/validate-audit
/// Records `validated_at` in `docs/.kronn.json` and refreshes the project's audit status.
pub async fn validate_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    // The Validated badge asserts a completed pipeline + a FINISHED human
    // validation — earned, never forged (Codex A5 v2). Gate:
    //   1. the project's LATEST run is Completed;
    //   2. it carries the durable `validation_discussion_id` link (076 —
    //      written in the same transaction as the Completed status; a
    //      manually created look-alike discussion satisfies nothing);
    //   3. that discussion's last Agent message ends on the terminal
    //      signal KRONN:VALIDATION_COMPLETE (same shared parser as the
    //      UI — a POST fired before the validation finished is refused).
    // The project lease is held from the gate through mark_validated so no
    // concurrent run can mutate docs between the check and the write.
    let lease =
        match crate::api::audit::docs_write::DocsWriteLease::try_acquire(&state, &project.id) {
            Ok(lease) => lease,
            Err(error) => return Json(ApiResponse::err(error)),
        };

    // KT-982 — settling every Critical and High sheet stands in for the
    // validation discussion's terminal marker.
    let td_gate_path = scanner::resolve_host_path(&project.path);
    let td_items = tokio::task::spawn_blocking(move || {
        crate::core::td_review::validation_items(&td_gate_path)
    })
    .await
    .unwrap_or_default();
    let td_gate = crate::core::td_review::gate(&td_items);
    let td_gate_met = td_gate.blocking_total > 0 && td_gate.met;

    // A cross-agent dispute a human has not answered is an open question on
    // the audit itself: the project cannot be validated over it.
    let pid = project.id.clone();
    let pending = state
        .db
        .with_read_conn(move |conn| crate::db::audit_cross::pending_disputes(conn, &pid))
        .await;
    let pending = match pending {
        Ok(pending) => pending,
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };
    if let Some(refusal) = open_disputes_refusal(&td_items, &pending) {
        return Json(ApiResponse::err(refusal));
    }

    // KT-1199 — the committed run state of the repository stands in for the
    // run this instance does not have (or has older): same gates — the run
    // Completed, every step's file as it was produced, every Critical and High
    // sheet settled, then the docs gates below.
    let repo_run: Option<crate::core::audit_run_state::RunState> = {
        let pid = project.id.clone();
        let latest = state
            .db
            .with_read_conn(move |conn| {
                Ok(crate::db::audit_runs::list_recent(conn, &pid, 1)?
                    .into_iter()
                    .next())
            })
            .await
            .unwrap_or(None);
        let path = scanner::resolve_host_path(&project.path);
        let candidate =
            tokio::task::spawn_blocking(move || match crate::core::audit_run_state::probe(&path) {
                crate::core::audit_run_state::RepoProbe::Found { state, .. }
                    if super::run_state::repo_is_newer(&state, latest.as_ref()) =>
                {
                    Some(path)
                }
                _ => None,
            })
            .await
            .ok()
            .flatten();
        match candidate {
            None => None,
            Some(path) => {
                let verdict =
                    tokio::task::spawn_blocking(move || super::run_state::repo_validation(&path))
                        .await;
                match verdict {
                    Ok(Ok(validation)) => Some(validation.state),
                    Ok(Err(refusal)) => return Json(ApiResponse::err(refusal.message())),
                    Err(e) => return Json(ApiResponse::err(format!("Task failed: {e}"))),
                }
            }
        }
    };
    let from_repo = repo_run.is_some();

    let pid = project.id.clone();
    let cancel_registry = state.cancel_registry.clone();
    let gate = state
        .db
        .with_conn(move |conn| {
            let latest = crate::db::audit_runs::list_recent(conn, &pid, 1)?
                .into_iter()
                .next();
            let mut disc = match latest
                .as_ref()
                .and_then(|r| r.validation_discussion_id.clone())
            {
                Some(disc_id) => crate::db::discussions::get_discussion(conn, &disc_id)?,
                None => None,
            };
            let is_active = |discussion: &Discussion| -> anyhow::Result<bool> {
                Ok(discussion.awaiting_agent
                    || crate::db::agent_dispatch::has_active_for_discussion(conn, &discussion.id)?
                    || cancel_registry
                        .lock()
                        .map(|map| map.contains_key(&discussion.id))
                        .unwrap_or(true))
            };
            let mut validation_agent_active =
                disc.as_ref().map(is_active).transpose()?.unwrap_or(false);
            // Durable completion (KT-1122): the stamp written with the signal,
            // else the earliest clean signal in the messages — later messages
            // never undo it.
            let completion_of =
                |run: Option<&AuditRun>, discussion: Option<&Discussion>| match (run, discussion) {
                    (Some(run), Some(discussion)) => {
                        record::effective_completion(conn, &run.id, &discussion.id)
                    }
                    _ => Ok(None),
                };
            let linked_is_finished = completion_of(latest.as_ref(), disc.as_ref())?.is_some();
            if disc.as_ref().is_some_and(|discussion| discussion.archived) && !linked_is_finished {
                let mut statement = conn.prepare(
                    "SELECT id FROM discussions
                     WHERE project_id=?1 AND archived=0 AND title LIKE 'Validation audit%'
                       AND julianday(created_at) >= julianday(?2)
                     ORDER BY updated_at DESC, rowid DESC",
                )?;
                let candidate_ids = statement
                    .query_map(
                        rusqlite::params![
                            pid,
                            latest.as_ref().map(|run| run.started_at.to_rfc3339())
                        ],
                        |row| row.get::<_, String>(0),
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                drop(statement);
                for candidate_id in candidate_ids {
                    let Some(candidate) =
                        crate::db::discussions::get_discussion(conn, &candidate_id)?
                    else {
                        continue;
                    };
                    if validation_discussion_finished(&candidate) {
                        validation_agent_active |= is_active(&candidate)?;
                        if let Some(run) = latest.as_ref() {
                            crate::db::audit_runs::set_validation_discussion(
                                conn,
                                &run.id,
                                &candidate.id,
                            )?;
                            if let Some((message_id, at)) =
                                record::first_signal(conn, &candidate.id)?
                            {
                                record::record_completion(conn, &candidate.id, &message_id, &at)?;
                            }
                        }
                        disc = Some(candidate);
                        break;
                    }
                }
            }
            let documents = latest
                .as_ref()
                .map(|run| crate::db::audit_documents::get(conn, &run.id))
                .transpose()?
                .flatten();
            let finished = completion_of(latest.as_ref(), disc.as_ref())?;
            Ok((latest, disc, validation_agent_active, documents, finished))
        })
        .await;
    let (optimize_documents, documents, run_id) = match gate {
        _ if from_repo => {
            let run = repo_run.as_ref().expect("repo run when from_repo");
            (run.kind == "Full", None, run.run_id.clone())
        }
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
        Ok((latest, linked_disc, validation_agent_active, documents, finished)) => {
            // No local run and no usable repository state (`from_repo` is
            // handled above): read-only, there is nothing to validate.
            let Some(run) = latest else {
                return Json(ApiResponse::err(ValidationBlock::NoLocalRun.message(None)));
            };
            if run.status != "Completed" {
                return Json(ApiResponse::err(format!(
                    "The latest audit run is {} — only a Completed run can be validated (resume or re-run it first).",
                    run.status
                )));
            }
            if validation_agent_active {
                return Json(ApiResponse::err(
                    "The validation agent is still active or queued — wait for it to finish before validating.",
                ));
            }
            if linked_disc
                .as_ref()
                .is_some_and(|disc| disc.project_id.as_deref() != Some(project.id.as_str()))
            {
                return Json(ApiResponse::err(
                    "The linked validation discussion belongs to another project — refusing.",
                ));
            }
            if !td_gate_met {
                let Some(disc) = linked_disc else {
                    return Json(ApiResponse::err(
                        "The latest completed run carries no linked validation discussion — only the discussion the audit itself created (and linked in the same transaction) can validate it, unless every Critical and High tech-debt sheet is settled.",
                    ));
                };
                if disc.project_id.as_deref() != Some(project.id.as_str()) {
                    return Json(ApiResponse::err(
                        "The linked validation discussion belongs to another project — refusing.",
                    ));
                }
                if finished.is_none() {
                    return Json(ApiResponse::err(
                        "The validation discussion has not finished — the agent must end on KRONN:VALIDATION_COMPLETE, or every Critical and High tech-debt sheet must be settled, before the project can be marked validated.",
                    ));
                }
            }
            (run.kind == "Full", documents, run.id)
        }
    };

    // Who opened this validation and who validates it (KT-1122): recorded on
    // the run and in `docs/.kronn.json`, so a clone shows who signed off.
    let by = operator_pseudo(&state).await;
    let started_by = match repo_run.as_ref() {
        Some(run) => Some(run.started_by.clone()),
        None => {
            let run_id = run_id.clone();
            state
                .db
                .with_read_conn(move |conn| record::record(conn, &run_id))
                .await
                .ok()
                .and_then(|stored| stored.started_by)
        }
    };
    let validated_run_id = run_id.clone();
    // The rows this instance holds of the run it validates: the source the
    // run's digests are rebuilt from afterwards (the repository's own run is
    // rebuilt from its committed copy).
    let local_rows = if from_repo {
        None
    } else {
        let run_id = run_id.clone();
        state
            .db
            .with_read_conn(move |conn| {
                let Some(run) = crate::db::audit_runs::get_by_id(conn, &run_id)? else {
                    return Ok(None);
                };
                Ok(Some(super::run_state::LocalRunRows {
                    steps: crate::db::audit_runs::list_audit_steps(conn, &run_id)?,
                    baseline: crate::db::audit_runs::adr_baseline(conn, &run_id)?,
                    run,
                }))
            })
            .await
            .ok()
            .flatten()
    };

    let by_for_file = by.clone();
    super::full::stop_validation_agents(&state, &project.id).await;
    let validate_result = lease.blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let index_file = scanner::detect_docs_entry(&project_path);

        if !index_file.exists() {
            return Err(format!(
                "{} not found — run the audit first",
                crate::core::docs_location::DocsRoot::of(&project_path).path("AGENTS.md")
            ));
        }

        let report = if optimize_documents {
            crate::core::document_optimization::analyze_and_write(&project_path)?
        } else {
            crate::core::document_optimization::analyze(&project_path)?
        };
        if optimize_documents {
            let blocking: Vec<String> = report
                .blocking_diagnostics()
                .map(|d| format!("{}: {} ({})", d.code, d.message, d.path))
                .collect();
            if !blocking.is_empty() {
                return Err(format!(
                    "Documentary validation failed; KRONN:VALIDATION_COMPLETE is blocked: {}",
                    blocking.join("; ")
                ));
            }
        }
        let unresolved = unresolved_unknowns(&project_path, &report);
        if !unresolved.is_empty() {
            return Err(format!(
                "Unresolved unknowns remain (`{{{{...}}}}` or `<!-- TODO: ... -->`); the validation must settle them before the project is validated: {}",
                unresolved.join(", ")
            ));
        }

        // From the repository there is no per-run fingerprint set: every
        // step's file was verified against its committed digest instead.
        if !from_repo {
            crate::core::audit_documents::verify(&project_path, documents.as_deref())?;
        }
        crate::core::citation_drift::record_all(&project_path)?;
        // The docs nothing anchors are vouched for too (KT-1319); a failed
        // stamp only queues them for a re-read, it never refuses the validation.
        if let Err(e) = super::revalidate::stamp_validated_rereads(&project_path) {
            tracing::warn!("Validation re-read stamps not recorded: {e}");
        }
        // A failed record never refuses the validation: the docs simply read
        // as not yet human-validated.
        if let Err(e) = crate::core::doc_provenance::record_all(
            &project_path,
            crate::core::doc_provenance::ValidationReason::AuditValidation,
        ) {
            tracing::warn!("Human-validation provenance not recorded: {e}");
        }
        kronn_state::mark_validated_by(&project_path, &by_for_file, started_by.as_deref())?;
        // The run's digests follow the documents the human just validated.
        super::run_state::refresh_validated_digests(&project_path, &validated_run_id, local_rows);
        Ok(())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = validate_result {
        return Json(ApiResponse::err(e));
    }
    let at = chrono::Utc::now().to_rfc3339();
    if let Err(e) = state
        .db
        .with_conn(move |conn| record::record_validated(conn, &run_id, &by, &at))
        .await
    {
        tracing::warn!("Validated in .kronn.json but not on the run row: {e}");
    }

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

/// Why validation is refused while dispute cards wait, if any do.
pub(crate) fn open_disputes_refusal(
    items: &[TdReviewItem],
    pending: &[crate::db::audit_cross::PendingDispute],
) -> Option<String> {
    let open = crate::core::td_review::open_disputes(items, pending);
    if open.is_empty() {
        return None;
    }
    let ids: Vec<&str> = open.iter().map(|d| d.td_id.as_str()).collect();
    Some(format!(
        "{} cross-agent dispute(s) still await a human decision ({}): answer each card in the \"Cross-agent audit — disputes\" discussion, or settle those sheets in the tech-debt review, before the project can be validated.",
        open.len(),
        ids.join(", ")
    ))
}

/// Documents Kronn owns that still carry a placeholder or a `TODO:` marker.
/// The validation's first phase settles every such unknown; TD sheets and
/// dated reports have their own review and are not counted here.
fn unresolved_unknowns(
    project_path: &std::path::Path,
    report: &crate::core::document_optimization::DocumentaryOptimizationReport,
) -> Vec<String> {
    let mut paths: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "placeholder")
        .filter(|d| {
            let within = crate::core::docs_location::DocsRoot::of(project_path)
                .strip(&d.path)
                .unwrap_or_default()
                .to_string();
            !within.starts_with("tech-debt/") && !within.starts_with("reports/")
        })
        .filter(|d| crate::core::document_optimization::kronn_owns(project_path, &d.path))
        .map(|d| d.path.clone())
        .collect();
    paths.dedup();
    paths
}

/// POST /api/projects/:id/mark-bootstrapped
/// Records `bootstrapped_at` in `docs/.kronn.json` and refreshes the project's audit status.
pub async fn mark_bootstrapped(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let lease =
        match crate::api::audit::docs_write::DocsWriteLease::acquire(&state, &project.id).await {
            Ok(lease) => lease,
            Err(error) => return Json(ApiResponse::err(error)),
        };
    let project_path_str = project.path.clone();

    let result = lease
        .blocking(move || -> Result<(), String> {
            let project_path = scanner::resolve_host_path(&project_path_str);
            let index_file = scanner::detect_docs_entry(&project_path);

            if !index_file.exists() {
                return Err(format!(
                    "{} not found",
                    crate::core::docs_location::DocsRoot::of(&project_path).path("AGENTS.md")
                ));
            }

            kronn_state::mark_bootstrapped(&project_path)
        })
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = result {
        return Json(ApiResponse::err(e));
    }

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

#[cfg(test)]
mod validate_gate_tests {
    use super::*;
    use axum::extract::{Path as AxPath, State};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn test_state() -> AppState {
        let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let config = Arc::new(RwLock::new(crate::core::config::default_config()));
        AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    /// Seed a project whose path is a real tempdir with a docs entry, plus
    /// an audit run and (optionally) a linked validation discussion whose
    /// last Agent message is `last_agent_msg`.
    async fn seed(
        state: &AppState,
        tmp: &std::path::Path,
        run_status: &str,
        link: Option<&str>,
        last_agent_msg: Option<&str>,
    ) {
        std::fs::create_dir_all(tmp.join("docs")).unwrap();
        std::fs::write(tmp.join("docs/AGENTS.md"), "# proj docs\n").unwrap();
        let path = tmp.to_str().unwrap().to_string();
        let link = link.map(|s| s.to_string());
        let msg = last_agent_msg.map(|s| s.to_string());
        let status = run_status.to_string();
        state.db.with_conn(move |conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p1', 'P', ?1, ?2, ?2)",
                rusqlite::params![path, now],
            )?;
            crate::db::audit_runs::insert_running(
                conn, "r1", "p1", "Full", "ClaudeCode", chrono::Utc::now(),
            )?;
            if status == "Completed" {
                crate::db::audit_runs::complete(
                    conn, "r1", chrono::Utc::now(), "Completed",
                    0, 0, 0, 0, 0, 0, 0, 100, None, None,
                )?;
                crate::db::audit_documents::record(conn, "r1",
                    &crate::core::audit_documents::capture_json(std::path::Path::new(&path))
                        .map_err(anyhow::Error::msg)?)?;
            } else if status == "Interrupted" {
                crate::db::audit_runs::update_last_completed_step(conn, "r1", 3)?;
                crate::db::audit_runs::mark_interrupted(conn, "r1", "test")?;
            }
            if let Some(disc_id) = link.as_deref() {
                conn.execute(
                    "INSERT INTO discussions (id, project_id, title, agent, language, created_at, updated_at)
                     VALUES (?1, 'p1', 'Validation audit AI', 'ClaudeCode', 'fr', datetime('now'), datetime('now'))",
                    [disc_id],
                )?;
                if let Some(m) = msg.as_deref() {
                    conn.execute(
                        "INSERT INTO messages (id, discussion_id, role, content, timestamp, tokens_used)
                         VALUES ('m1', ?1, 'Agent', ?2, datetime('now'), 0)",
                        rusqlite::params![disc_id, m],
                    )?;
                }
                crate::db::audit_runs::set_validation_discussion(conn, "r1", disc_id)?;
            }
            Ok(())
        }).await.unwrap();
    }

    async fn call(state: &AppState) -> ApiResponse<AiAuditStatus> {
        validate_audit(State(state.clone()), AxPath("p1".to_string()))
            .await
            .0
    }

    // Model the completed validation phase when a test writes its fixture
    // docs after seed(). Production records this with the terminal reply.
    async fn record_reviewed_documents(state: &AppState, path: &std::path::Path) {
        let documents = crate::core::audit_documents::capture_json(path).unwrap();
        state
            .db
            .with_conn(move |conn| crate::db::audit_documents::record(conn, "r1", &documents))
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn happy_path_latest_completed_linked_and_finished() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("Tout est validé.\nKRONN:VALIDATION_COMPLETE"),
        )
        .await;
        let resp = call(&state).await;
        assert!(resp.success, "gate must pass: {:?}", resp.error);
        assert!(
            !state.audit_tracker.lock().unwrap().leased.contains("p1"),
            "the lease must be released after validation"
        );
    }

    #[tokio::test]
    async fn a_legacy_completed_run_requires_a_new_audit_without_guessing_proof() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE"),
        )
        .await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE audit_runs SET final_doc_fingerprints_json = NULL WHERE id = 'r1'",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let response = call(&state).await;
        assert!(!response.success);
        let error = response.error.unwrap();
        assert!(error.contains("run the audit again"), "{error}");
        assert!(state
            .db
            .with_read_conn(|conn| crate::db::audit_documents::get(conn, "r1"))
            .await
            .unwrap()
            .is_none());
        assert!(crate::core::kronn_state::read(tmp.path()).is_none_or(|s| s.validated_at.is_none()));
    }

    #[tokio::test]
    async fn documents_pulled_after_the_validation_finished_are_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE"),
        )
        .await;
        std::fs::write(
            tmp.path().join("docs/AGENTS.md"),
            "# Documents pulled after the audit\nNew unreviewed policy.\n",
        )
        .unwrap();
        let response = call(&state).await;
        assert!(
            !response.success,
            "unreviewed contents must not be validated"
        );
        let error = response.error.unwrap();
        assert!(
            error.contains("changed since") && error.contains("docs/AGENTS.md"),
            "{error}"
        );
        assert!(crate::core::kronn_state::read(tmp.path()).is_none_or(|s| s.validated_at.is_none()));
        assert!(crate::core::doc_provenance::report(tmp.path(), false)
            .unwrap()
            .docs
            .is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unresolved_unknowns_block_validation_until_settled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE"),
        )
        .await;
        let docs = tmp.path().join("docs");
        std::fs::write(
            docs.join("AGENTS.md"),
            "# Projet\n\nSee [glossary](glossary.md) and [notes](legacy/notes.md).\n",
        )
        .unwrap();
        std::fs::write(
            docs.join("glossary.md"),
            "# Glossaire\n\n<!-- TODO: ask user — what does « lot » mean? -->\n",
        )
        .unwrap();
        // A TD sheet's own unknowns and a human legacy note do not count.
        std::fs::create_dir_all(docs.join("tech-debt")).unwrap();
        std::fs::write(
            docs.join("tech-debt/TD-20260901-x.md"),
            "# TD\n<!-- TODO: verify -->\n",
        )
        .unwrap();
        std::fs::create_dir_all(docs.join("legacy")).unwrap();
        std::fs::write(
            docs.join("legacy/notes.md"),
            "# Notes\n<!-- TODO: later -->\n",
        )
        .unwrap();

        let refused = call(&state).await;
        assert!(!refused.success);
        let error = refused.error.unwrap();
        assert!(error.contains("docs/glossary.md"), "{error}");
        assert!(
            !error.contains("tech-debt") && !error.contains("legacy"),
            "{error}"
        );
        assert!(crate::core::kronn_state::read(tmp.path()).is_none_or(|s| s.validated_at.is_none()));

        std::fs::write(
            docs.join("glossary.md"),
            "# Glossaire\n\n- **lot**: unknown.\n",
        )
        .unwrap();
        record_reviewed_documents(&state, tmp.path()).await;
        let validated = call(&state).await;
        assert!(validated.success, "{:?}", validated.error);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn specialized_validation_checks_unknowns_without_writing_the_full_report() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE"),
        )
        .await;
        std::fs::write(tmp.path().join("docs/AGENTS.md"), "# {{BROKEN_TEMPLATE}}\n").unwrap();
        record_reviewed_documents(&state, tmp.path()).await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE audit_runs SET kind = 'Security' WHERE id = 'r1'",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let response = call(&state).await;
        assert!(
            !response.success,
            "a specialized run cannot validate a template"
        );
        assert!(response.error.unwrap().contains("Unresolved unknowns"));
        std::fs::write(
            tmp.path().join("docs/AGENTS.md"),
            "# Completed documentation\n",
        )
        .unwrap();
        record_reviewed_documents(&state, tmp.path()).await;
        assert!(call(&state).await.success);
        assert!(!tmp
            .path()
            .join(crate::core::document_optimization::REPORT_FILE)
            .exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lookalike_unlinked_discussion_is_refused() {
        // A manually created "Validation audit ..." discussion NOT linked to
        // the run satisfies nothing (the old title/date heuristic is gone).
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(&state, tmp.path(), "Completed", None, None).await;
        state.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, project_id, title, agent, language, created_at, updated_at)
                 VALUES ('forged', 'p1', 'Validation audit AI', 'ClaudeCode', 'fr', datetime('now'), datetime('now'))",
                [],
            )?;
            Ok(())
        }).await.unwrap();
        let resp = call(&state).await;
        assert!(!resp.success);
        assert!(resp
            .error
            .unwrap()
            .contains("no linked validation discussion"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn linked_but_unfinished_or_midtext_signal_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("mentioning KRONN:VALIDATION_COMPLETE mid-sentence and going on"),
        )
        .await;
        let resp = call(&state).await;
        assert!(!resp.success);
        assert!(resp.error.unwrap().contains("has not finished"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn interrupted_latest_run_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(&state, tmp.path(), "Interrupted", None, None).await;
        let resp = call(&state).await;
        assert!(!resp.success);
        assert!(resp.error.unwrap().contains("only a Completed run"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn newer_interrupted_run_supersedes_an_old_validated_completed() {
        // The ordering regression the list_recent(...,1) gate must lock:
        // an old Completed run — fully linked and signaled — must NOT
        // validate once a NEWER Interrupted attempt exists (its mutation
        // made the old validation stale).
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE"),
        )
        .await;
        state
            .db
            .with_conn(|conn| {
                crate::db::audit_runs::insert_running(
                    conn,
                    "r2",
                    "p1",
                    "Full",
                    "ClaudeCode",
                    chrono::Utc::now() + chrono::Duration::minutes(5),
                )?;
                crate::db::audit_runs::update_last_completed_step(conn, "r2", 2)?;
                crate::db::audit_runs::mark_interrupted(conn, "r2", "cut")?;
                Ok(())
            })
            .await
            .unwrap();
        let resp = call(&state).await;
        assert!(!resp.success);
        assert!(
            resp.error.unwrap().contains("only a Completed run"),
            "the newer Interrupted attempt must gate out the stale Completed"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn linked_discussion_owned_by_another_project_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(&state, tmp.path(), "Completed", None, None).await;
        state.db.with_conn(|conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p2', 'Other', '/tmp/other', ?1, ?1)",
                rusqlite::params![now],
            )?;
            conn.execute(
                "INSERT INTO discussions (id, project_id, title, agent, language, created_at, updated_at)
                 VALUES ('d-foreign', 'p2', 'Validation audit AI', 'ClaudeCode', 'fr', datetime('now'), datetime('now'))",
                [],
            )?;
            conn.execute(
                "INSERT INTO messages (id, discussion_id, role, content, timestamp, tokens_used)
                 VALUES ('m1', 'd-foreign', 'Agent', 'KRONN:VALIDATION_COMPLETE', datetime('now'), 0)",
                [],
            )?;
            crate::db::audit_runs::set_validation_discussion(conn, "r1", "d-foreign")?;
            Ok(())
        }).await.unwrap();
        let resp = call(&state).await;
        assert!(!resp.success);
        assert!(resp.error.unwrap().contains("another project"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn archived_unfinished_validation_can_finish_in_a_replacement_discussion() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d-archived"),
            Some("Validation still pending"),
        )
        .await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE discussions SET archived=1 WHERE id='d-archived'",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO discussions
                     (id, project_id, title, agent, language, created_at, updated_at)
                     VALUES ('d-resumed', 'p1', 'Validation audit AI', 'ClaudeCode', 'fr', datetime('now', '+1 second'), datetime('now', '+1 second'))",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO messages
                     (id, discussion_id, role, content, timestamp, tokens_used)
                     VALUES ('m-resumed', 'd-resumed', 'Agent', 'Reprise terminée.\nKRONN:VALIDATION_COMPLETE', datetime('now'), 0)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let response = call(&state).await;
        assert!(
            response.success,
            "replacement must validate: {:?}",
            response.error
        );
        let linked = state
            .db
            .with_conn(|conn| {
                Ok(crate::db::audit_runs::get_by_id(conn, "r1")?
                    .and_then(|run| run.validation_discussion_id))
            })
            .await
            .unwrap();
        assert_eq!(linked.as_deref(), Some("d-resumed"));
    }

    async fn timeline_validation(
        state: &AppState,
    ) -> Option<crate::api::audit::run::AuditTimelineValidation> {
        crate::api::audit::run::audit_timeline(State(state.clone()), AxPath("p1".to_string()))
            .await
            .0
            .data
            .expect("timeline")
            .latest_validation
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn archived_finished_validation_is_reported_by_the_timeline_and_validates() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("Fin.\nKRONN:VALIDATION_COMPLETE"),
        )
        .await;
        state
            .db
            .with_conn(|conn| {
                conn.execute("UPDATE discussions SET archived=1 WHERE id='d1'", [])?;
                Ok(())
            })
            .await
            .unwrap();

        let reported = timeline_validation(&state)
            .await
            .expect("linked validation");
        assert_eq!(reported.discussion_id, "d1");
        assert!(reported.finished && reported.archived);
        let response = call(&state).await;
        assert!(response.success, "{:?}", response.error);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timeline_reports_an_unfinished_validation_and_none_for_an_interrupted_run() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE puis la suite"),
        )
        .await;
        let reported = timeline_validation(&state)
            .await
            .expect("linked validation");
        assert!(!reported.finished && !reported.archived);
        assert_eq!(reported.completed_at, None);

        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Interrupted",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE"),
        )
        .await;
        assert!(timeline_validation(&state).await.is_none());
    }

    async fn push_agent_message(state: &AppState, id: &str, content: &str, at: &str) {
        let msg: DiscussionMessage = serde_json::from_value(serde_json::json!({
            "id": id, "role": "Agent", "content": content,
            "timestamp": at, "tokens_used": 0
        }))
        .unwrap();
        state
            .db
            .with_conn(move |conn| {
                crate::db::discussions::insert_message(conn, "d1", &msg)?;
                Ok(())
            })
            .await
            .unwrap();
    }

    async fn timeline_block(state: &AppState) -> Option<ValidationBlock> {
        crate::api::audit::run::audit_timeline(State(state.clone()), AxPath("p1".to_string()))
            .await
            .0
            .data
            .expect("timeline")
            .validation_block
    }

    // KT-1122: the signal is recorded when it arrives; the later commit note
    // or DoD update (front-africanews) never makes the validation unfinished.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_finished_validation_stays_finished_after_later_agent_messages() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(&state, tmp.path(), "Completed", Some("d1"), None).await;
        push_agent_message(
            &state,
            "m-signal",
            "Fin.\nKRONN:VALIDATION_COMPLETE",
            "2026-08-28T21:37:00Z",
        )
        .await;
        push_agent_message(
            &state,
            "m-pr",
            "Commit poussé (PR #34)",
            "2026-08-29T08:00:00Z",
        )
        .await;
        push_agent_message(&state, "m-dod", "DoD mise à jour", "2026-08-29T08:05:00Z").await;
        state
            .db
            .with_conn(|conn| {
                conn.execute("UPDATE discussions SET archived=1 WHERE id='d1'", [])?;
                Ok(())
            })
            .await
            .unwrap();

        let reported = timeline_validation(&state)
            .await
            .expect("linked validation");
        assert!(reported.finished && reported.archived);
        assert_eq!(
            reported.completed_at.as_deref(),
            Some("2026-08-28T21:37:00+00:00")
        );
        assert_eq!(timeline_block(&state).await, None, "the server accepts");

        // The stamp, not the messages, carries it: even a pruned signal
        // message does not reopen the validation.
        state
            .db
            .with_conn(|conn| {
                conn.execute("DELETE FROM messages WHERE id='m-signal'", [])?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(timeline_validation(&state).await.unwrap().finished);
        let response = call(&state).await;
        assert!(response.success, "{:?}", response.error);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_server_verdict_agrees_with_the_gate_when_the_ui_cannot_validate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("Encore en cours"),
        )
        .await;
        assert_eq!(
            timeline_block(&state).await,
            Some(ValidationBlock::NotFinished)
        );
        let refused = call(&state).await;
        assert!(!refused.success);
        assert!(refused.error.unwrap().contains("has not finished"));

        // `**KRONN:…` (the stream truncated inside the emphasis): the UI and
        // the gate used to disagree on it; now both read one parser.
        push_agent_message(
            &state,
            "m-bold",
            "Fin.\n**KRONN:VALIDATION_COMPLETE",
            "2026-08-28T21:40:00Z",
        )
        .await;
        assert_eq!(timeline_block(&state).await, None);
        assert!(call(&state).await.success);
    }

    // KT-1122: "resume validation" targets the discussion linked to the latest
    // run — a sub-audit's included — never an older, still open discussion that
    // merely carries a generated validation title (front-africanews, April).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_resumed_validation_is_the_runs_link_even_for_a_sub_audit() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d-run"),
            Some("Encore en cours"),
        )
        .await;
        state
            .db
            .with_conn(|conn| {
                conn.execute("UPDATE audit_runs SET kind='Security' WHERE id='r1'", [])?;
                conn.execute(
                    "UPDATE discussions SET title='Validation audit Security' WHERE id='d-run'",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO discussions
                     (id, project_id, title, agent, language, created_at, updated_at)
                     VALUES ('d-april', 'p1', 'Validation audit AI', 'ClaudeCode', 'fr',
                             '2026-04-02T10:00:00Z', '2026-04-02T10:00:00Z')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let reported = timeline_validation(&state)
            .await
            .expect("linked validation");
        assert_eq!(reported.discussion_id, "d-run");
        assert!(!reported.finished);
        let verdict = state
            .db
            .with_read_conn(|conn| validation_state(conn, "p1", false))
            .await
            .unwrap();
        assert_eq!(verdict.discussion_id.as_deref(), Some("d-run"));
        assert_eq!(verdict.blocked, Some(ValidationBlock::NotFinished));
    }

    // KT-1122 rule, as KT-1199 amends it: an instance without the run (P2P
    // peer, another clone) is read-only unless the repository carries a
    // committed, Completed run state that passes the gates. Without one it
    // states why and never validates, not even a project whose docs are shared.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_instance_without_the_run_is_read_only_and_says_why() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("Fin.\nKRONN:VALIDATION_COMPLETE"),
        )
        .await;
        state
            .db
            .with_conn(|conn| {
                conn.execute("DELETE FROM audit_runs WHERE id='r1'", [])?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(timeline_validation(&state).await.is_none());
        assert_eq!(
            timeline_block(&state).await,
            Some(ValidationBlock::NoLocalRun)
        );
        let response = call(&state).await;
        assert!(!response.success);
        let reason = response.error.unwrap();
        assert!(reason.contains("read-only"), "{reason}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn who_opened_and_who_validated_are_recorded_and_shown() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        state.config.write().await.server.pseudo = Some("  Romu ".into());
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("Fin.\nKRONN:VALIDATION_COMPLETE"),
        )
        .await;
        state
            .db
            .with_conn(|conn| crate::db::audit_validation::set_started_by(conn, "r1", "Audit-Bot"))
            .await
            .unwrap();
        assert_eq!(
            timeline_validation(&state)
                .await
                .unwrap()
                .started_by
                .as_deref(),
            Some("Audit-Bot")
        );

        let response = call(&state).await;
        assert!(response.success, "{:?}", response.error);

        let file = kronn_state::read(tmp.path()).expect(".kronn.json written");
        assert_eq!(file.validated_by.as_deref(), Some("Romu"));
        assert_eq!(file.validation_started_by.as_deref(), Some("Audit-Bot"));
        let data =
            crate::api::audit::run::audit_timeline(State(state.clone()), AxPath("p1".to_string()))
                .await
                .0
                .data
                .expect("timeline");
        assert_eq!(data.recorded_validated_by.as_deref(), Some("Romu"));
        assert_eq!(
            data.recorded_validation_started_by.as_deref(),
            Some("Audit-Bot")
        );
        let shown = data.latest_validation.expect("validation");
        assert_eq!(shown.validated_by.as_deref(), Some("Romu"));
        assert!(shown.validated_at.is_some());
    }

    fn write_td(tmp: &std::path::Path, id: &str, severity: &str, status: &str) {
        let dir = tmp.join("docs/tech-debt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{id}.md")),
            format!(
                "---\nname: {id}\nmetadata:\n  type: tech-debt\n---\n\n# {id}\n\n- **Severity**: {severity}\n- **Status**: {status}\n"
            ),
        )
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn settled_critical_and_high_sheets_validate_without_the_discussion() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("Validation en cours"),
        )
        .await;
        write_td(tmp.path(), "TD-20261001-a", "Critical", "Confirmed by user");
        write_td(tmp.path(), "TD-20261001-b", "High", "Deferred");
        write_td(tmp.path(), "TD-20261001-c", "Low", "Verified in source");

        let refused = call(&state).await;
        assert!(!refused.success, "a deferred High does not settle");
        assert!(refused.error.unwrap().contains("has not finished"));

        write_td(tmp.path(), "TD-20261001-b", "High", "Accepted decision");
        record_reviewed_documents(&state, tmp.path()).await;
        let validated = call(&state).await;
        assert!(validated.success, "{:?}", validated.error);
        assert_eq!(validated.data, Some(AiAuditStatus::Validated));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_sheet_path_needs_a_completed_run_and_a_sheet() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(&state, tmp.path(), "Completed", None, None).await;
        // No sheet at all: the discussion stays mandatory.
        let refused = call(&state).await;
        assert!(refused
            .error
            .unwrap()
            .contains("no linked validation discussion"));
        write_td(tmp.path(), "TD-20261001-a", "High", "Rejected");
        record_reviewed_documents(&state, tmp.path()).await;
        let validated = call(&state).await;
        assert!(validated.success, "{:?}", validated.error);

        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(&state, tmp.path(), "Interrupted", None, None).await;
        write_td(tmp.path(), "TD-20261001-a", "High", "Rejected");
        let interrupted = call(&state).await;
        assert!(interrupted.error.unwrap().contains("only a Completed run"));
    }

    #[tokio::test]
    async fn medium_and_low_sheets_do_not_replace_the_validation_discussion() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state();
        seed(&state, tmp.path(), "Completed", None, None).await;
        write_td(tmp.path(), "TD-20261001-a", "Medium", "Confirmed by user");
        write_td(tmp.path(), "TD-20261001-b", "Low", "Rejected");
        record_reviewed_documents(&state, tmp.path()).await;
        let response = call(&state).await;
        assert!(!response.success);
        assert!(response
            .error
            .unwrap()
            .contains("no linked validation discussion"));
    }

    #[tokio::test]
    async fn a_pending_validation_dispatch_blocks_even_settled_blocking_sheets() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE"),
        )
        .await;
        write_td(tmp.path(), "TD-20261001-a", "High", "Confirmed by user");
        record_reviewed_documents(&state, tmp.path()).await;
        state.db.with_conn(|conn| {
            conn.execute("INSERT INTO messages (id, discussion_id, role, content, timestamp, tokens_used, sort_order)
                VALUES ('dispatch-user', 'd1', 'User', 'Continue checking', datetime('now'), 0, 1)", [])?;
            crate::db::agent_dispatch::enqueue_for_latest_user(conn,
                crate::db::agent_dispatch::NewLatestUserDispatch {
                    id: "job-validation", discussion_id: "d1", dedupe_key: "validation-regression",
                    agent_override: None, chain_prompt_ids: &[], batch_item: None,
                    group_id: None, group_concurrency_limit: None,
                })?;
            Ok(())
        }).await.unwrap();
        let token = tokio_util::sync::CancellationToken::new();
        state
            .cancel_registry
            .lock()
            .unwrap()
            .insert("d1".into(), token.clone());
        let response = call(&state).await;
        assert!(!response.success);
        assert!(
            !token.is_cancelled(),
            "validating must not stop the unfinished validator"
        );
        assert!(state
            .db
            .with_read_conn(|conn| crate::db::agent_dispatch::has_active_for_discussion(conn, "d1"))
            .await
            .unwrap());
        assert!(response.error.unwrap().contains("validation agent"));
        assert!(crate::core::kronn_state::read(tmp.path()).is_none_or(|s| s.validated_at.is_none()));
    }

    #[tokio::test]
    async fn an_older_finished_lookalike_cannot_replace_an_archived_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("Not finished"),
        )
        .await;
        state.db.with_conn(|conn| {
            conn.execute("UPDATE discussions SET archived=1 WHERE id='d1'", [])?;
            conn.execute("INSERT INTO discussions (id, project_id, title, agent, language, created_at, updated_at)
                VALUES ('old', 'p1', 'Validation audit AI', 'ClaudeCode', 'fr', datetime('now','-1 day'), datetime('now'))", [])?;
            conn.execute("INSERT INTO messages (id, discussion_id, role, content, timestamp, tokens_used)
                VALUES ('old-message', 'old', 'Agent', 'KRONN:VALIDATION_COMPLETE', datetime('now'), 0)", [])?;
            Ok(())
        }).await.unwrap();
        let response = call(&state).await;
        assert!(!response.success);
        assert!(response.error.unwrap().contains("has not finished"));
        let linked = state
            .db
            .with_conn(|conn| crate::db::audit_runs::get_by_id(conn, "r1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(linked.validation_discussion_id.as_deref(), Some("d1"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn held_lease_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE"),
        )
        .await;
        assert!(state.audit_tracker.lock().unwrap().try_acquire_lease("p1"));
        let resp = call(&state).await;
        assert!(!resp.success);
        assert!(resp.error.unwrap().contains("currently running"));
    }
}
