//! Debug endpoints — backs the Settings > Debug viewer.
//!
//! The UI polls `GET /api/debug/logs?lines=N` to paint a scrollable
//! tail of the backend's recent `tracing` events. Source is the in-memory
//! ringbuffer defined in `core::log_buffer`; no file on disk, no Docker
//! socket required.

use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::core::log_buffer::{LOG_BUFFER, DEFAULT_CAPACITY};
use crate::models::ApiResponse;
use crate::AppState;

/// Default number of lines returned when the caller doesn't specify `lines`.
/// Picked to fill a couple of screens on a regular monitor while staying
/// cheap to serialize.
const DEFAULT_TAIL_LINES: usize = 200;
/// Hard cap so a malicious/careless caller can't ask for gigabytes.
const MAX_TAIL_LINES: usize = 2000;

#[derive(Debug, Default, Deserialize)]
pub struct LogsQuery {
    /// How many lines to return, counted from the tail (most recent last).
    /// Clamped to [0, MAX_TAIL_LINES]. Defaults to DEFAULT_TAIL_LINES.
    pub lines: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct LogsResponse {
    /// Most-recent lines, oldest-first — ready to paste into a `<pre>`.
    pub lines: Vec<String>,
    /// Total buffered lines across all levels — useful for the viewer's
    /// "N events captured" hint.
    pub buffered: usize,
    /// Max capacity of the ringbuffer (informational).
    pub capacity: usize,
    /// Whether backend debug mode is active. The UI uses this to decide
    /// whether to show a "turn on debug for verbose logs" callout —
    /// captures still happen at `info` even when debug is off.
    pub debug_mode: bool,
}

/// GET /api/debug/logs?lines=200
pub async fn get_logs(
    State(state): State<AppState>,
    Query(query): Query<LogsQuery>,
) -> Json<ApiResponse<LogsResponse>> {
    let lines_requested = query.lines.unwrap_or(DEFAULT_TAIL_LINES).min(MAX_TAIL_LINES);
    let lines = LOG_BUFFER.tail(lines_requested);
    let buffered = LOG_BUFFER.len();
    let debug_mode = state.config.read().await.server.debug_mode;
    Json(ApiResponse::ok(LogsResponse {
        lines,
        buffered,
        capacity: DEFAULT_CAPACITY,
        debug_mode,
    }))
}

/// POST /api/debug/logs/clear
/// Empty the ringbuffer. Handy when diagnosing a specific scenario: clear,
/// reproduce, capture.
pub async fn clear_logs(State(_state): State<AppState>) -> Json<ApiResponse<()>> {
    LOG_BUFFER.clear();
    Json(ApiResponse::ok(()))
}
