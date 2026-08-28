use std::path::PathBuf;

use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::{core::portable_library as portable, models::ApiResponse, AppState};

#[derive(Debug, Deserialize)]
pub struct LibraryQuery {
    pub project_id: Option<String>,
    pub search: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LibraryState {
    pub scope: portable::LibraryScope,
    pub project_id: Option<String>,
    pub items: Vec<LibraryItemView>,
    pub drift: String,
    pub approved: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LibraryItemView {
    pub kind: portable::LibraryKind,
    pub id: String,
    pub scope: portable::LibraryScope,
    pub source: String,
    pub content_sha256: String,
    pub content: String,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    pub project_id: Option<String>,
    pub items: Vec<LibraryItemView>,
}

async fn roots(
    state: &AppState,
    project_id: Option<&str>,
) -> Result<(PathBuf, Option<PathBuf>), String> {
    let global = crate::core::config::config_dir()
        .map_err(|e| e.to_string())?
        .join(".agents");
    let Some(id) = project_id else {
        return Ok((global, None));
    };
    let id = id.to_owned();
    let project = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "project not found".to_string())?;
    let path = PathBuf::from(project.path);
    if !path.is_dir() {
        return Err("project path is unavailable".into());
    }
    Ok((global, Some(path)))
}

pub async fn state(
    Query(query): Query<LibraryQuery>,
    State(state): State<AppState>,
) -> Json<ApiResponse<LibraryState>> {
    Json(match state_inner(&state, &query).await {
        Ok(value) => ApiResponse::ok(value),
        Err(e) => ApiResponse::err(e),
    })
}

async fn state_inner(state: &AppState, query: &LibraryQuery) -> Result<LibraryState, String> {
    let (global, project) = roots(state, query.project_id.as_deref()).await?;
    let catalog = portable::discover(
        Some(&global),
        project.as_ref().map(|p| p.join(".agents")).as_deref(),
    )?;
    let items = catalog
        .search(query.search.as_deref().unwrap_or(""))
        .into_iter()
        .map(|item| LibraryItemView {
            kind: item.kind,
            id: item.id.clone(),
            scope: item.sidecar.provenance.scope,
            source: item.sidecar.provenance.source.clone(),
            content_sha256: item.sidecar.provenance.content_sha256.clone(),
            content: String::from_utf8_lossy(&item.content).into_owned(),
            data: item.sidecar.data.clone(),
        })
        .collect();
    let (drift, approved) = match &project {
        None => ("not_applicable".to_string(), false),
        Some(path) => match portable::check_frozen_hash(path) {
            Ok(_) => (
                "clean".to_string(),
                portable::is_lock_approved(path).unwrap_or(false),
            ),
            Err(e) if e.starts_with("cannot read") => ("unsynced".to_string(), false),
            Err(_) => ("drifted".to_string(), false),
        },
    };
    Ok(LibraryState {
        scope: if project.is_some() {
            portable::LibraryScope::Project
        } else {
            portable::LibraryScope::Global
        },
        project_id: query.project_id.clone(),
        items,
        drift,
        approved,
    })
}

pub async fn sync(
    Query(query): Query<LibraryQuery>,
    State(state): State<AppState>,
) -> Json<ApiResponse<portable::SyncReport>> {
    Json(
        match roots(&state, query.project_id.as_deref())
            .await
            .and_then(|(global, project)| {
                let project =
                    project.ok_or_else(|| "project_id is required for sync".to_string())?;
                let catalog = portable::discover(Some(&global), Some(&project.join(".agents")))?;
                portable::sync(&catalog, &project)
            }) {
            Ok(v) => ApiResponse::ok(v),
            Err(e) => ApiResponse::err(e),
        },
    )
}

pub async fn check(
    Query(query): Query<LibraryQuery>,
    State(state): State<AppState>,
) -> Json<ApiResponse<portable::KronnLock>> {
    Json(
        match roots(&state, query.project_id.as_deref())
            .await
            .and_then(|(_, p)| {
                portable::check_frozen_hash(&p.ok_or_else(|| "project_id is required".to_string())?)
            }) {
            Ok(v) => ApiResponse::ok(v),
            Err(e) => ApiResponse::err(e),
        },
    )
}

pub async fn approve(
    Query(query): Query<LibraryQuery>,
    State(state): State<AppState>,
) -> Json<ApiResponse<bool>> {
    Json(
        match roots(&state, query.project_id.as_deref())
            .await
            .and_then(|(_, p)| {
                portable::approve_lock(&p.ok_or_else(|| "project_id is required".to_string())?)
            }) {
            Ok(()) => ApiResponse::ok(true),
            Err(e) => ApiResponse::err(e),
        },
    )
}

pub async fn migrate(
    Query(query): Query<LibraryQuery>,
    State(state): State<AppState>,
) -> Json<ApiResponse<portable::MigrationReport>> {
    Json(
        match migrate_inner(&state, query.project_id.as_deref()).await {
            Ok(v) => ApiResponse::ok(v),
            Err(e) => ApiResponse::err(e),
        },
    )
}

async fn migrate_inner(
    state: &AppState,
    project_id: Option<&str>,
) -> Result<portable::MigrationReport, String> {
    let (global, project) = roots(state, project_id).await?;
    let skills: Vec<_> = crate::core::skills::list_all_skills()
        .into_iter()
        .filter(|v| !v.is_builtin)
        .collect();
    let directives: Vec<_> = crate::core::directives::list_all_directives()
        .into_iter()
        .filter(|v| !v.is_builtin)
        .collect();
    let (qps, workflows) = state
        .db
        .with_conn(crate::db::portable_library_records)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(project) = project {
        let qps: Vec<_> = qps
            .into_iter()
            .filter(|v| v.project_id.as_deref() == project_id)
            .collect();
        let workflows: Vec<_> = workflows
            .into_iter()
            .filter(|v| v.project_id.as_deref() == project_id)
            .collect();
        portable::migrate_legacy(
            &project.join(".agents"),
            portable::LibraryScope::Project,
            &[],
            &[],
            &qps,
            &workflows,
        )
    } else {
        let qps: Vec<_> = qps.into_iter().filter(|v| v.project_id.is_none()).collect();
        let workflows: Vec<_> = workflows
            .into_iter()
            .filter(|v| v.project_id.is_none())
            .collect();
        portable::migrate_legacy(
            &global,
            portable::LibraryScope::Global,
            &skills,
            &directives,
            &qps,
            &workflows,
        )
    }
}

pub async fn export(
    Query(query): Query<LibraryQuery>,
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<LibraryItemView>>> {
    Json(match state_inner(&state, &query).await {
        Ok(v) => ApiResponse::ok(v.items),
        Err(e) => ApiResponse::err(e),
    })
}

pub async fn import(
    State(state): State<AppState>,
    Json(req): Json<ImportRequest>,
) -> Json<ApiResponse<portable::SyncReport>> {
    Json(match import_inner(&state, req).await {
        Ok(v) => ApiResponse::ok(v),
        Err(e) => ApiResponse::err(e),
    })
}

async fn import_inner(
    state: &AppState,
    req: ImportRequest,
) -> Result<portable::SyncReport, String> {
    let (global, project) = roots(state, req.project_id.as_deref()).await?;
    let has_project_item = req
        .items
        .iter()
        .any(|item| item.scope == portable::LibraryScope::Project);
    if has_project_item && project.is_none() {
        return Err("project_id is required to import project-scope items".to_string());
    }
    let staging = project.as_ref().map(|p| p.join(".agents"));
    for item in &req.items {
        let root = match item.scope {
            portable::LibraryScope::Global => &global,
            portable::LibraryScope::Project => staging.as_ref().expect("checked above"),
        };
        portable::import_item(
            root,
            item.kind,
            &item.id,
            &item.content,
            item.data.clone(),
            item.scope,
        )?;
    }
    match project {
        Some(project) => {
            let catalog = portable::discover(Some(&global), staging.as_deref())?;
            portable::sync(&catalog, &project)
        }
        None => Ok(portable::SyncReport::default()),
    }
}
