//! Model catalog contract (KT-531).
//!
//! Canonical identity is the pair `(runtime_target_id, model_id)` — never a display
//! label. Resolution order is fixed: live discovery (ACP or an official
//! machine-readable interface) > last valid live snapshot ("cached",
//! explicitly flagged as potentially stale) > operator-configured manual
//! entry > one-time migrated seed from the formerly hardcoded catalog. The UI
//! must never present `Cached` or `Migrated` as a current discovery.

use super::setup::{AgentType, ModelTier};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ModelProvenance {
    /// Confirmed by a live discovery call within this refresh cycle.
    Live,
    /// The last successful live snapshot, kept because the most recent
    /// refresh attempt failed or was not run again yet. Must always be
    /// labeled as potentially stale in the UI.
    Cached,
    /// Configured by the operator for a runtime with no reliable live
    /// catalogue. A functional fallback, never a mandatory mirror of live.
    Manual,
    /// Seeded once from the formerly hardcoded catalog to preserve existing
    /// configurations. Never rewritten by discovery; only reconciliation can
    /// promote the identity to `Live`.
    Migrated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ModelAvailability {
    Available,
    Unavailable,
}

/// Normalized reason a model is unavailable, or why a discovery attempt could
/// not confirm it. Shared verbatim across the catalog, preflight diagnostics
/// and audit history so the UI never has to parse a provider-specific string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ModelUnavailableReason {
    /// Absent from the most recent successful live catalogue for this
    /// runtime, though it was previously seen.
    Disappeared,
    /// Discovery could not authenticate against the runtime/provider.
    AuthRequired,
    /// The discovery call did not complete within its bound.
    Timeout,
    /// The runtime's CLI/binary is not installed or not runnable.
    CliMissing,
    /// The runtime answered but its catalogue payload could not be parsed.
    InvalidCatalog,
    /// The runtime/provider returned an error unrelated to auth or timeout.
    ProviderError,
    /// This runtime has no live discovery path implemented; only cache,
    /// manual or migrated entries can exist for it.
    Unsupported,
}

/// One model as Kronn's shared contract sees it. `id` is an opaque encoding
/// of `(runtime_target_id, model_id)` — stable across reconciliation, free of
/// delimiter ambiguity and never derived from `display_name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CatalogModelEntry {
    pub id: String,
    /// Durable execution target namespace. CLI families use
    /// `agent:<canonical-slug>`; named OpenAI-compatible connections use
    /// `http:<immutable-connection-id>`. Transport selection is intentionally
    /// not encoded here: direct CLI and ACP are routes to the same target.
    pub runtime_target_id: String,
    /// Projection metadata used by existing UI and runner code. It is not
    /// part of the catalog identity.
    pub agent_type: AgentType,
    /// The exact model identifier as the runtime/provider knows it — what a
    /// `--model` flag or API `model` field must receive.
    pub model_id: String,
    pub display_name: String,
    /// Operator-set label override. When present, selectors show this
    /// instead of `display_name`, even after the record is reconciled to
    /// `Live` (KT-531: operator display choices survive reconciliation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_alias: Option<String>,
    pub provenance: ModelProvenance,
    pub availability: ModelAvailability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<ModelUnavailableReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_detail: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub reasoning_modes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_mode: Option<String>,
    /// Operator-assigned Economy/Default/Reasoning tier, if any. Survives
    /// reconciliation the same way `display_alias` does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_assignment: Option<ModelTier>,
    /// True when this identity was ever created as a manual entry (even if
    /// its provenance has since been promoted to `Live` by reconciliation).
    /// Kept for the audit trail required by KT-531.
    #[serde(default)]
    pub manual_origin: bool,
    /// First time this identity was ever seen (any provenance).
    pub first_seen_at: DateTime<Utc>,
    /// Last time this identity was confirmed present by a live discovery.
    /// Manual and migrated rows remain `None` until a live reconciliation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<DateTime<Utc>>,
    /// Last time Kronn attempted to verify this identity, live or not.
    pub last_checked_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Payload for creating or updating a manual catalog entry. Identity
/// (`runtime_target_id` + `model_id`) is immutable once created — editing it would
/// silently orphan every reference that already resolved to the old
/// identity, so a caller who wants a different `model_id` must delete and
/// recreate the entry.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct UpsertManualModelRequest {
    pub runtime_target_id: String,
    pub agent_type: AgentType,
    pub model_id: String,
    pub display_name: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub reasoning_modes: Vec<String>,
    #[serde(default)]
    pub default_reasoning_mode: Option<String>,
    #[serde(default)]
    pub tier_assignment: Option<ModelTier>,
}

/// Response for one runtime target — the resolved list plus
/// the metadata a selector needs to render provenance honestly.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ModelCatalogView {
    pub runtime_target_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_label: Option<String>,
    pub agent_type: AgentType,
    pub models: Vec<CatalogModelEntry>,
    /// Whether the most recent refresh attempt for this runtime reached a
    /// live source successfully. `false` means every model below is at best
    /// `Cached`/`Manual`/`Migrated`.
    pub live_refresh_ok: bool,
    /// The live snapshot backing this view is older than the freshness
    /// window, or there has never been one. The UI must never present
    /// `Cached`/`Migrated` entries as a current discovery when this is true.
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_live_success_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_reason: Option<ModelUnavailableReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ModelCatalogSnapshot {
    pub targets: Vec<ModelCatalogView>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct RefreshModelCatalogRequest {
    pub runtime_target_id: String,
    pub agent_type: AgentType,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DeleteManualModelRequest {
    pub runtime_target_id: String,
    pub model_id: String,
}

/// Structured, catalog-driven preflight diagnostic. Shared by discussion
/// dispatch, Quick Prompt runs, comparisons and workflow steps so the UI
/// renders one consistent card regardless of the launch surface.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct CatalogPreflightFailure {
    pub runtime_target_id: String,
    pub agent_type: AgentType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    pub reason: ModelUnavailableReason,
    pub detail: String,
    pub last_checked_at: DateTime<Utc>,
    /// Machine-readable recommended next step (`"configure_manual_model"`,
    /// `"recheck_catalog"`, `"install_cli"`, `"authenticate"`). The frontend
    /// maps this to the recheck/settings shortcut; it is deliberately not a
    /// prose sentence so i18n stays centralized in the frontend dictionaries.
    pub recommended_action: String,
}
