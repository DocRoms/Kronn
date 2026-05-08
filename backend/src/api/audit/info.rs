// `GET /api/projects/:id/audit-info` — read-only endpoint returning
// the docs/ tree (filled vs unfilled) plus parsed TODOs and tech-debt
// items, used by the validation/briefing UIs to summarize "what's left".

use axum::{
    extract::{Path, State},
    Json,
};

use crate::models::*;
use crate::AppState;

use super::helpers::compute_audit_info_sync;

/// GET /api/projects/:id/audit-info
/// Returns the list of filled AI files and remaining TODOs for the validation prompt.
pub async fn audit_info(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AuditInfo>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    // Run filesystem I/O on blocking thread pool to avoid blocking the async runtime
    let result = tokio::task::spawn_blocking(move || {
        compute_audit_info_sync(&project_path_str)
    }).await.unwrap_or_else(|_| AuditInfo { files: vec![], todos: vec![], tech_debt_items: vec![] });

    Json(ApiResponse::ok(result))
}
