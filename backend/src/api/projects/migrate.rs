// `POST /api/projects/:id/migrate-docs` — flips a legacy `ai/` project
// to the modern `docs/AGENTS.md` layout (Kronn 0.7.1+). Idempotent on
// already-migrated and never-bootstrapped projects. The actual move +
// cross-ref rewrite lives in `core::docs_migration`; this is the HTTP
// glue + response shape.

use axum::{extract::{Path, State}, Json};

use crate::core::scanner;
use crate::models::*;
use crate::AppState;

#[derive(Debug, serde::Deserialize)]
pub struct MigrateDocsRequest {
    /// Create a `ai → docs` symlink for retro-compat after the move.
    /// Defaults to true. Operators can opt out via `{"create_symlink": false}`.
    #[serde(default = "default_true")]
    pub create_symlink: bool,
}

fn default_true() -> bool { true }

#[derive(Debug, serde::Serialize)]
pub struct MigrateDocsResponse {
    pub status: &'static str,
    /// Files moved on success. 0 on no-op.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_moved: Option<usize>,
    /// Path refs rewritten (cross-refs in markdown + root redirectors).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refs_rewritten: Option<usize>,
    /// Whether a `ai → docs` symlink was created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symlink_created: Option<bool>,
    /// Reason on failure / no-op.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// POST /api/projects/:id/migrate-docs
///
/// Migrate the project's legacy `ai/` folder to the modern `docs/AGENTS.md`
/// convention (Kronn 0.7.1+). Idempotent: safe to call on already-migrated
/// or never-bootstrapped projects.
pub async fn migrate_docs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<MigrateDocsRequest>>,
) -> Json<ApiResponse<MigrateDocsResponse>> {
    let req = body.map(|Json(b)| b).unwrap_or(MigrateDocsRequest { create_symlink: true });

    let pid = id.clone();
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &pid)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path = scanner::resolve_host_path(&project.path);
    let outcome = crate::core::docs_migration::migrate_project(&project_path, req.create_symlink).await;

    use crate::core::docs_migration::MigrationOutcome;
    let response = match outcome {
        MigrationOutcome::Migrated { files_moved, refs_rewritten, symlink_created } => {
            MigrateDocsResponse {
                status: "migrated",
                files_moved: Some(files_moved),
                refs_rewritten: Some(refs_rewritten),
                symlink_created: Some(symlink_created),
                reason: None,
            }
        }
        MigrationOutcome::AlreadyMigrated => MigrateDocsResponse {
            status: "already_migrated",
            files_moved: None, refs_rewritten: None, symlink_created: None, reason: None,
        },
        MigrationOutcome::NotApplicable => MigrateDocsResponse {
            status: "not_applicable",
            files_moved: None, refs_rewritten: None, symlink_created: None,
            reason: Some("Project has no `ai/` directory to migrate.".into()),
        },
        MigrationOutcome::Failed { reason } => {
            return Json(ApiResponse::err(reason));
        }
    };
    Json(ApiResponse::ok(response))
}
