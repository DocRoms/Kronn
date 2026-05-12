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
    /// Total tech-debt entries detected in the project's docs tree:
    /// one count per file under `docs/tech-debt/` plus one count per
    /// table row in `docs/inconsistencies-tech-debt.md`. Computed by
    /// `scanner::count_tech_debt` and surfaced as a badge on the
    /// project card so users see at a glance how many TD items remain
    /// to address. Not persisted in DB.
    #[serde(default)]
    pub tech_debt_count: u32,
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

/// One row in the `audit_runs` table — one record per audit invocation.
///
/// Inserted at audit start with `status = Running` and zeroed counts;
/// updated to a terminal status with populated counts when the pipeline
/// finishes. The frontend health badge reads the latest N rows for a
/// project to render the sparkline + delta chip.
///
/// 0.8.2 — see migration 050 for schema. The `kind` field is forward-
/// compatible: we ship with `Full` only and extend to `Security`,
/// `Docker`, etc. in S2 without touching this struct.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditRun {
    pub id: String,
    pub project_id: String,
    /// `Full` | `Drift` | `Security` | `Docker` | `Performance` |
    /// `Accessibility` | `Database` | `ApiDesign` | `Custom`.
    /// Kept as String for forward-compat (new variants don't break
    /// rows already on disk).
    pub kind: String,
    pub agent_type: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u32>,
    /// `Running` while in flight; `Completed` / `Failed` / `Cancelled`
    /// once terminal.
    pub status: String,
    #[serde(default)]
    pub td_critical: u32,
    #[serde(default)]
    pub td_high: u32,
    #[serde(default)]
    pub td_medium: u32,
    #[serde(default)]
    pub td_low: u32,
    #[serde(default)]
    pub td_total: u32,
    #[serde(default)]
    pub td_resolved_since_last: u32,
    #[serde(default)]
    pub td_new_since_last: u32,
    #[serde(default)]
    pub td_carried_over: u32,
    /// 0-100 health score computed by `compute_health_score` at the
    /// moment of completion. `None` while `status == Running`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_score: Option<u8>,
    /// Relative path under the project root, e.g.
    /// `docs/tech-debt/_reconciliation-2026-05-13.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_path: Option<String>,
    /// Raw JSON string of `Vec<AuditRecommendation>`, populated by the
    /// cluster detector in Step 10 (Full audits only). Kept as String
    /// in the model to avoid forcing schema migrations on every
    /// recommendation-shape tweak.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendations_json: Option<String>,
}

/// Recommendation emitted by the Step 10 cluster detector. Lives in
/// `AuditRun.recommendations_json` as a JSON-encoded list.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditRecommendation {
    /// The specialized audit kind to suggest.
    pub kind: String,
    /// Why this kind is recommended — surfaced in the UI tooltip.
    pub reason: String,
    /// Number of TDs that drove the recommendation (the cluster size).
    /// Used to rank multiple recommendations.
    pub cluster_size: u8,
}

/// Compute the 0-100 health score from the severity distribution.
///
/// Calibration: a fresh green-field repo with 0 TDs scores 100. The
/// penalty per severity is biased so a single Critical (12 points) bites
/// harder than 8 Low (2.4 points). Tuned against DOCROMS_WEB's May
/// audit (1C / 6H / 6M / 5L → score 53 = "significant debt, plan a
/// pass"). Clamped to [0, 100].
pub fn compute_health_score(critical: u32, high: u32, medium: u32, low: u32) -> u8 {
    let raw = 100.0
        - (critical as f64 * 12.0)
        - (high     as f64 *  4.0)
        - (medium   as f64 *  1.5)
        - (low      as f64 *  0.3);
    raw.clamp(0.0, 100.0) as u8
}

#[cfg(test)]
mod health_score_tests {
    use super::compute_health_score;

    #[test]
    fn clean_repo_scores_100() {
        assert_eq!(compute_health_score(0, 0, 0, 0), 100);
    }

    #[test]
    fn one_critical_drops_to_88() {
        assert_eq!(compute_health_score(1, 0, 0, 0), 88);
    }

    #[test]
    fn docroms_may_audit_scores_53() {
        // 1 Critical + 6 High + 6 Medium + 5 Low (real distribution
        // from the 2026-05-12 DOCROMS_WEB audit). 100 - 12 - 24 - 9 -
        // 1.5 = 53.5 → 53 after truncation.
        assert_eq!(compute_health_score(1, 6, 6, 5), 53);
    }

    #[test]
    fn catastrophic_clamps_to_zero() {
        assert_eq!(compute_health_score(20, 20, 20, 20), 0);
    }

    #[test]
    fn medium_only_pattern_is_amber() {
        // 10 Medium = 100 - 15 = 85. Healthy yellow zone.
        assert_eq!(compute_health_score(0, 0, 10, 0), 85);
    }

    #[test]
    fn low_only_pattern_stays_green() {
        // 30 Low = 100 - 9 = 91. Still green.
        assert_eq!(compute_health_score(0, 0, 0, 30), 91);
    }
}

/// 0.8.2 — Specialized audit types ("Design C").
///
/// `Full` runs the canonical 10-step pipeline. The other variants run
/// a focused subset that only re-audits one dimension. They share the
/// reconciliation + audit_runs row machinery; only the step list differs.
///
/// `Custom` is the escape hatch: the caller supplies a free-form prompt
/// (single step). All variants are wired through `kind_to_steps()` in
/// `api::audit`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "PascalCase")]
pub enum AuditKind {
    #[default]
    Full,
    Drift,
    Security,
    Docker,
    Performance,
    Accessibility,
    Database,
    ApiDesign,
    Custom,
}

impl AuditKind {
    /// Snake-case label persisted in `audit_runs.kind` and used for
    /// progress / SSE event names so the UI can filter by audit type.
    pub fn as_label(&self) -> &'static str {
        match self {
            AuditKind::Full          => "Full",
            AuditKind::Drift         => "Drift",
            AuditKind::Security      => "Security",
            AuditKind::Docker        => "Docker",
            AuditKind::Performance   => "Performance",
            AuditKind::Accessibility => "Accessibility",
            AuditKind::Database      => "Database",
            AuditKind::ApiDesign     => "ApiDesign",
            AuditKind::Custom        => "Custom",
        }
    }
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct LaunchAuditRequest {
    pub agent: AgentType,
    /// 0.8.2 — Specialized audit type. Omitted/null defaults to `Full`
    /// for backwards-compat (the only kind the UI knows about pre-0.8.2).
    #[serde(default)]
    pub kind: Option<AuditKind>,
    /// 0.8.2 — Caller-provided free-form prompt; only honored when
    /// `kind == AuditKind::Custom`. Ignored otherwise.
    #[serde(default)]
    pub custom_prompt: Option<String>,
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
