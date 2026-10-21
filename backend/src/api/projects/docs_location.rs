//! KT-263 — where a project keeps its AI doc, and overriding it.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::docs_location::{self, DocsDirSource};
use crate::models::{ApiErrorCode, ApiResponse};
use crate::AppState;

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ProjectDocsLocation {
    /// The project's own override, as saved.
    pub override_dir: Option<String>,
    /// The global default from the settings: it pre-fills the briefing and
    /// never applies by itself (KT-1192).
    pub default_dir: Option<String>,
    /// The folder in use, relative to the project root; `None` when refused.
    pub effective_dir: Option<String>,
    pub source: DocsDirSource,
    /// Why a configured folder is not used.
    pub refused: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetProjectDocsLocation {
    /// `None` or blank clears the override.
    pub dir: Option<String>,
}

fn view(
    project_path: &str,
    override_dir: Option<String>,
    default_dir: Option<String>,
) -> ProjectDocsLocation {
    let root = crate::core::scanner::resolve_host_path(project_path);
    let resolved = docs_location::resolve(&root, override_dir.as_deref());
    ProjectDocsLocation {
        effective_dir: resolved.relative(&root),
        source: resolved.source,
        refused: resolved.refused,
        override_dir,
        default_dir,
    }
}

fn coded(status: StatusCode, code: ApiErrorCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiResponse::<ProjectDocsLocation>::err_coded(code, message)),
    )
        .into_response()
}

/// Re-key a project's override after its path changed (`new_path`) or the
/// project was deleted (`None`): the process-wide map is keyed by path.
pub async fn rekey(state: &AppState, project_id: &str, old_path: &str, new_path: Option<&str>) {
    docs_location::set_project_override(old_path, None);
    let Some(new_path) = new_path else {
        return;
    };
    let id = project_id.to_string();
    match state
        .db
        .with_read_conn(move |conn| crate::db::projects::project_docs_dir(conn, &id))
        .await
    {
        Ok(dir) => docs_location::set_project_override(new_path, dir.as_deref()),
        Err(error) => tracing::warn!("AI doc folder of {project_id} not re-keyed: {error}"),
    }
}

/// GET /api/projects/{id}/docs-location
pub async fn get(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let found = state
        .db
        .with_read_conn(move |conn| {
            let Some(project) = crate::db::projects::get_project(conn, &id)? else {
                return Ok(None);
            };
            let dir = crate::db::projects::project_docs_dir(conn, &id)?;
            Ok(Some((project.path, dir)))
        })
        .await;
    match found {
        Ok(Some((path, dir))) => {
            let default_dir = state.config.read().await.scan.default_docs_dir.clone();
            Json(ApiResponse::ok(view(&path, dir, default_dir))).into_response()
        }
        Ok(None) => coded(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            "Project not found",
        ),
        Err(error) => coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            error.to_string(),
        ),
    }
}

/// PUT /api/projects/{id}/docs-location — a folder that is absolute, uses
/// `..`, or reaches outside the project through a symlink is a 400.
pub async fn set(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SetProjectDocsLocation>,
) -> Response {
    let lookup = id.clone();
    let project = match state
        .db
        .with_read_conn(move |conn| crate::db::projects::get_project(conn, &lookup))
        .await
    {
        Ok(Some(project)) => project,
        Ok(None) => {
            return coded(
                StatusCode::NOT_FOUND,
                ApiErrorCode::NotFound,
                "Project not found",
            )
        }
        Err(error) => {
            return coded(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Internal,
                error.to_string(),
            )
        }
    };

    let requested = request
        .dir
        .as_deref()
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(str::to_string);
    let normalized = match requested {
        None => None,
        Some(dir) => {
            let path = project.path.clone();
            let checked = tokio::task::spawn_blocking(move || {
                let root = crate::core::scanner::resolve_host_path(&path);
                docs_location::validate_for_project(&root, &dir)
            })
            .await
            .unwrap_or_else(|error| Err(format!("check failed: {error}")));
            match checked {
                Ok(rel) => Some(rel),
                Err(reason) => {
                    return coded(StatusCode::BAD_REQUEST, ApiErrorCode::Validation, reason)
                }
            }
        }
    };

    let save_id = id.clone();
    let saved = normalized.clone();
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            crate::db::projects::set_project_docs_dir(conn, &save_id, saved.as_deref())
        })
        .await
    {
        return coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            error.to_string(),
        );
    }
    docs_location::set_project_override(&project.path, normalized.as_deref());
    let default_dir = state.config.read().await.scan.default_docs_dir.clone();
    Json(ApiResponse::ok(view(
        &project.path,
        normalized,
        default_dir,
    )))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_reports_the_override_its_folder_and_a_refusal() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().to_string_lossy().into_owned();
        let v = view(&path, Some(".ai".into()), None);
        assert_eq!(v.effective_dir.as_deref(), Some(".ai"));
        assert_eq!(v.source, DocsDirSource::ProjectOverride);
        assert_eq!(v.refused, None);

        let v = view(&path, None, None);
        assert_eq!(v.effective_dir.as_deref(), Some("docs"));
        assert_eq!(v.source, DocsDirSource::Detected);

        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(outside.path(), t.path().join("ctx")).unwrap();
            let v = view(&path, Some("ctx".into()), None);
            assert_eq!(v.effective_dir, None);
            assert!(v.refused.unwrap().contains("symlink"));
        }
    }
}
