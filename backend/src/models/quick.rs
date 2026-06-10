// Quick Prompts (reusable prompt templates) + Quick APIs (reusable API
// call templates). Both ride on the same `{{variable}}` rendering engine
// the workflow steps use, so they live alongside the workflow types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{AgentType, ExtractSpec, ModelTier, PaginationSpec};

// ═══════════════════════════════════════════════════════════════════════════════
// Quick Prompts (reusable prompt templates with variables)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PromptVariable {
    pub name: String,
    pub label: String,
    pub placeholder: String,
    /// Optional human description of what this variable means. Shown in
    /// the batch-workflow UI so the user mapping tracker fields to QP
    /// variables knows what each one is for.
    #[serde(default)]
    pub description: Option<String>,
    /// Whether the variable must be filled before the QP can run.
    /// Defaults to `true` for backward compatibility — existing QP
    /// variables are treated as required.
    #[serde(default = "default_variable_required")]
    pub required: bool,
    /// 2026-06-10 — optional regex the provided value must match (anchored
    /// full-match). Lets a workflow declare a shape (`^[A-Z]+-\d+$` for a
    /// Jira key) so a typo like `7152` instead of `EW-7152` is rejected at
    /// launch with a clear message, BEFORE it reaches the API as a literal
    /// path param and 404s. `None` = no shape constraint (legacy). Invalid
    /// regex is treated as "no constraint" (never blocks a launch on a
    /// malformed pattern; logged).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
}

fn default_variable_required() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickPrompt {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub prompt_template: String,
    pub variables: Vec<PromptVariable>,
    pub agent: AgentType,
    pub project_id: Option<String>,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    /// 0.8.5 — optional profile binding (persona injection at launch).
    /// Mirrors `WorkflowStep.profile_ids` + `Discussion.profile_ids`. Empty
    /// vec = no profile bound (legacy behaviour).
    #[serde(default)]
    pub profile_ids: Vec<String>,
    /// 0.8.5 — optional directive binding (rules-of-conduct at launch).
    /// Mirrors `WorkflowStep.directive_ids` + `Discussion.directive_ids`.
    /// Empty vec = no directive bound (legacy behaviour).
    #[serde(default)]
    pub directive_ids: Vec<String>,
    #[serde(default)]
    pub tier: ModelTier,
    /// Optional human description of what this Quick Prompt does. Shown
    /// in the batch-workflow picker so the user knows which QP fits their
    /// use case. Empty string = legacy QP created before 2026-04-10.
    #[serde(default)]
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateQuickPromptRequest {
    pub name: String,
    pub icon: Option<String>,
    pub prompt_template: String,
    #[serde(default)]
    pub variables: Vec<PromptVariable>,
    pub agent: Option<AgentType>,
    pub project_id: Option<String>,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    #[serde(default)]
    pub profile_ids: Vec<String>,
    #[serde(default)]
    pub directive_ids: Vec<String>,
    #[serde(default)]
    pub tier: ModelTier,
    #[serde(default)]
    pub description: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Quick APIs (0.6.0) — reusable API call templates with {{variables}}.
// Same pattern as QuickPrompt but the engine is HTTP, not LLM. Field names
// follow `WorkflowStep` ApiCall fields verbatim so the frontend can reuse
// `ApiCallStepCard` (and therefore `ApiCallAiHelper`) as the editor.
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickApi {
    pub id: String,
    pub name: String,
    pub icon: String,
    /// Optional human description — shown in the BatchApiCall picker.
    #[serde(default)]
    pub description: String,
    pub project_id: Option<String>,

    // API request shape — same field names as WorkflowStep ApiCall fields.
    pub api_plugin_slug: String,
    pub api_config_id: String,
    pub api_endpoint_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_query: Option<std::collections::HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_path_params: Option<std::collections::HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_headers: Option<std::collections::HashMap<String, String>>,
    /// Same shape as `WorkflowStep.api_body`: a JSON `Value` rather than a
    /// raw string. The runtime engine walks the tree and interpolates
    /// string leaves only — no string-level templating that would let a
    /// `{{var}}` containing `","` punch through into JSON injection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_body: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_extract: Option<ExtractSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_pagination: Option<PaginationSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_max_retries: Option<u8>,

    /// Variables prompted at run-time (single-call) or whose names become
    /// the keys mapped from each batch item (batch-call).
    pub variables: Vec<PromptVariable>,

    /// 0.8.5 — optional profile binding. Picked up by any downstream
    /// agent surface that consumes this Quick API (e.g. the "Compare
    /// agents" QA helper). Empty vec = unbound. Pure API calls ignore
    /// this; it only matters when the QA result feeds into an LLM step.
    #[serde(default)]
    pub profile_ids: Vec<String>,
    /// 0.8.5 — optional directive binding. Same rationale as
    /// `profile_ids` above.
    #[serde(default)]
    pub directive_ids: Vec<String>,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateQuickApiRequest {
    pub name: String,
    pub icon: Option<String>,
    #[serde(default)]
    pub description: String,
    pub project_id: Option<String>,
    pub api_plugin_slug: String,
    pub api_config_id: String,
    pub api_endpoint_path: String,
    pub api_method: Option<String>,
    pub api_query: Option<std::collections::HashMap<String, String>>,
    pub api_path_params: Option<std::collections::HashMap<String, String>>,
    pub api_headers: Option<std::collections::HashMap<String, String>>,
    pub api_body: Option<serde_json::Value>,
    pub api_extract: Option<ExtractSpec>,
    pub api_pagination: Option<PaginationSpec>,
    pub api_timeout_ms: Option<u64>,
    pub api_max_retries: Option<u8>,
    #[serde(default)]
    pub variables: Vec<PromptVariable>,
    #[serde(default)]
    pub profile_ids: Vec<String>,
    #[serde(default)]
    pub directive_ids: Vec<String>,
}

// 0.8.5 — Quick Prompt version snapshot. Written by `db::quick_prompts`
// on every INSERT (v1) and UPDATE (v2, v3, …). Carries every editable
// field at the time of the change so the history drawer can render the
// timeline + the metrics aggregator can group launches by `(qp_id,
// version_index)` via the matching columns on `discussions`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickPromptVersion {
    pub id: String,
    pub quick_prompt_id: String,
    pub version_index: u32,
    pub name: String,
    pub icon: String,
    pub prompt_template: String,
    pub variables: Vec<PromptVariable>,
    pub agent: AgentType,
    pub project_id: Option<String>,
    pub skill_ids: Vec<String>,
    pub profile_ids: Vec<String>,
    pub directive_ids: Vec<String>,
    pub tier: ModelTier,
    pub description: String,
    pub created_at: DateTime<Utc>,
}

// 0.8.5 — Aggregated launch metrics for a single QP version. Returned
// by `GET /api/quick-prompts/:id/metrics` (one row per `version_index`
// that has at least one launch with `originating_qp_version` set).
// Only the FIRST agent reply of each discussion is counted — that's
// the message that reflects the QP's pertinence; follow-up turns are
// driven by the user's reactions, not the QP itself.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickPromptVersionMetrics {
    pub version_index: u32,
    /// Number of launched discussions whose first-agent-reply lands
    /// in this version's window. Pertinence Δs are only emitted when
    /// `launches >= 3` (the noise floor).
    pub launches: u32,
    pub avg_tokens: u64,
    /// Mean wall-clock duration of the first agent reply, milliseconds.
    /// `None` when no launch in this version has a captured
    /// `duration_ms` (legacy rows or imported transcripts).
    pub avg_duration_ms: Option<u64>,
    /// Mean USD cost of the first agent reply. `None` when no launch
    /// has cost data (e.g. local Ollama runs).
    pub avg_cost_usd: Option<f64>,
}

// Skills / Profiles / Directives extracted to `agents.rs` (TD-models-monolith).


// ─── Quick Prompts / APIs API requests ────────────────────────────────────

/// 0.7.0 UX pass — payload for `POST /api/quick-prompts/import`.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct ImportQuickPromptRequest {
    pub content: String,
    pub project_id: Option<String>,
}

/// Self-contained envelope produced by `GET /api/quick-prompts/:id/export`.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickPromptExportEnvelope {
    pub kind: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    /// Like the workflow envelope: `id`, `project_id`, `created_at`,
    /// `updated_at` are present on the wire but reset at import.
    pub quick_prompt: QuickPrompt,
}

/// 0.6.0 — payload for `POST /api/quick-apis/import`. Mirrors the QP shape.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct ImportQuickApiRequest {
    pub content: String,
    pub project_id: Option<String>,
}

/// Self-contained envelope produced by `GET /api/quick-apis/:id/export`.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickApiExportEnvelope {
    pub kind: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    /// `id`, `project_id`, `created_at`, `updated_at` are present on the
    /// wire but reset at import — fresh values are minted by the importer.
    pub quick_api: QuickApi,
}

/// 0.6.0 — payload for `POST /api/quick-apis/:id/run`. Lets the user
/// launch a saved QuickApi standalone (Run drawer in the Quick APIs page),
/// passing values for the declared `variables`.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct RunQuickApiRequest {
    /// Map of variable name → user-entered value. Keys must match
    /// `QuickApi.variables[*].name`. Missing keys for required variables
    /// get the call rejected before any HTTP fires.
    #[serde(default)]
    #[ts(type = "Record<string, string>")]
    pub variables: ::std::collections::HashMap<String, String>,
}

/// Response from `POST /api/quick-apis/:id/run`. Mirrors the
/// `/test-api-call` shape so the frontend can reuse the same UI.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct RunQuickApiResponse {
    pub success: bool,
    pub duration_ms: u64,
    /// Parsed envelope (data/status/summary) on success, `None` on failure.
    pub envelope: Option<serde_json::Value>,
    /// Error message on failure, `None` on success.
    pub error: Option<String>,
}

/// 0.6.0 — payload for `POST /api/quick-apis/:id/batch`. Fan-out the same
/// QA over a list of items (sub-domains, ticket keys, languages, etc.)
/// without needing a workflow. Mirror of the `BatchApiCall` step type
/// but standalone — uses the same parallel HTTP executor under the hood.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct BatchRunQuickApiRequest {
    /// Items to fan-out over. Accepts:
    ///   - JSON array of strings (each fills the QA's first variable):
    ///     `["www.example.com", "de.example.com", "fr.example.com"]`
    ///   - JSON array of objects (each key maps to a variable name):
    ///     `[{"host":"www.example.com","limit":"5"}, ...]`
    pub items: serde_json::Value,
    /// Max parallel HTTP calls (default 5, hard-capped at 20).
    #[serde(default)]
    pub concurrent_limit: Option<u32>,
}

/// Response from `POST /api/quick-apis/:id/batch`. The full aggregated
/// envelope produced by the BatchApiCall executor — the frontend renders
/// `envelope.data.items[]` as a per-item result table.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct BatchRunQuickApiResponse {
    /// Overall status: `OK` (all succeeded), `PARTIAL` (some failed), `ERROR` (all failed).
    pub status: String,
    pub duration_ms: u64,
    pub envelope: Option<serde_json::Value>,
    pub error: Option<String>,
}
