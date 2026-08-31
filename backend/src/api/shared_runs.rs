use crate::{
    models::{ApiResponse, SharedRun},
    AppState,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ListRunsQuery {
    pub kind: Option<String>,
    pub source_id: Option<String>,
    pub project_id: Option<String>,
    pub discussion_id: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}
fn default_limit() -> u32 {
    50
}

pub async fn persist_and_broadcast(state: &AppState, run: SharedRun) -> anyhow::Result<()> {
    let saved = run.clone();
    state
        .db
        .with_conn(move |conn| crate::db::shared_runs::upsert(conn, &saved))
        .await?;
    let _ = state
        .ws_broadcast
        .send(crate::models::WsMessage::SharedRunUpdated { run_id: run.id });
    Ok(())
}

/// Scoping the run listing by project/discussion is only meaningful when the
/// caller's scope actually exists: an unknown id silently returning an empty
/// list would look identical to "this project/discussion has no runs yet",
/// which hides typos and stale ids from a rehydrating client. Fail closed
/// with an explicit error instead.
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListRunsQuery>,
) -> Json<ApiResponse<Vec<SharedRun>>> {
    if let Some(project_id) = query.project_id.clone() {
        match state
            .db
            .with_conn(move |conn| crate::db::projects::get_project(conn, &project_id))
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => return Json(ApiResponse::err("Unknown project_id scope")),
            Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
        }
    }
    if let Some(discussion_id) = query.discussion_id.clone() {
        match state
            .db
            .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &discussion_id))
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => return Json(ApiResponse::err("Unknown discussion_id scope")),
            Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
        }
    }
    let result = state
        .db
        .with_conn(move |conn| {
            crate::db::shared_runs::list(
                conn,
                query.kind.as_deref(),
                query.source_id.as_deref(),
                query.project_id.as_deref(),
                query.discussion_id.as_deref(),
                query.limit,
            )
        })
        .await;
    match result {
        Ok(runs) => Json(ApiResponse::ok(runs)),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<SharedRun>> {
    match state
        .db
        .with_conn(move |conn| crate::db::shared_runs::get(conn, &id))
        .await
    {
        Ok(Some(run)) => Json(ApiResponse::ok(run)),
        Ok(None) => Json(ApiResponse::err("Run not found")),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}
