use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use std::time::Instant;

use crate::core::{dependency_updates, scanner};
use crate::models::{ApiResponse, DependencyUpdateSummary};
use crate::AppState;

#[derive(Debug, Default, Deserialize)]
pub struct DependencyUpdatesQuery {
    /// Ignore the in-memory cache and query package registries again.
    #[serde(default)]
    pub refresh: bool,
}

/// GET /api/projects/:id/dependency-updates
///
/// Discovers supported manifests, then runs bounded read-only checks through
/// their package managers. Results are cached because most managers consult a
/// remote registry; modifying a manifest invalidates the cache immediately.
pub async fn dependency_updates(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DependencyUpdatesQuery>,
) -> Json<ApiResponse<DependencyUpdateSummary>> {
    let project = match state
        .db
        .with_read_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(project)) => project,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
    };
    let root = scanner::resolve_host_path(&project.path);
    if !root.is_dir() {
        return Json(ApiResponse::err(format!(
            "Project path not found: {}",
            root.display()
        )));
    }

    let fingerprint_root = root.clone();
    let fingerprint = match tokio::task::spawn_blocking(move || {
        dependency_updates::manifest_fingerprint(&fingerprint_root)
    })
    .await
    {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            return Json(ApiResponse::err(format!(
                "Dependency scan task failed: {error}"
            )))
        }
    };
    let cache_key = dependency_updates::cache_key(&root);

    if !query.refresh {
        let cache = state.dependency_update_cache.lock().await;
        if let Some(cached) = cache.get(&cache_key) {
            if cached.fingerprint == fingerprint
                && cached.inserted_at.elapsed() < dependency_updates::CACHE_TTL
            {
                let mut summary = cached.summary.clone();
                summary.cached = true;
                return Json(ApiResponse::ok(summary));
            }
        }
    }

    let summary = dependency_updates::inspect_dependency_updates(&root).await;
    state.dependency_update_cache.lock().await.insert(
        cache_key,
        dependency_updates::CachedDependencyUpdates {
            fingerprint,
            inserted_at: Instant::now(),
            summary: summary.clone(),
        },
    );
    Json(ApiResponse::ok(summary))
}
