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
}

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
        Ok(content) if !content.trim().is_empty() => content,
        _ => return Err(TargetRefusal::Missing),
    };
    if super::validation::count_raw_placeholders(&content) > 0 {
        return Err(TargetRefusal::Template);
    }
    Ok(())
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

    let lease =
        match crate::api::audit::docs_write::DocsWriteLease::acquire(&state, &project.id).await {
            Ok(lease) => lease,
            Err(error) => return Json(ApiResponse::err(error)),
        };
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
