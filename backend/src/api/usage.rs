//! 0.8.7 — `GET /api/usage` : agent CLI usage / cost report via `ccusage`.
//!
//! Thin HTTP wrapper around `core::usage::fetch_usage`. Returns the global
//! spend of the current day / week / month (or the whole history) across the
//! detected CLIs (Claude / Codex / Gemini …), read from their local logs. See `core::usage` for the scope
//! discipline (global, not per-Kronn-project in 0.8.7).

use axum::{extract::Query, Json};
use serde::Deserialize;

use crate::core::usage::{self, UsageReport};
use crate::models::ApiResponse;

#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    /// `daily` (default) | `weekly` | `monthly`. Validated server-side.
    #[serde(default)]
    pub period: Option<String>,
    /// `true` reads the whole log history instead of the current day / week /
    /// month (the default window, #270). For callers that average over time.
    #[serde(default)]
    pub history: bool,
}

/// GET /api/usage?period=daily[&history=true]
pub async fn get_usage(Query(q): Query<UsageQuery>) -> Json<ApiResponse<UsageReport>> {
    let period = q.period.as_deref().unwrap_or("daily");
    match usage::fetch_usage(period, q.history).await {
        Ok(report) => Json(ApiResponse::ok(report)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}
