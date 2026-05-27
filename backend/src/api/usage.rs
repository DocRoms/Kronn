//! 0.8.7 — `GET /api/usage` : agent CLI usage / cost report via `ccusage`.
//!
//! Thin HTTP wrapper around `core::usage::fetch_usage`. Returns the global
//! daily / weekly / monthly spend across the detected CLIs (Claude / Codex /
//! Gemini …), read from their local logs. See `core::usage` for the scope
//! discipline (global, not per-Kronn-project in 0.8.7).

use axum::{
    extract::Query,
    Json,
};
use serde::Deserialize;

use crate::core::usage::{self, UsageReport};
use crate::models::ApiResponse;

#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    /// `daily` (default) | `weekly` | `monthly`. Validated server-side.
    #[serde(default)]
    pub period: Option<String>,
}

/// GET /api/usage?period=daily
pub async fn get_usage(Query(q): Query<UsageQuery>) -> Json<ApiResponse<UsageReport>> {
    let period = q.period.as_deref().unwrap_or("daily");
    match usage::fetch_usage(period).await {
        Ok(report) => Json(ApiResponse::ok(report)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}
