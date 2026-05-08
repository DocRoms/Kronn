// Audit lifecycle markers: validate (post-audit user signoff) and
// mark-bootstrapped (post-bootstrap-discussion finalization). Both
// inject a dated comment marker into `docs/AGENTS.md` and refresh the
// project's audit_status.

use axum::{
    extract::{Path, State},
    Json,
};

use crate::core::scanner;
use crate::models::*;
use crate::AppState;

/// POST /api/projects/:id/validate-audit
/// Marks the audit as validated by injecting a KRONN:VALIDATED marker into docs/AGENTS.md.
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
        let index_file = project_path.join("docs/AGENTS.md");

        if !index_file.exists() {
            return Err("docs/AGENTS.md not found — run the audit first".into());
        }

        let content = std::fs::read_to_string(&index_file)
            .map_err(|e| format!("Cannot read docs/AGENTS.md: {}", e))?;

        if content.contains("KRONN:VALIDATED") {
            return Ok(());
        }

        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let marker = format!("\n<!-- KRONN:VALIDATED:{} -->\n", today);
        let new_content = format!("{}{}", content.trim_end(), marker);

        std::fs::write(&index_file, new_content)
            .map_err(|e| format!("Failed to write marker: {}", e))?;

        Ok(())
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = validate_result {
        return Json(ApiResponse::err(e));
    }

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

/// POST /api/projects/:id/mark-bootstrapped
/// Marks the project as bootstrapped by injecting a KRONN:BOOTSTRAPPED marker into docs/AGENTS.md.
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
        let index_file = project_path.join("docs/AGENTS.md");

        if !index_file.exists() {
            return Err("docs/AGENTS.md not found".into());
        }

        let content = std::fs::read_to_string(&index_file)
            .map_err(|e| format!("Cannot read docs/AGENTS.md: {}", e))?;

        if content.contains("KRONN:BOOTSTRAPPED") {
            return Ok(());
        }

        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let marker = format!("\n<!-- KRONN:BOOTSTRAPPED:{} -->\n", today);
        let new_content = format!("{}{}", content.trim_end(), marker);

        std::fs::write(&index_file, new_content)
            .map_err(|e| format!("Failed to write marker: {}", e))?;

        Ok(())
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = result {
        return Json(ApiResponse::err(e));
    }

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}
