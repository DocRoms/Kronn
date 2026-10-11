//! KT-1167 — "Turn into Kronn tasks": the audit's tech-debt sheets become
//! planning tasks, on demand or after a Full audit when the briefing asks.

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};

use crate::core::td_tasks;
use crate::models::{
    ApiErrorCode, ApiResponse, AuditKind, PlanningActor, PlanningActorKind, TdTasksOutcome,
};
use crate::AppState;

/// Loads the sheets off the async runtime, then converts them in one
/// transaction.
pub(crate) async fn convert_project_tds(
    db: &Arc<crate::db::Database>,
    project_id: String,
    project_path: PathBuf,
    actor: PlanningActor,
) -> anyhow::Result<TdTasksOutcome> {
    let sources = tokio::task::spawn_blocking(move || {
        let docs_root = crate::core::docs_location::DocsRoot::of(&project_path);
        td_tasks::load_sources_rel(&docs_root.dir(&project_path), docs_root.rel())
    })
    .await?;
    db.with_conn(move |conn| td_tasks::convert(conn, &project_id, &sources, &actor))
        .await
}

/// The briefing option, read from `docs/briefing.md` or the DB notes: an
/// agent rewriting the file must not silently turn it off.
pub(crate) fn briefing_option_set(file_notes: Option<&str>, db_notes: Option<&str>) -> bool {
    file_notes.is_some_and(td_tasks::briefing_wants_td_tasks)
        || db_notes.is_some_and(td_tasks::briefing_wants_td_tasks)
}

/// The conversion that closes a successful Full audit, when the briefing
/// asked for it. `None` when it does not apply.
pub(crate) async fn convert_after_audit(
    db: &Arc<crate::db::Database>,
    kind: AuditKind,
    fully_succeeded: bool,
    option_set: bool,
    project_id: &str,
    project_path: &FsPath,
    audit_run_id: &str,
) -> Option<anyhow::Result<TdTasksOutcome>> {
    if !(option_set && fully_succeeded && matches!(kind, AuditKind::Full)) {
        return None;
    }
    let actor = PlanningActor {
        kind: PlanningActorKind::Backend,
        id: Some(format!("audit:{audit_run_id}")),
        session_id: None,
        source_message_id: None,
    };
    Some(
        convert_project_tds(
            db,
            project_id.to_string(),
            project_path.to_path_buf(),
            actor,
        )
        .await,
    )
}

/// POST /api/projects/{id}/audit/td-tasks
pub async fn convert_td_tasks(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<TdTasksOutcome>> {
    let lookup = id.clone();
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &lookup))
        .await
    {
        Ok(Some(project)) => project,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Project not found",
            ))
        }
        Err(e) => return Json(ApiResponse::err(format!("DB error: {e}"))),
    };
    let project_path = crate::core::scanner::resolve_host_path(&project.path);
    if !project_path.is_dir() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Project directory not found",
        ));
    }
    match convert_project_tds(
        &state.db,
        project.id,
        project_path,
        PlanningActor::default(),
    )
    .await
    {
        Ok(outcome) => Json(ApiResponse::ok(outcome)),
        Err(e) => Json(ApiResponse::err(format!("Could not create the tasks: {e}"))),
    }
}

#[cfg(test)]
#[path = "td_tasks_tests.rs"]
mod tests;
