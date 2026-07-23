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
    /// True when the project directory resolves on disk. Computed by
    /// `enrich_audit_status` (the list/get API layer), NOT persisted. The DB
    /// row mapper defaults it to `true` so a non-enriched read (e.g. the export
    /// payload) never falsely flags a project. Drives the "chemin introuvable —
    /// remap" banner + per-card badge after a cross-OS import (WSL ⇄ macOS),
    /// where absolute paths don't translate.
    #[serde(default = "default_true")]
    pub path_exists: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_skill_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub briefing_notes: Option<String>,
    /// 0.8.3 — Companion repos that an agent on this project should
    /// know about. A typical setup: a frontend project pointing at
    /// the backend API repo, the IaC repo, and the shared design
    /// system repo. The audit pipeline + every discussion / QP /
    /// workflow running on this project picks up this list in its
    /// system prompt prelude. Stored as in-row JSON (small data,
    /// projects rarely have more than 5 links).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked_repos: Vec<LinkedRepo>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_true() -> bool {
    true
}

/// A companion repository linked to a project. The `location` is
/// either a filesystem path (preferred — gives the agent direct
/// read access) or a URL (GitHub/GitLab — the agent still gets it
/// in context as a pointer to fetch on demand). The `kind` is just
/// a UI hint for the icon + grouping; it doesn't change runtime
/// behavior.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct LinkedRepo {
    pub id: String,
    pub name: String,
    /// Bucket for UI grouping + icon: `"api"` | `"iac"` | `"design"`
    /// | `"shared-lib"` | `"docs"` | `"other"`. Stored as String for
    /// forward-compat (new kinds don't break rows already in DB).
    pub kind: String,
    /// Filesystem path (`/home/user/repos/my-api`) OR URL
    /// (`https://github.com/org/my-api`). The agent decides what to
    /// do with it based on the format.
    pub location: String,
    /// One-line explanation of why this repo is linked. Shown to
    /// agents in the prompt prelude so they know when to consult
    /// each link (e.g. "GraphQL schema lives here" vs "frontend
    /// design tokens").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
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
/// Produced by the SSE streams (`full_audit`, `partial_audit`)
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
    /// `"installing"` during template install, `"auditing"` during the dynamic chain
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
    /// `"full"` for the chained audit, `"partial"` for drift-triggered
    /// sub-audits, `"full_audit"` for the end-to-end variant. Kept as a
    /// string so future audit kinds don't force a schema migration.
    pub kind: String,
    /// 0.8.3 — live chips state surfaced via the poll endpoint, NOT
    /// just via SSE. Solves the case where the SSE stream stalls or
    /// buffers (nginx, agent freeze, page re-mount): the frontend
    /// polls `/api/audit-status` every few seconds and re-seeds the
    /// chips from these fields. Optional so the JSON shape stays
    /// backwards-compatible with old clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens_so_far: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    /// 0.8.4 (#319 / B3) — running count of `tool_call` events the
    /// agent has fired DURING the current step. Reset on every
    /// `step_start`. Surfaced as a chip after the tool name (e.g.
    /// `🔧 Write (14)`) so the user has a "still alive" signal even
    /// when the token chip is frozen (heavy step writing many TD
    /// files without intermediate `Usage` blocks — the symptom that
    /// confused the user during the 8-min Step 8 of the Full audit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool_call_count: Option<u32>,
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
    /// `Running` while in flight; `Completed` / `Failed` / `Cancelled` /
    /// `Interrupted` once terminal. `Interrupted` (0.8.3 #311) means
    /// the SSE stream ended before the executed chain completed, without an
    /// explicit cancel — typically a rate-limit, claude crash, or
    /// network blip. The frontend treats `Interrupted` specifically:
    /// it shows a dynamic resume button for `last_completed_step + 1`
    /// instead of a fresh "Lancer".
    pub status: String,
    /// 0.8.3 (#311) — last successfully completed step (1-based,
    /// matches the executed step-chain indexing). 0 = no step done yet.
    /// A chained Full currently completes at 16. Set on every `step_done` where
    /// `validate_step_output` returns success=true. Drives
    /// the resume mechanism: on resume we start at `this + 1`.
    #[serde(default)]
    pub last_completed_step: u32,
    /// 076 — durable link to the validation discussion created in the SAME
    /// transaction as the Completed status. The validate endpoint trusts
    /// only this, never title/date heuristics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_discussion_id: Option<String>,
    /// 076 — structured per-step outcomes (requested/succeeded/unchanged)
    /// for partial runs; provenance for the drift oracle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_outcomes_json: Option<String>,
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
    /// completion-time cluster detector (Full audits only). Kept as String
    /// in the model to avoid forcing schema migrations on every
    /// recommendation-shape tweak.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendations_json: Option<String>,
}

/// 0.8.4 (#298) — Per-step metrics for the post-audit recap panel.
///
/// One row per step per `audit_runs` row. Inserted at `step_start`
/// (only the started-at + file_label fields are populated), finalized
/// at `step_done` (ended_at + duration_ms + tokens + cli_success), and
/// decorated by `step_warning` (#292) when the step's output doesn't
/// look right.
///
/// The frontend ProjectCard reads `GET /api/audit-runs/:run_id/steps`
/// for a collapsed "▾ Détails du dernier audit" panel; the table is
/// sortable by `duration_ms` and `step_tokens` so the user can spot
/// the heaviest step at a glance.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditRunStep {
    pub audit_run_id: String,
    pub step_index: u32,
    pub file_label: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cumulative_tokens: Option<u64>,
    /// `false` when the CLI exited non-zero OR `step_warning` fired.
    pub cli_success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_warning: Option<String>,
    /// Mirrors the `step_warning.repaired` field from #292.
    #[serde(default)]
    pub step_repaired_from_template: bool,
}

/// Recommendation emitted by the completion-time cluster detector. Lives in
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
mod audit_kind_label_tests {
    use super::AuditKind;

    #[test]
    fn from_label_is_the_exact_inverse_of_as_label() {
        // Resume-by-run-id recovers the kind from the persisted label, so
        // the round-trip must be lossless for every variant. A drift here
        // would resume the wrong pipeline.
        for k in [
            AuditKind::Full, AuditKind::Drift, AuditKind::Security,
            AuditKind::Docker, AuditKind::Performance, AuditKind::Accessibility,
            AuditKind::Rgaa, AuditKind::Database, AuditKind::ApiDesign,
            AuditKind::CodeQuality, AuditKind::Custom,
        ] {
            assert_eq!(AuditKind::from_label(k.as_label()), Some(k), "{k:?} round-trip");
        }
    }

    #[test]
    fn from_label_rejects_unknown_labels() {
        assert_eq!(AuditKind::from_label("Nonsense"), None);
        assert_eq!(AuditKind::from_label(""), None);
        assert_eq!(AuditKind::from_label("full"), None, "case-sensitive on purpose");
    }
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
/// `Full` exposes the canonical 9-step foundation; a launched Full audit
/// appends 7 focused dimensions for a 16-step chain. The other variants run
/// one focused dimension. They share the
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
    /// 0.8.4 (#287) — French RGAA 4.1 accessibility norm.
    /// Sub-case of `Accessibility` but checks against the stricter
    /// 106 RGAA criteria (vs WCAG 2.1 AA's 50). Mandatory for
    /// public-service French sites + companies > 250 employees.
    Rgaa,
    Database,
    ApiDesign,
    /// 0.8.13 — code quality & maintainability: templates (Twig/JSX/HTML),
    /// styles (CSS architecture), backend-language smells, perf/eco hygiene.
    /// Born from the DOCROMS_WEB dogfooding: a Full audit surfaces docs &
    /// infra debt but never code-quality findings.
    CodeQuality,
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
            AuditKind::Rgaa          => "Rgaa",
            AuditKind::Database      => "Database",
            AuditKind::ApiDesign     => "ApiDesign",
            AuditKind::CodeQuality   => "CodeQuality",
            AuditKind::Custom        => "Custom",
        }
    }

    /// Inverse of [`as_label`]. Used to recover the kind of a persisted
    /// `audit_runs` row when resuming by `resume_run_id`. Unknown labels
    /// return `None` (never silently fall back to `Full`, which would run
    /// the wrong pipeline on a resume).
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "Full"          => Some(AuditKind::Full),
            "Drift"         => Some(AuditKind::Drift),
            "Security"      => Some(AuditKind::Security),
            "Docker"        => Some(AuditKind::Docker),
            "Performance"   => Some(AuditKind::Performance),
            "Accessibility" => Some(AuditKind::Accessibility),
            "Rgaa"          => Some(AuditKind::Rgaa),
            "Database"      => Some(AuditKind::Database),
            "ApiDesign"     => Some(AuditKind::ApiDesign),
            "CodeQuality"   => Some(AuditKind::CodeQuality),
            "Custom"        => Some(AuditKind::Custom),
            _               => None,
        }
    }

    /// 0.8.4 (#322 / F2) — user-facing display name for the kind,
    /// used in disc titles + UI badges. Different from `as_label()`
    /// (which is the canonical wire / DB token, kept TitleCase for
    /// serde round-trip stability). `display_name` is what a human
    /// expects to read: "RGAA 4.1" not "Rgaa", "Sécurité" not
    /// "Security" (FR — the user-facing locale of Kronn). When the
    /// app ships in EN/ES the frontend can still translate from the
    /// `as_label()` token + an i18n key; this helper only matters
    /// for backend-emitted strings (disc title, log lines).
    pub fn display_name(&self) -> &'static str {
        match self {
            AuditKind::Full          => "Audit global",
            AuditKind::Drift         => "Drift",
            AuditKind::Security      => "Sécurité",
            AuditKind::Docker        => "Docker",
            AuditKind::Performance   => "Performance",
            AuditKind::Accessibility => "Accessibilité",
            AuditKind::Rgaa          => "RGAA 4.1",
            AuditKind::Database      => "Base de données",
            AuditKind::ApiDesign     => "Design d'API",
            AuditKind::CodeQuality   => "Qualité de code",
            AuditKind::Custom        => "Custom",
        }
    }

    /// 0.8.4 (#287) — true for kinds that spawn a validation discussion
    /// after a successful run. Full and the 7 sub-audits all qualify;
    /// `Drift` and `Custom` don't (Drift is checksum-only, Custom is
    /// caller-defined and shouldn't get a Kronn-shaped validation flow).
    pub fn is_validatable(&self) -> bool {
        matches!(
            self,
            AuditKind::Full
                | AuditKind::Security
                | AuditKind::Docker
                | AuditKind::Performance
                | AuditKind::Accessibility
                | AuditKind::Rgaa
                | AuditKind::Database
                | AuditKind::ApiDesign
                | AuditKind::CodeQuality
        )
    }

    /// 0.8.4 (#287) — true for everything except `Full` and the
    /// non-validatable kinds. Drives the validation-prompt selector:
    /// sub-audits use `build_sub_audit_validation_prompt`, Full uses
    /// `build_validation_prompt` (the full 4-phase protocol).
    pub fn is_sub_audit(&self) -> bool {
        self.is_validatable() && !matches!(self, AuditKind::Full)
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
    /// Resume an interrupted run. The server loads this `audit_runs` row,
    /// verifies it belongs to the project and is `Interrupted`, then derives
    /// BOTH the kind and the checkpoint (`last_completed_step`) from the row
    /// — `kind`/`custom_prompt` above are ignored when this is set. This
    /// makes resume impossible to misuse: no client-supplied step count to
    /// oversize, and no way to graft a checkpoint onto the wrong pipeline.
    #[serde(default)]
    pub resume_run_id: Option<String>,
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

/// Body for `POST /api/projects/:id/clone-and-remap` — re-clone a project's
/// `repo_url` locally and re-point the existing project at the clone. Used to
/// recover projects whose path no longer resolves after a cross-machine DB
/// import (e.g. WSL `/home/...` paths on macOS).
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CloneAndRemapRequest {
    /// Optional parent directory to clone into. When omitted the server picks
    /// a sensible existing location (common parent of on-disk projects →
    /// `KRONN_REPOS_DIR` → first existing scan path).
    #[serde(default)]
    pub parent_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct CloneAndRemapResponse {
    pub project_id: String,
    /// The local path the project now points at (where the repo was cloned).
    pub new_path: String,
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

/// 0.8.7 — per-source failure surfaced to the user (GitLab silently
/// returning 0 repos because the token expired was the trigger ; the
/// front-end now renders a chip with the error so the user knows WHY
/// a source produced no results, instead of guessing the integration
/// is broken).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscoverSourceError {
    pub source_id: String,
    pub source_label: String,
    pub provider: String, // "github" | "gitlab"
    pub message: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscoverReposResponse {
    pub repos: Vec<RemoteRepo>,
    pub sources: Vec<String>,
    pub available_sources: Vec<RepoSource>,
    #[serde(default)]
    pub errors: Vec<DiscoverSourceError>,
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
