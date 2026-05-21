//! HTTP surface for the unified API-call logs (0.8.6 #24).
//!
//! `GET /api/api-call-logs?source=…&project_id=…&plugin_slug=…&status=…&limit=N`
//! returns the most recent rows (newest first), capped at `limit` (default 100,
//! max 1000). `GET /api/api-call-logs/:id` returns a single row for the
//! detail drawer. `POST /api/api-call-logs/purge` deletes rows older than
//! `days` (default 30).

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::models::ApiResponse;
use crate::db::api_call_logs::{self, ApiCallLog, ApiCallSource, ApiCallStatus, ListFilter};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub source: Option<String>,
    pub project_id: Option<String>,
    pub plugin_slug: Option<String>,
    pub status: Option<String>,
    pub limit: Option<u32>,
}

fn parse_source(s: &str) -> Option<ApiCallSource> {
    match s {
        "workflow" => Some(ApiCallSource::Workflow),
        "agent_broker" => Some(ApiCallSource::AgentBroker),
        "manual_test" => Some(ApiCallSource::ManualTest),
        _ => None,
    }
}

fn parse_status(s: &str) -> Option<ApiCallStatus> {
    match s {
        "OK" => Some(ApiCallStatus::Ok),
        "ERROR" => Some(ApiCallStatus::Error),
        "RateLimited" => Some(ApiCallStatus::RateLimited),
        "TimedOut" => Some(ApiCallStatus::TimedOut),
        _ => None,
    }
}

pub async fn list_api_call_logs(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Json<ApiResponse<Vec<ApiCallLog>>> {
    let source = q.source.as_deref().and_then(parse_source);
    let status = q.status.as_deref().and_then(parse_status);
    let project_id = q.project_id.clone();
    let plugin_slug = q.plugin_slug.clone();
    let limit = q.limit;
    match state
        .db
        .with_conn(move |conn| {
            api_call_logs::list(
                conn,
                ListFilter {
                    source,
                    project_id: project_id.as_deref(),
                    plugin_slug: plugin_slug.as_deref(),
                    status,
                    limit,
                },
            )
            .map_err(|e| anyhow::anyhow!("list api_call_logs: {e}"))
        })
        .await
    {
        Ok(rows) => Json(ApiResponse::ok(rows)),
        Err(e) => Json(ApiResponse::err(format!("{e}"))),
    }
}

pub async fn get_api_call_log(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Option<ApiCallLog>>> {
    match state
        .db
        .with_conn(move |conn| {
            api_call_logs::get(conn, &id).map_err(|e| anyhow::anyhow!("get api_call_log: {e}"))
        })
        .await
    {
        Ok(row) => Json(ApiResponse::ok(row)),
        Err(e) => Json(ApiResponse::err(format!("{e}"))),
    }
}

#[derive(Debug, Deserialize)]
pub struct PurgeRequest {
    pub days: Option<u32>,
}

pub async fn purge_api_call_logs(
    State(state): State<AppState>,
    Json(req): Json<PurgeRequest>,
) -> Json<ApiResponse<usize>> {
    let days = req.days.unwrap_or(30).clamp(1, 365);
    match state
        .db
        .with_conn(move |conn| {
            api_call_logs::purge_older_than(conn, days)
                .map_err(|e| anyhow::anyhow!("purge api_call_logs: {e}"))
        })
        .await
    {
        Ok(n) => Json(ApiResponse::ok(n)),
        Err(e) => Json(ApiResponse::err(format!("{e}"))),
    }
}
