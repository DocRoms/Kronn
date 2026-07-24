// Data models for the Kronn backend. Most domain types live in their own
// sub-modules (TD-20260417-models-monolith); this file holds the
// cross-cutting helpers + a handful of small viewer/response shapes that
// don't need a dedicated home, and re-exports each sub-module via
// `pub use *` so `use crate::models::*` keeps working at the call sites.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

pub mod agent_decisions;
pub mod agents;
pub mod bundle;
pub mod db;
pub mod discussions;
pub mod git;
pub mod learnings;
pub mod mcp;
pub mod multiuser;
pub mod ollama;
pub mod projects;
pub mod quick;
pub mod setup;
pub mod stats;
pub mod workflows;

pub use agent_decisions::*;
pub use agents::*;
pub use bundle::*;
pub use db::*;
pub use discussions::*;
pub use git::*;
pub use learnings::*;
pub use mcp::*;
pub use multiuser::*;
pub use ollama::*;
pub use projects::*;
pub use quick::*;
pub use setup::*;
pub use stats::*;
pub use workflows::*;

/// Deserialize an optional field that distinguishes between absent, null, and present.
/// - Absent key → `None` (outer Option is None → use existing value)
/// - Explicit null → `Some(None)` (set to null)
/// - Present value → `Some(Some(value))` (set to value)
pub(crate) fn deserialize_optional_field<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Some(Option::deserialize(deserializer)?))
}

// ─── AI Documentation Files (read-only viewer) ────────────────────────────

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AiFileNode {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<AiFileNode>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AiFileContent {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AiSearchResult {
    pub path: String,
    pub match_count: u32,
}

// ─── Generic API response wrappers ────────────────────────────────────────

/// D11 (0.8.11) — machine-readable error category, so the frontend and the MCP
/// tools can branch on the KIND of failure instead of string-matching `error`.
/// Serialized as a stable snake_case string in `ApiResponse.error_code`.
/// Introduced incrementally: handlers opt in via `ApiResponse::err_coded`;
/// legacy `ApiResponse::err` leaves `error_code` unset (back-compatible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ApiErrorCode {
    /// The requested resource does not exist (→ HTTP 404 semantics).
    NotFound,
    /// The request is malformed / fails a business rule (→ 400/422).
    Validation,
    /// The request conflicts with current state (→ 409).
    Conflict,
    /// An unexpected server-side failure (→ 500).
    Internal,
}

impl ApiErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApiErrorCode::NotFound => "not_found",
            ApiErrorCode::Validation => "validation",
            ApiErrorCode::Conflict => "conflict",
            ApiErrorCode::Internal => "internal",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
    /// D11 — stable error category (see `ApiErrorCode`). Omitted from the wire
    /// when unset so legacy clients + untouched handlers are unaffected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            error_code: None,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg.into()),
            error_code: None,
        }
    }

    /// Error response with a machine-readable category. Prefer this over `err`
    /// in new/updated handlers so the frontend + MCP can branch on the kind.
    pub fn err_coded(code: ApiErrorCode, msg: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg.into()),
            error_code: Some(code.as_str().to_string()),
        }
    }
}

/// Paginated API response — wraps a list with total count + page info.
#[derive(Debug, Serialize)]
pub struct PaginatedResponse<T: Serialize> {
    pub items: Vec<T>,
    pub total: u32,
    pub page: u32,
    pub per_page: u32,
}

/// Query params for paginated endpoints.
///
/// IMPORTANT: `page` defaults to 0 as a sentinel for "no pagination requested".
/// Callers (e.g. `api::discussions::list`) extract `Query<PaginationQuery>` and
/// only paginate when `page > 0` — a bare `GET /api/discussions` returns
/// everything (we hit a regression on 2026-04-13 where defaulting `page` to 1
/// silently capped >50-item lists). Pre-axum-0.8 the same intent was expressed
/// via `Option<Query<…>>`, but axum 0.8 dropped `OptionalFromRequestParts` for
/// `Query`, so we use the `0` sentinel instead.
#[derive(Debug, Deserialize, Default)]
pub struct PaginationQuery {
    #[serde(default)]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

fn default_per_page() -> u32 {
    50
}

// ─── Context Files (uploaded file context for discussions) ────────────────

/// A file uploaded as context for a discussion.
/// Content is extracted to text at upload time and stored in DB.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ContextFile {
    pub id: String,
    pub discussion_id: String,
    pub filename: String,
    pub mime_type: String,
    pub original_size: u64,
    pub extracted_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_path: Option<String>,
    /// The message this file is attached to. `None` = pending (still staged in
    /// the composer) or a legacy disc-wide file. Always serialized (even when
    /// null) so the frontend can split pending-vs-attached without ambiguity.
    pub message_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Response after uploading a context file.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UploadContextFileResponse {
    pub file: ContextFile,
    /// Suggested skill IDs based on file extension
    pub suggested_skills: Vec<String>,
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
