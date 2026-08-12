//! `GET /api/rtk/state` — RTK adoption, collected and bounded (KT-197).
//!
//! Runs the five RTK commands through Quick Exec and folds them into one panel.
//! Quick Exec rather than a bespoke spawn because the boundary is already there:
//! `rtk` is allowlisted, the argv is fixed by a template, and the cwd has to
//! resolve inside a declared project root.

use axum::{extract::State, Json};
use tokio_util::sync::CancellationToken;

use crate::core::quick_exec::{self, QuickExecStatus};
use crate::core::quick_exec_templates::spec_from_template;
use crate::core::rtk_state::{classify, render, RtkSource, RtkState, SourceState};
use crate::models::ApiResponse;
use crate::AppState;

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct RtkStateResponse {
    pub state: RtkState,
    /// The panel as text, already bounded — what an agent should read instead of
    /// five command outputs.
    pub rendered: String,
}

/// `GET /api/rtk/state`
pub async fn rtk_state(State(state): State<AppState>) -> Json<ApiResponse<RtkStateResponse>> {
    // RTK reports on the CLI's own history, not on a project's code, so any
    // declared root will do — but there must BE one, because Quick Exec refuses to
    // run anywhere that is not declared.
    let roots = match state
        .db
        .with_conn(|conn| {
            Ok(crate::db::projects::list_projects(conn)?
                .into_iter()
                .map(|project| std::path::PathBuf::from(project.path))
                .collect::<Vec<_>>())
        })
        .await
    {
        Ok(roots) => roots,
        Err(error) => return Json(ApiResponse::err(format!("project lookup failed: {error}"))),
    };

    let Some(cwd) = roots.iter().find(|path| path.is_dir()).cloned() else {
        return Json(ApiResponse::err(
            "no project directory is available to run rtk in — add a project first",
        ));
    };

    let cancel = CancellationToken::new();
    let mut collected = Vec::new();
    for source in [
        RtkSource::Gain,
        RtkSource::Session,
        RtkSource::Discover,
        RtkSource::HookAudit,
        RtkSource::CcEconomics,
    ] {
        collected.push((source, collect(source, &cwd, &roots, &cancel).await));
    }

    let find = |wanted: RtkSource| {
        collected
            .iter()
            .find(|(source, _)| *source == wanted)
            .map(|(_, state)| state.clone())
            .unwrap_or(SourceState::Unavailable {
                diagnosis: "this source was not collected".to_string(),
                remedy: "re-run the collection".to_string(),
            })
    };

    let assembled = RtkState {
        gain: find(RtkSource::Gain),
        session: find(RtkSource::Session),
        discover: find(RtkSource::Discover),
        hook_audit: find(RtkSource::HookAudit),
        cc_economics: find(RtkSource::CcEconomics),
    };
    let rendered = render(&assembled);
    Json(ApiResponse::ok(RtkStateResponse {
        state: assembled,
        rendered,
    }))
}

/// Run one source and classify what came back.
///
/// A source that cannot be prepared or run is classified with `ran = false`, which
/// yields `Unavailable` — never an empty `Ready`. An unmeasured adoption rate is
/// not a good one.
async fn collect(
    source: RtkSource,
    cwd: &std::path::Path,
    roots: &[std::path::PathBuf],
    cancel: &CancellationToken,
) -> SourceState {
    let Ok(spec) = spec_from_template(source.template_id(), cwd, &[]) else {
        return classify(source, "", "", false);
    };
    let Ok(validated) = quick_exec::validate(&spec, roots) else {
        return classify(source, "", "", false);
    };
    // No artifact directory: these outputs are small and the panel is the product.
    // Keeping a log per collection would grow a directory for nothing.
    let Ok(result) = quick_exec::run(&validated, None, cancel).await else {
        return classify(source, "", "", false);
    };

    // `rtk` reports its own blockers on a zero exit as often as on a failure, so
    // the summary is classified either way. Only a run that never completed is
    // treated as absent.
    let ran = !matches!(
        result.status,
        QuickExecStatus::TimedOut | QuickExecStatus::Cancelled | QuickExecStatus::Rejected
    );
    classify(source, &result.summary, "", ran)
}
