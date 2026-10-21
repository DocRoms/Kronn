// `POST /api/projects/{id}/audit-runs/{run_id}/steps/{step}/accept` — a human
// accepts a failed step of the project's interrupted audit as it stands
// (KT-1131). The step counts as done: the next resume carries it, and the run
// it finalizes can complete and be validated without a global audit.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;

use crate::models::ApiResponse;
use crate::AppState;

/// Longest reason kept, in characters.
const MAX_REASON_CHARS: usize = 500;

#[derive(Debug, Default, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct AcceptAuditStepRequest {
    /// Why the step is accepted; optional.
    #[serde(default)]
    #[ts(optional)]
    pub reason: Option<String>,
    /// KT-1211 — accept a step whose failure is known to leave an unsound
    /// document (truncated, citations that do not resolve, an undisposed
    /// detector signal, a stopped step) anyway. Without it the acceptance is
    /// refused and the answer carries the diff to look at.
    #[serde(default)]
    #[ts(optional)]
    pub override_unsound: Option<bool>,
}

/// Prefix of the refusal that asks for an explicit override, so a client can
/// tell it from any other refusal and offer the override with the diff.
pub const OVERRIDE_REQUIRED_PREFIX: &str = "override_required:";

/// Lines of diff a refusal carries.
const MAX_DIFF_LINES: usize = 60;

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AcceptAuditStepResponse {
    pub step: u32,
    pub accepted_by: String,
    pub accepted_at: String,
    /// Failed steps of the run still neither retried nor accepted. Empty: a
    /// resume now only finalizes the run.
    pub remaining_failed: Vec<u32>,
}

/// Why an acceptance is refused, before anything is written.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TargetRefusal {
    Missing,
    Template,
}

/// The step's target must exist, hold something, and no longer be the
/// template: a step whose document was never written cannot be accepted.
/// The synthetic review step has no target to check.
pub(crate) fn check_target(
    project_path: &std::path::Path,
    target_file: &str,
) -> Result<(), TargetRefusal> {
    if target_file == "REVIEW" {
        return Ok(());
    }
    let content = match std::fs::read_to_string(project_path.join(target_file)) {
        // A partial marker is Kronn's, not the document's.
        Ok(content) => super::step_guard::strip_partial_marker(&content).unwrap_or(content),
        _ => return Err(TargetRefusal::Missing),
    };
    if content.trim().is_empty() {
        return Err(TargetRefusal::Missing);
    }
    if super::validation::count_raw_placeholders(&content) > 0 {
        return Err(TargetRefusal::Template);
    }
    Ok(())
}

/// The change a step made, as a bounded unified diff.
pub(crate) fn diff_excerpt(before: &str, after: &str) -> String {
    let diff = crate::core::repository_resources::unified_diff(before.as_bytes(), after.as_bytes());
    let mut lines = diff.lines();
    let mut excerpt: Vec<&str> = lines.by_ref().take(MAX_DIFF_LINES).collect();
    if lines.next().is_some() {
        excerpt.push("… (diff truncated)");
    }
    excerpt.join("\n")
}

/// What makes accepting `row` as it stands need an explicit override.
pub(crate) fn accept_blocker_of(
    row: &crate::models::AuditRunStep,
) -> Option<super::step_guard::AcceptBlocker> {
    use super::step_guard::{AcceptBlocker, STATE_RESTORED};
    // A step that never ended has no verdict and, when its agent was cut off,
    // no failure reason either: its target is unvetted, whatever it holds.
    if row.ended_at.is_none() && row.accepted_at.is_none() {
        return Some(AcceptBlocker::NeverEnded);
    }
    row.step_warning
        .as_deref()
        .and_then(AcceptBlocker::of_reason)
        // A stopped step whose document was put back leaves it whole.
        .filter(|blocker| {
            !(*blocker == AcceptBlocker::Killed
                && row.target_state.as_deref() == Some(STATE_RESTORED))
        })
}

/// A reason as stored: one line, trimmed, bounded; `None` when empty.
fn normalized_reason(reason: Option<String>) -> Option<String> {
    let reason = reason?;
    let one_line = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    let bounded: String = one_line.chars().take(MAX_REASON_CHARS).collect();
    (!bounded.is_empty()).then_some(bounded)
}

/// Who accepts: the operator of this instance, as its configuration names it.
async fn operator(state: &AppState) -> String {
    state
        .config
        .read()
        .await
        .server
        .pseudo
        .clone()
        .filter(|pseudo| !pseudo.trim().is_empty())
        .unwrap_or_else(|| "operator".into())
}

pub async fn accept_audit_step(
    State(state): State<AppState>,
    Path((id, run_id, step)): Path<(String, String, u32)>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Json(req): Json<AcceptAuditStepRequest>,
) -> Json<ApiResponse<AcceptAuditStepResponse>> {
    // A human decision: never an agent's, whatever route list lets it through.
    if bridge.is_some() {
        return Json(ApiResponse::err(
            "Accepting an audit step is a human decision: refused to an agent's bridge token.",
        ));
    }
    let reason = normalized_reason(req.reason);
    let by = operator(&state).await;

    let pid = id.clone();
    let rid = run_id.clone();
    let loaded = state
        .db
        .with_conn(move |conn| {
            let project = crate::db::projects::get_project(conn, &pid)?;
            let run = crate::db::audit_runs::get_by_id(conn, &rid)?;
            let latest = crate::db::audit_runs::list_recent(conn, &pid, 1)?
                .into_iter()
                .next();
            let steps = crate::db::audit_runs::list_audit_steps(conn, &rid)?;
            Ok((project, run, latest, steps))
        })
        .await;
    let (project, run, latest, steps) = match loaded {
        Ok(loaded) => loaded,
        Err(e) => return Json(ApiResponse::err(format!("db: {e}"))),
    };
    let Some(project) = project else {
        return Json(ApiResponse::err("Project not found"));
    };
    let Some(run) = run.filter(|run| run.project_id == project.id) else {
        return Json(ApiResponse::err("Audit run not found for this project"));
    };
    if latest.as_ref().map(|l| l.id.as_str()) != Some(run.id.as_str()) {
        return Json(ApiResponse::err(
            "Only a step of the project's latest audit run can be accepted.",
        ));
    }
    if run.status != "Interrupted" {
        return Json(ApiResponse::err(format!(
            "The run is {}: only a step of an interrupted run can be accepted.",
            run.status
        )));
    }
    let Some(kind) = crate::models::AuditKind::from_label(&run.kind) else {
        return Json(ApiResponse::err(format!("Unknown audit kind {}", run.kind)));
    };
    let chain = super::steps_for(
        kind,
        &crate::core::docs_location::DocsRoot::of(&crate::core::scanner::resolve_host_path(
            &project.path,
        )),
    );
    let total = chain.len() as u32;
    if !(1..=total).contains(&step) {
        return Json(ApiResponse::err(format!(
            "Step {step} is not part of this {total}-step audit."
        )));
    }
    let done = super::full::already_succeeded_step_indices(&steps);
    if done.contains(&step) {
        return Json(ApiResponse::err(format!(
            "Step {step} is already done: only a failed step can be accepted."
        )));
    }
    let analysis_step = &chain[step as usize - 1];
    let project_root = crate::core::scanner::resolve_host_path(&project.path);
    let target = analysis_step.target_file.to_string();
    let checked = {
        let root = project_root.clone();
        let target = target.clone();
        tokio::task::spawn_blocking(move || check_target(&root, &target)).await
    };
    match checked {
        Ok(Ok(())) => {}
        Ok(Err(TargetRefusal::Missing)) => {
            return Json(ApiResponse::err(format!(
                "`{target}` is missing or empty: there is nothing to accept. Retry the step instead."
            )))
        }
        Ok(Err(TargetRefusal::Template)) => {
            return Json(ApiResponse::err(format!(
                "`{target}` still holds template placeholders: it was never filled. Retry the step instead."
            )))
        }
        Err(e) => return Json(ApiResponse::err(format!("target check failed: {e}"))),
    }

    // KT-1211 (M4) — a step that failed for a reason that leaves its document
    // unsound is accepted only on an explicit override, with the diff shown.
    let step_row = steps.iter().find(|row| row.step_index == step);
    let blocker = step_row.and_then(accept_blocker_of);
    let overridden = req.override_unsound.unwrap_or(false);
    if let Some(blocker) = blocker.filter(|_| !overridden) {
        let snapshot = {
            let rid = run.id.clone();
            state
                .db
                .with_conn(move |conn| crate::db::audit_runs::step_snapshot(conn, &rid, step))
                .await
                .ok()
                .flatten()
        };
        let diff = {
            let root = project_root.clone();
            let target = target.clone();
            tokio::task::spawn_blocking(move || {
                let now = super::step_guard::read_target(&root, &target)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let now = super::step_guard::strip_partial_marker(&now).unwrap_or(now);
                match snapshot {
                    Some(before) => diff_excerpt(before.content.as_deref().unwrap_or(""), &now),
                    None => String::new(),
                }
            })
            .await
            .unwrap_or_default()
        };
        let mut message = format!(
            "{OVERRIDE_REQUIRED_PREFIX} step {step} cannot be accepted as it stands: {}. \
             Retry the step, or confirm the override to accept it anyway.",
            blocker.label()
        );
        if !diff.is_empty() {
            message.push_str(&format!("\n\nWhat the step changed in `{target}`:\n{diff}"));
        }
        return Json(ApiResponse::err(message));
    }
    let reason = match (blocker, reason) {
        (Some(blocker), Some(reason)) => Some(format!("override ({}): {reason}", blocker.label())),
        (Some(blocker), None) => Some(format!("override ({})", blocker.label())),
        (None, reason) => reason,
    };

    let lease =
        match crate::api::audit::docs_write::DocsWriteLease::acquire(&state, &project.id).await {
            Ok(lease) => lease,
            Err(error) => return Json(ApiResponse::err(error)),
        };
    // The step now counts as done: its document is the human's call, no
    // longer marked partial.
    {
        let root = project_root.clone();
        let target = target.clone();
        match lease
            .blocking(move || super::step_guard::clear_partial_marker(&root, &target))
            .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => tracing::warn!("Accepted step {step}: {e}"),
            Err(e) => tracing::warn!("Accepted step {step}: marker cleanup failed: {e}"),
        }
    }
    let at = Utc::now();
    let label = super::full::step_file_label(analysis_step).to_string();
    let written = {
        let lease = lease.clone();
        let rid = run.id.clone();
        let pid = project.id.clone();
        let by = by.clone();
        let reason = reason.clone();
        state
            .db
            .with_conn(move |conn| {
                let _lease = lease;
                // Re-read under the lease: the run must still be the latest.
                let latest = crate::db::audit_runs::list_recent(conn, &pid, 1)?
                    .into_iter()
                    .next();
                if latest.map(|l| l.id) != Some(rid.clone()) {
                    return Ok(None);
                }
                let accepted = crate::db::audit_runs::accept_step(
                    conn,
                    &rid,
                    step,
                    &label,
                    &by,
                    reason.as_deref(),
                    at,
                )?;
                if accepted {
                    crate::db::audit_runs::delete_step_snapshot(conn, &rid, step)?;
                }
                let steps = crate::db::audit_runs::list_audit_steps(conn, &rid)?;
                Ok(Some((accepted, steps)))
            })
            .await
    };
    let steps = match written {
        Ok(Some((true, steps))) => steps,
        Ok(Some((false, _))) => {
            return Json(ApiResponse::err(format!(
                "Step {step} is already done: only a failed step can be accepted."
            )))
        }
        Ok(None) => {
            return Json(ApiResponse::err(
                "A newer audit run started meanwhile: accept a step of that run instead.",
            ))
        }
        Err(e) => return Json(ApiResponse::err(format!("db: {e}"))),
    };
    tracing::info!(
        target: "kronn::audit",
        project = %project.id, run = %run.id, step, by = %by,
        reason = reason.as_deref().unwrap_or(""),
        "audit step accepted as it stands"
    );
    let done = super::full::already_succeeded_step_indices(&steps);
    Json(ApiResponse::ok(AcceptAuditStepResponse {
        step,
        accepted_by: by,
        accepted_at: at.to_rfc3339(),
        remaining_failed: (1..=total).filter(|s| !done.contains(s)).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reason_is_kept_on_one_bounded_line() {
        assert_eq!(normalized_reason(None), None);
        assert_eq!(normalized_reason(Some("  \n ".into())), None);
        assert_eq!(
            normalized_reason(Some(" up to date\nby hand ".into())).as_deref(),
            Some("up to date by hand")
        );
        let long = "é".repeat(MAX_REASON_CHARS + 20);
        assert_eq!(
            normalized_reason(Some(long)).unwrap().chars().count(),
            MAX_REASON_CHARS
        );
    }

    #[test]
    fn a_missing_empty_or_template_target_is_refused() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            check_target(root.path(), "docs/glossary.md"),
            Err(TargetRefusal::Missing)
        );
        std::fs::create_dir_all(root.path().join("docs")).unwrap();
        std::fs::write(root.path().join("docs/glossary.md"), "  \n").unwrap();
        assert_eq!(
            check_target(root.path(), "docs/glossary.md"),
            Err(TargetRefusal::Missing)
        );
        std::fs::write(
            root.path().join("docs/glossary.md"),
            "# Glossary\n{{TERMS}}\n",
        )
        .unwrap();
        assert_eq!(
            check_target(root.path(), "docs/glossary.md"),
            Err(TargetRefusal::Template)
        );
        std::fs::write(
            root.path().join("docs/glossary.md"),
            "# Glossary\nKronn: a tool.\n",
        )
        .unwrap();
        assert_eq!(check_target(root.path(), "docs/glossary.md"), Ok(()));
        assert_eq!(check_target(root.path(), "REVIEW"), Ok(()));
    }
}
