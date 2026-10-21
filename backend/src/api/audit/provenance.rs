// Human-validation provenance of the docs (KT-1079): what a person validated,
// what Kronn changed since, and « Confirmer ».

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;

use crate::core::doc_provenance::{self, ConfirmDocsRequest, DocProvenanceReport};
use crate::core::scanner;
use crate::models::{ApiErrorCode, ApiResponse};
use crate::AppState;

#[derive(Debug, Default, Deserialize)]
pub struct ProvenanceQuery {
    /// Adds the before/after of each doc to reconfirm.
    #[serde(default)]
    pub detail: bool,
}

async fn project_of(state: &AppState, id: String) -> Result<(String, std::path::PathBuf), String> {
    match state
        .db
        .with_read_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(p)) => Ok((p.id, scanner::resolve_host_path(&p.path))),
        Ok(None) => Err("Project not found".into()),
        Err(e) => Err(format!("DB error: {e}")),
    }
}

/// GET /api/projects/:id/doc-provenance
pub async fn doc_provenance_report(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ProvenanceQuery>,
) -> Json<ApiResponse<DocProvenanceReport>> {
    let path = match project_of(&state, id).await {
        Ok((_, path)) => path,
        Err(e) => return Json(ApiResponse::err(e)),
    };
    let result = tokio::task::spawn_blocking(move || doc_provenance::report(&path, query.detail))
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {e}")));
    match result {
        Ok(report) => Json(ApiResponse::ok(report)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// POST /api/projects/:id/doc-provenance/confirm
/// « Confirmer »: a person re-validates the docs Kronn changed. Human only,
/// held under the project lease like a validation.
pub async fn confirm_doc_provenance(
    State(state): State<AppState>,
    Path(id): Path<String>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Json(req): Json<ConfirmDocsRequest>,
) -> Json<ApiResponse<DocProvenanceReport>> {
    if bridge.is_some() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Only a human can confirm a documentation change",
        ));
    }
    let (project_id, path) = match project_of(&state, id).await {
        Ok(found) => found,
        Err(e) => return Json(ApiResponse::err(e)),
    };
    let lease =
        match crate::api::audit::docs_write::DocsWriteLease::acquire(&state, &project_id).await {
            Ok(lease) => lease,
            Err(error) => return Json(ApiResponse::err_coded(ApiErrorCode::Conflict, error)),
        };
    let result = lease
        .blocking(move || {
            let confirmed = doc_provenance::confirm(&path, &req.docs)?;
            let documents = crate::core::audit_documents::capture(&path)?;
            Ok::<_, String>((doc_provenance::report(&path, false)?, confirmed, documents))
        })
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {e}")));
    let result = match result {
        Ok((report, confirmed, documents)) => {
            let pid = project_id.clone();
            state
                .db
                .with_conn(move |conn| {
                    crate::db::audit_documents::confirm_paths(conn, &pid, &confirmed, &documents)
                })
                .await
                .map(|()| report)
                .map_err(|e| format!("Cannot record confirmed audit documents: {e}"))
        }
        Err(e) => Err(e),
    };
    match result {
        Ok(report) => Json(ApiResponse::ok(report)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}
