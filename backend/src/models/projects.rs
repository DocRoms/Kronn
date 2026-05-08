// Projects & Repositories + AI Audit pipeline + Drift detection.
//
// Three closely-related domains kept together:
//   - Project / DetectedRepo / TokenOverride / AiConfigStatus
//   - Audit progress, request/response, info (files + TODOs + tech debt)
//   - Drift detection (which sections of `docs/` went stale since the
//     last audit, and the partial-audit request that re-fills them)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::AgentType;

// ─── Projects & Repositories ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub repo_url: Option<String>,
    pub token_override: Option<TokenOverride>,
    pub ai_config: AiConfigStatus,
    #[serde(default)]
    pub audit_status: AiAuditStatus,
    #[serde(default)]
    pub ai_todo_count: u32,
    /// True when the project still uses the legacy `ai/index.md` layout
    /// and no migrated `docs/AGENTS.md` exists. Computed by
    /// `enrich_audit_status` — drives the migration banner on
    /// `ProjectCard`. Not persisted in DB.
    #[serde(default)]
    pub needs_docs_migration: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_skill_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub briefing_notes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TokenOverride {
    pub provider: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AiConfigStatus {
    pub detected: bool,
    pub configs: Vec<AiConfigType>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AiConfigType {
    ClaudeMd,       // CLAUDE.md
    ClauseDir,      // .claude/
    AiDir,          // .ai/
    CursorRules,    // .cursorrules
    ContinueDev,    // .continue/
    McpJson,        // .mcp.json
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DetectedRepo {
    pub path: String,
    pub name: String,
    pub remote_url: Option<String>,
    pub branch: String,
    pub ai_configs: Vec<AiConfigType>,
    pub has_project: bool,
    pub hidden: bool,
}

// ─── AI Audit ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AiAuditStatus {
    #[default]
    NoTemplate,
    TemplateInstalled,
    Bootstrapped,
    Audited,
    Validated,
}

/// Live progress of a running audit, exposed via `GET /api/projects/:id/audit-status`.
///
/// Produced by the three SSE streams (`run_audit`, `partial_audit`, `full_audit`)
/// which write into `AppState.audit_tracker.progress` as they advance. The UI
/// polls this endpoint to "resume" the progress bar when the user navigates
/// away and comes back — no need to restart the audit since the server-side
/// process keeps running.
///
/// The struct is deliberately thin: it carries what's needed to paint a
/// progress bar, not the full audit content (that still flows through SSE
/// when the user is actively connected).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditProgress {
    pub project_id: String,
    /// `"installing"` during template install, `"auditing"` during the 10-step
    /// loop, `"validating"` during phase 3 (validation discussion creation),
    /// `"done"` briefly before the tracker clears the entry.
    pub phase: String,
    pub step_index: u32,
    pub total_steps: u32,
    /// `ai/` file currently being produced (e.g. `"repo-map.md"`), or
    /// `"Final review"` for the last step, or `None` between steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_file: Option<String>,
    pub started_at: DateTime<Utc>,
    /// `"full"` for the 10-step audit, `"partial"` for drift-triggered
    /// sub-audits, `"full_audit"` for the end-to-end variant. Kept as a
    /// string so future audit kinds don't force a schema migration.
    pub kind: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct LaunchAuditRequest {
    pub agent: AgentType,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct BootstrapProjectRequest {
    pub name: String,
    pub description: String,
    pub agent: AgentType,
    #[serde(default)]
    pub mcp_config_ids: Vec<String>,
    #[serde(default)]
    pub skill_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct BootstrapProjectResponse {
    pub project_id: String,
    pub discussion_id: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CloneProjectRequest {
    pub url: String,
    #[serde(default)]
    pub name: Option<String>,
    pub agent: AgentType,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct CloneProjectResponse {
    pub project_id: String,
    pub discussion_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RemoteRepo {
    pub name: String,
    pub full_name: String,
    pub clone_url: String,
    pub ssh_url: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub stargazers_count: u32,
    pub updated_at: String,
    pub source: String,  // "github" or "gitlab"
    pub already_cloned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RepoSource {
    pub id: String,           // MCP config id, or "env:github" / "env:gitlab"
    pub label: String,        // MCP config label, or "GitHub (env)" / "GitLab (env)"
    pub provider: String,     // "github" or "gitlab"
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct DiscoverReposRequest {
    #[serde(default)]
    pub source_ids: Vec<String>,  // empty = use all available sources
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscoverReposResponse {
    pub repos: Vec<RemoteRepo>,
    pub sources: Vec<String>,
    pub available_sources: Vec<RepoSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditInfo {
    pub files: Vec<AuditFileInfo>,
    pub todos: Vec<AuditTodo>,
    #[serde(default)]
    pub tech_debt_items: Vec<TechDebtItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TechDebtItem {
    pub id: String,
    pub problem: String,
    pub area: String,
    pub severity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditFileInfo {
    pub path: String,
    pub filled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditTodo {
    pub file: String,
    pub line: u32,
    pub text: String,
}

// ─── Audit Drift Detection ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DriftCheckResponse {
    pub audit_date: Option<String>,
    pub stale_sections: Vec<DriftSection>,
    pub fresh_sections: Vec<String>,
    pub total_sections: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DriftSection {
    pub ai_file: String,
    pub audit_step: usize,
    pub changed_sources: Vec<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct PartialAuditRequest {
    pub agent: AgentType,
    pub steps: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct StartBriefingResponse {
    pub discussion_id: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetBriefingRequest {
    pub notes: Option<String>,
}
