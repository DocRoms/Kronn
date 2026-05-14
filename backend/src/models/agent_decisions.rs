//! Agent decision log — one row per non-trivial choice the triage
//! agent surfaces in the Feasibility-Gated Implementation pattern.
//! See `workflows/triage.rs` for the manifest shape that populates
//! this table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One decision row. Mirrors the `agent_decisions` table 1:1 — see
/// migration `051_agent_decisions.sql`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct AgentDecision {
    pub id: String,
    pub run_id: String,
    pub step_name: String,
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_ref: Option<String>,
    /// `decided` | `mocked` | `blocked`. `clear` entries are NOT
    /// persisted (trivial by definition).
    pub category: String,
    pub decision_id: String,
    pub what: String,

    // ── decided ────────────────────────────────────────────
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen: Option<String>,
    /// JSON array of strings (each string = one rejected option).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options_json: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,

    // ── mocked ─────────────────────────────────────────────
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revisit_when: Option<String>,

    // ── blocked ────────────────────────────────────────────
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needed_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workaround: Option<String>,

    // ── lifecycle ──────────────────────────────────────────
    /// `pending` | `auto_approved` | `human_approved` | `overridden` | `resolved`.
    pub gate_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_value: Option<String>,
    /// JSON array of `"file:line"` strings, populated by the drift
    /// detector after the implement step runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_locations: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
}

/// Allowed values for `AgentDecision.category` and the corresponding
/// manifest bucket names.
pub const CATEGORY_DECIDED: &str = "decided";
pub const CATEGORY_MOCKED: &str = "mocked";
pub const CATEGORY_BLOCKED: &str = "blocked";

/// Allowed values for `AgentDecision.gate_status`.
pub const STATUS_PENDING: &str = "pending";
pub const STATUS_AUTO_APPROVED: &str = "auto_approved";
pub const STATUS_HUMAN_APPROVED: &str = "human_approved";
pub const STATUS_OVERRIDDEN: &str = "overridden";
pub const STATUS_RESOLVED: &str = "resolved";
