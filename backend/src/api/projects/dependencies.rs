use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::{Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Deserialize)]
pub struct DependencyMonitoringRequest {
    /// `None` disables opportunistic checks; manual refresh remains available.
    pub interval_days: Option<u16>,
}

#[derive(Debug, Serialize)]
pub struct DependencyMonitoringResponse {
    pub interval_days: Option<u16>,
}

fn apply_monitoring_schedule(summary: &mut DependencyUpdateSummary, interval_days: Option<u16>) {
    summary.monitoring_interval_days = interval_days;
    summary.next_check_at =
        interval_days.map(|days| summary.checked_at + ChronoDuration::days(i64::from(days)));
}

fn monitoring_result_is_current(
    summary: &DependencyUpdateSummary,
    interval_days: Option<u16>,
) -> bool {
    interval_days
        .is_none_or(|days| summary.checked_at + ChronoDuration::days(i64::from(days)) > Utc::now())
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
    let id_for_read = id.clone();
    let (project, monitoring) = match state
        .db
        .with_read_conn(move |conn| {
            let Some(project) = crate::db::projects::get_project(conn, &id_for_read)? else {
                return Ok(None);
            };
            let monitoring = crate::db::projects::get_dependency_monitoring(conn, &id_for_read)?;
            Ok(Some((project, monitoring)))
        })
        .await
    {
        Ok(Some(values)) => values,
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
                apply_monitoring_schedule(&mut summary, monitoring.interval_days);
                return Json(ApiResponse::ok(summary));
            }
        }
    }

    if !query.refresh && monitoring.manifest_fingerprint == Some(fingerprint) {
        if let Some(mut summary) = monitoring.summary {
            if monitoring_result_is_current(&summary, monitoring.interval_days) {
                summary.cached = true;
                apply_monitoring_schedule(&mut summary, monitoring.interval_days);
                state.dependency_update_cache.lock().await.insert(
                    cache_key,
                    dependency_updates::CachedDependencyUpdates {
                        fingerprint,
                        inserted_at: Instant::now(),
                        summary: summary.clone(),
                    },
                );
                return Json(ApiResponse::ok(summary));
            }
        }
    }

    let mut summary = dependency_updates::inspect_dependency_updates(&root).await;
    apply_monitoring_schedule(&mut summary, monitoring.interval_days);
    state.dependency_update_cache.lock().await.insert(
        cache_key,
        dependency_updates::CachedDependencyUpdates {
            fingerprint,
            inserted_at: Instant::now(),
            summary: summary.clone(),
        },
    );
    let id_for_write = id.clone();
    let summary_for_write = summary.clone();
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            crate::db::projects::save_dependency_scan(
                conn,
                &id_for_write,
                fingerprint,
                &summary_for_write,
            )
        })
        .await
    {
        tracing::warn!(
            project_id = %id,
            "Could not persist dependency scan result: {error}"
        );
    }
    Json(ApiResponse::ok(summary))
}

/// PUT /api/projects/:id/dependency-updates
///
/// Configure the opportunistic read-only check cadence. The endpoint never
/// executes a package-manager mutation; `interval_days = null` means manual
/// checks only.
pub async fn set_dependency_monitoring(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<DependencyMonitoringRequest>,
) -> Json<ApiResponse<DependencyMonitoringResponse>> {
    if request
        .interval_days
        .is_some_and(|days| !(1..=365).contains(&days))
    {
        return Json(ApiResponse::err(
            "Dependency monitoring interval must be between 1 and 365 days",
        ));
    }

    let id_for_write = id.clone();
    let interval_days = request.interval_days;
    match state
        .db
        .with_conn(move |conn| {
            let Some(project) = crate::db::projects::get_project(conn, &id_for_write)? else {
                return Ok(None);
            };
            crate::db::projects::set_dependency_monitoring_interval(
                conn,
                &id_for_write,
                interval_days,
            )?;
            Ok(Some(project.path))
        })
        .await
    {
        Ok(Some(project_path)) => {
            let root = scanner::resolve_host_path(&project_path);
            state
                .dependency_update_cache
                .lock()
                .await
                .remove(&dependency_updates::cache_key(&root));
            Json(ApiResponse::ok(DependencyMonitoringResponse {
                interval_days,
            }))
        }
        Ok(None) => Json(ApiResponse::err("Project not found")),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(checked_at: chrono::DateTime<Utc>) -> DependencyUpdateSummary {
        DependencyUpdateSummary {
            managers: Vec::new(),
            total_outdated: 0,
            total_major: 0,
            checked_at,
            cached: false,
            monitoring_interval_days: Some(7),
            next_check_at: None,
        }
    }

    #[test]
    fn manual_monitoring_never_becomes_due_implicitly() {
        let old = summary(Utc::now() - ChronoDuration::days(400));
        assert!(monitoring_result_is_current(&old, None));
    }

    #[test]
    fn configured_monitoring_becomes_due_after_interval() {
        let recent = summary(Utc::now() - ChronoDuration::days(6));
        let stale = summary(Utc::now() - ChronoDuration::days(8));
        assert!(monitoring_result_is_current(&recent, Some(7)));
        assert!(!monitoring_result_is_current(&stale, Some(7)));
    }

    #[test]
    fn schedule_metadata_uses_the_scan_timestamp() {
        let checked_at = Utc::now() - ChronoDuration::hours(2);
        let mut value = summary(checked_at);
        apply_monitoring_schedule(&mut value, Some(14));
        assert_eq!(value.monitoring_interval_days, Some(14));
        assert_eq!(
            value.next_check_at,
            Some(checked_at + ChronoDuration::days(14))
        );
    }
}
