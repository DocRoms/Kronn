// Data models for the Kronn backend. Most domain types live in their own
// sub-modules (TD-20260417-models-monolith); this file holds the
// cross-cutting helpers + a handful of small viewer/response shapes that
// don't need a dedicated home, and re-exports each sub-module via
// `pub use *` so `use crate::models::*` keeps working at the call sites.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

pub mod agents;
pub mod db;
pub mod discussions;
pub mod git;
pub mod mcp;
pub mod multiuser;
pub mod ollama;
pub mod projects;
pub mod quick;
pub mod setup;
pub mod stats;
pub mod workflows;

pub use agents::*;
pub use db::*;
pub use discussions::*;
pub use git::*;
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

#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self { success: true, data: Some(data), error: None }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self { success: false, data: None, error: Some(msg.into()) }
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

fn default_per_page() -> u32 { 50 }

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
