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

use crate::core::{kronn_state, scanner};
use crate::models::*;
use crate::AppState;

/// POST /api/projects/:id/validate-audit
/// Records `validated_at` in `docs/.kronn.json` and refreshes the project's audit status.
pub async fn validate_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    // Run filesystem I/O on blocking thread pool
    let validate_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let index_file = scanner::detect_docs_entry(&project_path);

        if !index_file.exists() {
            return Err("docs/AGENTS.md not found — run the audit first".into());
        }

        kronn_state::mark_validated(&project_path)
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = validate_result {
        return Json(ApiResponse::err(e));
    }

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

/// POST /api/projects/:id/mark-bootstrapped
/// Records `bootstrapped_at` in `docs/.kronn.json` and refreshes the project's audit status.
pub async fn mark_bootstrapped(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let index_file = scanner::detect_docs_entry(&project_path);

        if !index_file.exists() {
            return Err("docs/AGENTS.md not found".into());
        }

        kronn_state::mark_bootstrapped(&project_path)
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = result {
        return Json(ApiResponse::err(e));
    }

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}
