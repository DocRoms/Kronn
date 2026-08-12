//! `GET /api/projects/{id}/context-audit` — KT-194.
//!
//! Read-only by construction: the module behind it has no write path, and the tier
//! split it returns is a proposal for a human. An audit that rewrote instruction
//! files could delete the one rule holding something together.

use axum::{
    extract::{Path, State},
    Json,
};

use crate::core::context_audit::{audit_repo, render, ContextAudit};
use crate::models::ApiResponse;
use crate::AppState;

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ContextAuditResponse {
    pub project_id: String,
    pub audit: ContextAudit,
    /// The report as text, already bounded.
    pub rendered: String,
}

/// `GET /api/projects/{id}/context-audit`
pub async fn project_context_audit(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Json<ApiResponse<ContextAuditResponse>> {
    let lookup = project_id.clone();
    let path = match state
        .db
        .with_conn(move |conn| {
            Ok(crate::db::projects::list_projects(conn)?
                .into_iter()
                .find(|project| project.id == lookup)
                .map(|project| project.path))
        })
        .await
    {
        Ok(Some(path)) => path,
        Ok(None) => return Json(ApiResponse::err(format!("no project {project_id}"))),
        Err(error) => return Json(ApiResponse::err(format!("project lookup failed: {error}"))),
    };

    let root = std::path::PathBuf::from(&path);
    if !root.is_dir() {
        // Said rather than audited as empty: a missing checkout would otherwise
        // produce a clean report about a repository nobody looked at.
        return Json(ApiResponse::err(format!(
            "{path} is not a directory — the project checkout is missing, so its \
             context cannot be audited"
        )));
    }

    // Blocking filesystem work off the async runtime: a large tree would otherwise
    // stall every other request on this thread.
    let audit = match tokio::task::spawn_blocking(move || audit_repo(&root)).await {
        Ok(audit) => audit,
        Err(error) => return Json(ApiResponse::err(format!("audit failed: {error}"))),
    };
    let rendered = render(&audit);

    Json(ApiResponse::ok(ContextAuditResponse {
        project_id,
        audit,
        rendered,
    }))
}
