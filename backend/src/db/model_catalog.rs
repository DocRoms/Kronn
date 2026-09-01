//! Persistence for the KT-531 model catalog contract.
//!
//! Canonical identity is `(runtime_target_id, model_id)`. `reconcile_live` is the
//! only writer that can promote a row to `Live`/`Cached`, and it is written to
//! be idempotent and safely replayable after a restart (KT-531 reconciliation
//! requirement): reconciling the same live snapshot twice produces the same
//! rows, never duplicates.

use anyhow::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::parse_dt;
use crate::models::{
    AgentType, CatalogModelEntry, ModelAvailability, ModelCostHint, ModelProvenance, ModelTier,
    ModelUnavailableReason, UpsertManualModelRequest,
};

const COLUMNS: &str =
    "id, runtime_target_id, agent_type, model_id, display_name, display_alias, provenance, \
    availability, unavailable_reason, unavailable_detail, capabilities_json, \
    reasoning_modes_json, default_reasoning_mode, tier_assignment, cost_hint, privacy_note, \
    manual_origin, first_seen_at, last_seen_at, last_checked_at, created_at, updated_at";

pub fn canonical_id(runtime_target_id: &str, model_id: &str) -> String {
    let payload = format!("{runtime_target_id}\0{model_id}");
    format!("mc_{}", URL_SAFE_NO_PAD.encode(payload))
}

pub fn agent_runtime_target_id(agent_type: &AgentType) -> String {
    let slug = match agent_type {
        AgentType::ClaudeCode => "claude-code",
        AgentType::Codex => "codex",
        AgentType::OpenCode => "opencode",
        AgentType::Vibe => "vibe",
        AgentType::GeminiCli => "gemini-cli",
        AgentType::Kiro => "kiro",
        AgentType::CopilotCli => "copilot-cli",
        AgentType::Ollama => "ollama",
        AgentType::LiteLlm => "litellm",
        AgentType::Nvidia => "nvidia",
        AgentType::Custom => "custom",
    };
    format!("agent:{slug}")
}

pub fn http_runtime_target_id(connection_id: &str) -> String {
    format!("http:{connection_id}")
}

pub fn validate_runtime_target_id(runtime_target_id: &str) -> Result<()> {
    let suffix = runtime_target_id
        .strip_prefix("agent:")
        .or_else(|| runtime_target_id.strip_prefix("http:"));
    if suffix.is_none_or(|value| {
        value.is_empty()
            || value.len() > 128
            || value.chars().any(|character| {
                !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
            })
    }) {
        anyhow::bail!("runtime_target_id must use agent:<slug> or http:<connection-id>");
    }
    Ok(())
}

fn validate_runtime_target_projection(
    runtime_target_id: &str,
    agent_type: &AgentType,
) -> Result<()> {
    validate_runtime_target_id(runtime_target_id)?;
    if runtime_target_id.starts_with("agent:")
        && runtime_target_id != agent_runtime_target_id(agent_type)
    {
        anyhow::bail!(
            "runtime target {runtime_target_id} does not project to {}",
            format_agent_type(agent_type)
        );
    }
    Ok(())
}

fn validate_manual_fields(req: &UpsertManualModelRequest) -> Result<()> {
    validate_runtime_target_projection(&req.runtime_target_id, &req.agent_type)?;
    for (name, value) in [
        ("model_id", req.model_id.as_str()),
        ("display_name", req.display_name.as_str()),
    ] {
        if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            anyhow::bail!("{name} must contain 1 to 256 printable characters");
        }
    }
    if req.capabilities.len() > 16 || req.reasoning_modes.len() > 16 {
        anyhow::bail!("capabilities and reasoning modes are limited to 16 values");
    }
    for value in req.capabilities.iter().chain(&req.reasoning_modes) {
        if value.trim().is_empty() || value.len() > 64 || value.chars().any(char::is_control) {
            anyhow::bail!(
                "capability and reasoning mode values must be printable and at most 64 characters"
            );
        }
    }
    if req.default_reasoning_mode.as_ref().is_some_and(|value| {
        value.trim().is_empty() || value.len() > 64 || value.chars().any(char::is_control)
    }) {
        anyhow::bail!("default_reasoning_mode must be printable and at most 64 characters");
    }
    Ok(())
}

pub(crate) fn format_agent_type(agent_type: &AgentType) -> &'static str {
    match agent_type {
        AgentType::ClaudeCode => "ClaudeCode",
        AgentType::Codex => "Codex",
        AgentType::OpenCode => "OpenCode",
        AgentType::Vibe => "Vibe",
        AgentType::GeminiCli => "GeminiCli",
        AgentType::Kiro => "Kiro",
        AgentType::CopilotCli => "CopilotCli",
        AgentType::Ollama => "Ollama",
        AgentType::LiteLlm => "LiteLlm",
        AgentType::Nvidia => "Nvidia",
        AgentType::Custom => "Custom",
    }
}

pub(crate) fn parse_agent_type(s: &str) -> AgentType {
    match s {
        "ClaudeCode" => AgentType::ClaudeCode,
        "Codex" => AgentType::Codex,
        "OpenCode" => AgentType::OpenCode,
        "Vibe" => AgentType::Vibe,
        "GeminiCli" => AgentType::GeminiCli,
        "Kiro" => AgentType::Kiro,
        "CopilotCli" => AgentType::CopilotCli,
        "Ollama" => AgentType::Ollama,
        "LiteLlm" => AgentType::LiteLlm,
        "Nvidia" => AgentType::Nvidia,
        _ => AgentType::Custom,
    }
}

fn parse_provenance(s: &str) -> ModelProvenance {
    match s {
        "live" => ModelProvenance::Live,
        "cached" => ModelProvenance::Cached,
        "manual" => ModelProvenance::Manual,
        _ => ModelProvenance::Migrated,
    }
}

/// Stable ordering used to pick the best-provenance entry among candidates
/// for the same identity or the same tier assignment — lower sorts first.
pub fn provenance_rank(p: ModelProvenance) -> u8 {
    match p {
        ModelProvenance::Live => 0,
        ModelProvenance::Cached => 1,
        ModelProvenance::Manual => 2,
        ModelProvenance::Migrated => 3,
    }
}

fn format_reason(r: ModelUnavailableReason) -> &'static str {
    match r {
        ModelUnavailableReason::Disappeared => "disappeared",
        ModelUnavailableReason::AuthRequired => "auth_required",
        ModelUnavailableReason::Timeout => "timeout",
        ModelUnavailableReason::CliMissing => "cli_missing",
        ModelUnavailableReason::InvalidCatalog => "invalid_catalog",
        ModelUnavailableReason::ProviderError => "provider_error",
        ModelUnavailableReason::Unsupported => "unsupported",
    }
}

fn parse_reason(s: &str) -> ModelUnavailableReason {
    match s {
        "disappeared" => ModelUnavailableReason::Disappeared,
        "auth_required" => ModelUnavailableReason::AuthRequired,
        "timeout" => ModelUnavailableReason::Timeout,
        "cli_missing" => ModelUnavailableReason::CliMissing,
        "invalid_catalog" => ModelUnavailableReason::InvalidCatalog,
        "unsupported" => ModelUnavailableReason::Unsupported,
        _ => ModelUnavailableReason::ProviderError,
    }
}

fn format_tier(t: ModelTier) -> &'static str {
    match t {
        ModelTier::Economy => "economy",
        ModelTier::Default => "default",
        ModelTier::Reasoning => "reasoning",
    }
}

fn parse_tier(s: &str) -> Option<ModelTier> {
    match s {
        "economy" => Some(ModelTier::Economy),
        "default" => Some(ModelTier::Default),
        "reasoning" => Some(ModelTier::Reasoning),
        _ => None,
    }
}

fn format_cost_hint(c: ModelCostHint) -> &'static str {
    match c {
        ModelCostHint::Free => "free",
        ModelCostHint::Paid => "paid",
        ModelCostHint::Unknown => "unknown",
    }
}

fn parse_cost_hint(s: &str) -> Option<ModelCostHint> {
    match s {
        "free" => Some(ModelCostHint::Free),
        "paid" => Some(ModelCostHint::Paid),
        "unknown" => Some(ModelCostHint::Unknown),
        _ => None,
    }
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogModelEntry> {
    let capabilities_json: String = row.get(10)?;
    let reasoning_modes_json: String = row.get(11)?;
    Ok(CatalogModelEntry {
        id: row.get(0)?,
        runtime_target_id: row.get(1)?,
        agent_type: parse_agent_type(&row.get::<_, String>(2)?),
        model_id: row.get(3)?,
        display_name: row.get(4)?,
        display_alias: row.get(5)?,
        provenance: parse_provenance(&row.get::<_, String>(6)?),
        availability: if row.get::<_, String>(7)? == "available" {
            ModelAvailability::Available
        } else {
            ModelAvailability::Unavailable
        },
        unavailable_reason: row
            .get::<_, Option<String>>(8)?
            .as_deref()
            .map(parse_reason),
        unavailable_detail: row.get(9)?,
        capabilities: serde_json::from_str(&capabilities_json).unwrap_or_default(),
        reasoning_modes: serde_json::from_str(&reasoning_modes_json).unwrap_or_default(),
        default_reasoning_mode: row.get(12)?,
        tier_assignment: row
            .get::<_, Option<String>>(13)?
            .as_deref()
            .and_then(parse_tier),
        cost_hint: row
            .get::<_, Option<String>>(14)?
            .as_deref()
            .and_then(parse_cost_hint),
        privacy_note: row.get(15)?,
        manual_origin: row.get::<_, i64>(16)? != 0,
        first_seen_at: parse_dt(row.get(17)?),
        last_seen_at: row.get::<_, Option<String>>(18)?.map(parse_dt),
        last_checked_at: parse_dt(row.get(19)?),
        created_at: parse_dt(row.get(20)?),
        updated_at: parse_dt(row.get(21)?),
    })
}

pub fn list_all(conn: &Connection) -> Result<Vec<CatalogModelEntry>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM model_catalog_entries ORDER BY runtime_target_id, model_id"
    ))?;
    let rows = stmt
        .query_map([], row_to_entry)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn list_for_agent(conn: &Connection, agent_type: &AgentType) -> Result<Vec<CatalogModelEntry>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM model_catalog_entries WHERE agent_type = ?1 ORDER BY model_id"
    ))?;
    let rows = stmt
        .query_map(params![format_agent_type(agent_type)], row_to_entry)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn list_for_target(
    conn: &Connection,
    runtime_target_id: &str,
) -> Result<Vec<CatalogModelEntry>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM model_catalog_entries WHERE runtime_target_id = ?1 ORDER BY model_id"
    ))?;
    let rows = stmt
        .query_map(params![runtime_target_id], row_to_entry)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get(
    conn: &Connection,
    runtime_target_id: &str,
    model_id: &str,
) -> Result<Option<CatalogModelEntry>> {
    let id = canonical_id(runtime_target_id, model_id);
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM model_catalog_entries WHERE id = ?1"),
            params![id],
            row_to_entry,
        )
        .optional()?)
}

/// Create a brand-new manual entry. Fails (returns `Ok(None)`) when the
/// identity already exists — callers use `update_manual` for that case, so a
/// manual entry never silently clobbers a reconciled live one.
pub fn create_manual(
    conn: &Connection,
    req: &UpsertManualModelRequest,
) -> Result<Option<CatalogModelEntry>> {
    validate_manual_fields(req)?;
    if get(conn, &req.runtime_target_id, &req.model_id)?.is_some() {
        return Ok(None);
    }
    let id = canonical_id(&req.runtime_target_id, &req.model_id);
    let now = Utc::now().to_rfc3339();
    conn.execute(
        &format!(
            "INSERT INTO model_catalog_entries ({COLUMNS}) VALUES \
             (?1,?2,?3,?4,?5,NULL,'manual','available',NULL,NULL,?6,?7,?8,?9,?11,?12,1,?10,NULL,?10,?10,?10)"
        ),
        params![
            id,
            req.runtime_target_id,
            format_agent_type(&req.agent_type),
            req.model_id,
            req.display_name,
            serde_json::to_string(&req.capabilities)?,
            serde_json::to_string(&req.reasoning_modes)?,
            req.default_reasoning_mode,
            req.tier_assignment.map(format_tier),
            now,
            req.cost_hint.map(format_cost_hint),
            req.privacy_note,
        ],
    )?;
    if req.tier_assignment.is_some() {
        clear_other_tier_holders(conn, &req.runtime_target_id, req.tier_assignment, &id)?;
    }
    get(conn, &req.runtime_target_id, &req.model_id)
}

/// Update the operator-owned fields of an existing entry. `display_name`,
/// `capabilities` and `reasoning_modes` are only rewritten for `Manual` and
/// `Migrated` rows (the operator owns the record); `display_alias`,
/// `tier_assignment`, `cost_hint` and `privacy_note` are overlays that apply
/// regardless of provenance, so they survive — and can be set on — a
/// `Live`/`Cached` reconciled entry. `cost_hint`/`privacy_note` use
/// COALESCE, not replace-with-null like `tier_assignment`: a caller that
/// doesn't send them (`None`) must not silently erase a value reconciliation
/// or a previous operator edit already set.
pub fn update_manual(
    conn: &Connection,
    runtime_target_id: &str,
    model_id: &str,
    req: &UpsertManualModelRequest,
) -> Result<Option<CatalogModelEntry>> {
    validate_manual_fields(req)?;
    if req.runtime_target_id != runtime_target_id || req.model_id != model_id {
        anyhow::bail!("runtime_target_id and model_id are immutable");
    }
    let Some(existing) = get(conn, runtime_target_id, model_id)? else {
        return Ok(None);
    };
    if existing.agent_type != req.agent_type {
        anyhow::bail!("agent_type must match the target projection");
    }
    let now = Utc::now().to_rfc3339();
    let owns_record = matches!(
        existing.provenance,
        ModelProvenance::Manual | ModelProvenance::Migrated
    );
    if owns_record {
        conn.execute(
            "UPDATE model_catalog_entries SET display_name = ?1, capabilities_json = ?2, \
             reasoning_modes_json = ?3, default_reasoning_mode = ?4, tier_assignment = ?5, \
             cost_hint = COALESCE(?6, cost_hint), privacy_note = COALESCE(?7, privacy_note), \
             updated_at = ?8 WHERE id = ?9",
            params![
                req.display_name,
                serde_json::to_string(&req.capabilities)?,
                serde_json::to_string(&req.reasoning_modes)?,
                req.default_reasoning_mode,
                req.tier_assignment.map(format_tier),
                req.cost_hint.map(format_cost_hint),
                req.privacy_note,
                now,
                existing.id,
            ],
        )?;
    } else {
        conn.execute(
            "UPDATE model_catalog_entries SET display_alias = ?1, tier_assignment = ?2, \
             cost_hint = COALESCE(?3, cost_hint), privacy_note = COALESCE(?4, privacy_note), \
             updated_at = ?5 WHERE id = ?6",
            params![
                Some(req.display_name.clone()),
                req.tier_assignment.map(format_tier),
                req.cost_hint.map(format_cost_hint),
                req.privacy_note,
                now,
                existing.id,
            ],
        )?;
    }
    if req.tier_assignment.is_some() {
        clear_other_tier_holders(conn, runtime_target_id, req.tier_assignment, &existing.id)?;
    }
    get(conn, runtime_target_id, model_id)
}

/// Remove a manual/migrated entry outright. A `Live`/`Cached` entry cannot be
/// deleted this way (KT-531: disappearance is never a client-driven delete) —
/// use `mark_missing` to record its absence from the latest live catalogue.
pub fn delete_manual(conn: &Connection, runtime_target_id: &str, model_id: &str) -> Result<bool> {
    let Some(existing) = get(conn, runtime_target_id, model_id)? else {
        return Ok(false);
    };
    if !matches!(
        existing.provenance,
        ModelProvenance::Manual | ModelProvenance::Migrated
    ) {
        anyhow::bail!("cannot delete a live-sourced catalog entry; it can only become unavailable");
    }
    let affected = conn.execute(
        "DELETE FROM model_catalog_entries WHERE id = ?1",
        params![existing.id],
    )?;
    Ok(affected > 0)
}

/// Ensure at most one entry per `(agent_type, tier)` claims the assignment.
fn clear_other_tier_holders(
    conn: &Connection,
    runtime_target_id: &str,
    tier: Option<ModelTier>,
    keep_id: &str,
) -> Result<()> {
    let Some(tier) = tier else { return Ok(()) };
    conn.execute(
        "UPDATE model_catalog_entries SET tier_assignment = NULL, updated_at = ?1 \
         WHERE runtime_target_id = ?2 AND tier_assignment = ?3 AND id != ?4",
        params![
            Utc::now().to_rfc3339(),
            runtime_target_id,
            format_tier(tier),
            keep_id,
        ],
    )?;
    Ok(())
}

/// The best-provenance, available entry currently assigned to `tier` for
/// `agent_type`, if any. Pure so `resolve_model_flag` stays unit-testable.
pub fn resolve_tier_entry(
    entries: &[CatalogModelEntry],
    tier: ModelTier,
) -> Option<&CatalogModelEntry> {
    entries
        .iter()
        .filter(|e| {
            e.tier_assignment == Some(tier) && e.availability == ModelAvailability::Available
        })
        .min_by(|a, b| {
            provenance_rank(a.provenance)
                .cmp(&provenance_rank(b.provenance))
                .then(b.updated_at.cmp(&a.updated_at))
        })
}

/// One discovered model, as reported by a discovery adapter before it is
/// merged with any existing manual/migrated record for the same identity.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredModel {
    pub model_id: String,
    pub display_name: String,
    pub capabilities: Vec<String>,
    pub reasoning_modes: Vec<String>,
    pub default_reasoning_mode: Option<String>,
}

/// Structural, non-enumerated detection of an OpenCode Zen-routed model.
/// OpenCode's own docs qualify every Zen model id as `opencode/<model-id>`
/// inside its multi-provider config
/// [src: url: https://opencode.ai/docs/zen/]. Kronn has no live pricing
/// signal for these: ACP's `session/new` config options carry only
/// id/name (no cost), and Zen's own `/v1/models` endpoint returns only
/// `id`/`object`/`created`/`owned_by` — verified empty of pricing fields.
/// `cost_hint` therefore starts `Unknown`, never a guessed `Paid`/`Free`; the
/// accompanying note is a structural fact about the third-party gateway, not
/// per-model text. An operator with current pricing info can still override
/// both via the manual catalog entry (`update_manual`'s overlay path).
fn derive_opencode_zen_overlay(
    runtime_target_id: &str,
    model_id: &str,
) -> (Option<ModelCostHint>, Option<String>) {
    let is_zen_routed = runtime_target_id == agent_runtime_target_id(&AgentType::OpenCode)
        && model_id
            .split_once('/')
            .is_some_and(|(provider, _)| provider == "opencode");
    if !is_zen_routed {
        return (None, None);
    }
    (
        Some(ModelCostHint::Unknown),
        Some(
            "Routed through OpenCode Zen, a third-party model gateway. Pricing (including any \
             temporarily free models) and data-handling terms can change at any time — verify \
             current terms at opencode.ai/zen before sending sensitive data."
                .to_string(),
        ),
    )
}

/// Reconcile one runtime's live discovery result into the catalog. Idempotent
/// and safely replayable: reconciling an identical `discovered` set twice
/// leaves the same rows with only `last_checked_at`/`last_seen_at` advancing.
///
/// - New identities are inserted as `Live`; a first-seen OpenCode Zen model
///   (KT-543: see `derive_opencode_zen_overlay`) gets an honest `Unknown`
///   cost hint and a structural privacy note, never a guessed one.
/// - An identity that already exists (any provenance) is promoted to `Live`;
///   its `display_alias`, `tier_assignment`, `cost_hint`, `privacy_note` and
///   `manual_origin` survive untouched (KT-531: operator choices — and any
///   value already assigned — outrank live metadata).
/// - An identity previously `Live`/`Cached` that is absent from `discovered`
///   is downgraded to `Cached` (not deleted) so the last known catalogue
///   stays visible while clearly flagged as potentially stale.
pub fn reconcile_live(
    conn: &Connection,
    runtime_target_id: &str,
    agent_type: &AgentType,
    discovered: &[DiscoveredModel],
) -> Result<()> {
    validate_runtime_target_projection(runtime_target_id, agent_type)?;
    let now = Utc::now().to_rfc3339();
    let existing = list_for_target(conn, runtime_target_id)?;
    let discovered_ids: std::collections::HashSet<&str> =
        discovered.iter().map(|d| d.model_id.as_str()).collect();

    for model in discovered {
        let id = canonical_id(runtime_target_id, &model.model_id);
        match existing.iter().find(|e| e.model_id == model.model_id) {
            Some(_) => {
                conn.execute(
                    "UPDATE model_catalog_entries SET display_name = ?1, provenance = 'live', \
                     availability = 'available', unavailable_reason = NULL, \
                     unavailable_detail = NULL, capabilities_json = ?2, \
                     reasoning_modes_json = ?3, default_reasoning_mode = ?4, \
                     last_seen_at = ?5, last_checked_at = ?5, updated_at = ?5 WHERE id = ?6",
                    params![
                        model.display_name,
                        serde_json::to_string(&model.capabilities)?,
                        serde_json::to_string(&model.reasoning_modes)?,
                        model.default_reasoning_mode,
                        now,
                        id,
                    ],
                )?;
            }
            None => {
                let (cost_hint, privacy_note) =
                    derive_opencode_zen_overlay(runtime_target_id, &model.model_id);
                conn.execute(
                    &format!(
                        "INSERT INTO model_catalog_entries ({COLUMNS}) VALUES \
                         (?1,?2,?3,?4,?5,NULL,'live','available',NULL,NULL,?6,?7,?8,NULL,?10,?11,0,?9,?9,?9,?9,?9)"
                    ),
                    params![
                        id,
                        runtime_target_id,
                        format_agent_type(agent_type),
                        model.model_id,
                        model.display_name,
                        serde_json::to_string(&model.capabilities)?,
                        serde_json::to_string(&model.reasoning_modes)?,
                        model.default_reasoning_mode,
                        now,
                        cost_hint.map(format_cost_hint),
                        privacy_note,
                    ],
                )?;
            }
        }
    }

    // Anything previously live-sourced but missing from this snapshot becomes
    // `Cached` and unavailable — a stale-but-visible last-known state, never
    // deleted or silently selectable.
    for entry in &existing {
        if discovered_ids.contains(entry.model_id.as_str())
            || !matches!(
                entry.provenance,
                ModelProvenance::Live | ModelProvenance::Cached
            )
        {
            continue;
        }
        conn.execute(
            "UPDATE model_catalog_entries SET provenance = 'cached', availability = 'unavailable', \
             unavailable_reason = 'disappeared', unavailable_detail = ?1, last_checked_at = ?2, \
             updated_at = ?2 WHERE id = ?3",
            params!["absent from the latest successful live catalog", now, entry.id],
        )?;
    }

    set_refresh_log(conn, runtime_target_id, agent_type, true, None, None)
}

/// Record a failed discovery attempt without touching any model row: the
/// last known catalogue (`Cached`/`Manual`/`Migrated`) remains exactly as it
/// was, and the failure is only visible via the refresh log + preflight.
pub fn record_refresh_failure(
    conn: &Connection,
    runtime_target_id: &str,
    agent_type: &AgentType,
    reason: ModelUnavailableReason,
    detail: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE model_catalog_entries SET provenance = 'cached', last_checked_at = ?1, \
         updated_at = ?1 WHERE runtime_target_id = ?2 AND provenance = 'live'",
        params![now, runtime_target_id],
    )?;
    set_refresh_log(
        conn,
        runtime_target_id,
        agent_type,
        false,
        Some(reason),
        Some(detail),
    )
}

/// A model absent from a runtime's live catalogue is never deleted. It is
/// marked `unavailable` with a normalized reason and keeps `last_seen_at` from
/// the last time it was actually observed live, so the UI can show "last
/// seen" honestly. Reappearing under the same identity clears this state.
pub fn mark_unavailable(
    conn: &Connection,
    runtime_target_id: &str,
    model_id: &str,
    reason: ModelUnavailableReason,
    detail: Option<&str>,
) -> Result<bool> {
    let id = canonical_id(runtime_target_id, model_id);
    let now = Utc::now().to_rfc3339();
    let affected = conn.execute(
        "UPDATE model_catalog_entries SET availability = 'unavailable', unavailable_reason = ?1, \
         unavailable_detail = ?2, last_checked_at = ?3, updated_at = ?3 WHERE id = ?4",
        params![format_reason(reason), detail, now, id],
    )?;
    Ok(affected > 0)
}

pub fn mark_available(conn: &Connection, runtime_target_id: &str, model_id: &str) -> Result<bool> {
    let id = canonical_id(runtime_target_id, model_id);
    let now = Utc::now().to_rfc3339();
    let affected = conn.execute(
        "UPDATE model_catalog_entries SET availability = 'available', unavailable_reason = NULL, \
         unavailable_detail = NULL, last_seen_at = ?1, last_checked_at = ?1, updated_at = ?1 \
         WHERE id = ?2",
        params![now, id],
    )?;
    Ok(affected > 0)
}

#[derive(Debug, Clone)]
pub struct RefreshLog {
    pub runtime_target_id: String,
    pub agent_type: AgentType,
    pub last_live_success_at: Option<DateTime<Utc>>,
    pub last_attempt_at: DateTime<Utc>,
    pub last_error_reason: Option<ModelUnavailableReason>,
    pub last_error_detail: Option<String>,
}

fn set_refresh_log(
    conn: &Connection,
    runtime_target_id: &str,
    agent_type: &AgentType,
    success: bool,
    error_reason: Option<ModelUnavailableReason>,
    error_detail: Option<&str>,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let existing_success =
        get_refresh_log(conn, runtime_target_id)?.and_then(|l| l.last_live_success_at);
    let last_live_success_at = if success {
        Some(now.clone())
    } else {
        existing_success.map(|dt| dt.to_rfc3339())
    };
    conn.execute(
        "INSERT INTO model_catalog_refresh_log \
         (runtime_target_id, agent_type, last_live_success_at, last_attempt_at, last_error_reason, last_error_detail) \
         VALUES (?1,?2,?3,?4,?5,?6) \
         ON CONFLICT(runtime_target_id) DO UPDATE SET agent_type = excluded.agent_type, \
         last_live_success_at = excluded.last_live_success_at, \
         last_attempt_at = excluded.last_attempt_at, last_error_reason = excluded.last_error_reason, \
         last_error_detail = excluded.last_error_detail",
        params![
            runtime_target_id,
            format_agent_type(agent_type),
            last_live_success_at,
            now,
            error_reason.map(format_reason),
            if success { None } else { error_detail },
        ],
    )?;
    Ok(())
}

pub fn get_refresh_log(conn: &Connection, runtime_target_id: &str) -> Result<Option<RefreshLog>> {
    Ok(conn
        .query_row(
            "SELECT runtime_target_id, agent_type, last_live_success_at, last_attempt_at, last_error_reason, \
             last_error_detail FROM model_catalog_refresh_log WHERE runtime_target_id = ?1",
            params![runtime_target_id],
            |row| {
                Ok(RefreshLog {
                    runtime_target_id: row.get(0)?,
                    agent_type: parse_agent_type(&row.get::<_, String>(1)?),
                    last_live_success_at: row
                        .get::<_, Option<String>>(2)?
                        .map(parse_dt),
                    last_attempt_at: parse_dt(row.get(3)?),
                    last_error_reason: row
                        .get::<_, Option<String>>(4)?
                        .as_deref()
                        .map(parse_reason),
                    last_error_detail: row.get(5)?,
                })
            },
        )
        .optional()?)
}

/// `true` once the one-time hardcoded-catalog migration has run.
pub fn migration_already_ran(conn: &Connection) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM model_catalog_migration_state WHERE id = 1",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Mark the one-time migration as done. Idempotent: a second call is a no-op
/// because `id` is the table's primary key.
pub fn mark_migration_done(conn: &Connection) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO model_catalog_migration_state (id, migrated_at) VALUES (1, ?1)",
        params![Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Insert one migrated seed row. Used only by the one-time migration in
/// `core::model_catalog::migrate_hardcoded_catalog_once`. A no-op when the
/// identity already exists (defensive: the migration itself is already
/// guarded by `migration_already_ran`, but this keeps the function safe to
/// call from a test or a manual replay).
#[allow(clippy::too_many_arguments)]
pub fn insert_migrated_seed(
    conn: &Connection,
    agent_type: &AgentType,
    model_id: &str,
    display_name: &str,
    tier_assignment: Option<ModelTier>,
    capabilities: &[String],
    reasoning_modes: &[String],
) -> Result<()> {
    let runtime_target_id = agent_runtime_target_id(agent_type);
    insert_migrated_seed_for_target(
        conn,
        &runtime_target_id,
        agent_type,
        model_id,
        display_name,
        tier_assignment,
        capabilities,
        reasoning_modes,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn insert_migrated_seed_for_target(
    conn: &Connection,
    runtime_target_id: &str,
    agent_type: &AgentType,
    model_id: &str,
    display_name: &str,
    tier_assignment: Option<ModelTier>,
    capabilities: &[String],
    reasoning_modes: &[String],
) -> Result<()> {
    validate_runtime_target_projection(runtime_target_id, agent_type)?;
    if get(conn, runtime_target_id, model_id)?.is_some() {
        return Ok(());
    }
    let id = canonical_id(runtime_target_id, model_id);
    let now = Utc::now().to_rfc3339();
    conn.execute(
        &format!(
            "INSERT INTO model_catalog_entries ({COLUMNS}) VALUES \
             (?1,?2,?3,?4,?5,NULL,'migrated','available',NULL,NULL,?6,?7,NULL,?8,NULL,NULL,0,?9,NULL,?9,?9,?9)"
        ),
        params![
            id,
            runtime_target_id,
            format_agent_type(agent_type),
            model_id,
            display_name,
            serde_json::to_string(capabilities)?,
            serde_json::to_string(reasoning_modes)?,
            tier_assignment.map(format_tier),
            now,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrations::run(&conn).unwrap();
        conn
    }

    fn req(model_id: &str, tier: Option<ModelTier>) -> UpsertManualModelRequest {
        UpsertManualModelRequest {
            runtime_target_id: "http:connection-a".into(),
            agent_type: AgentType::LiteLlm,
            model_id: model_id.into(),
            display_name: format!("Display {model_id}"),
            capabilities: vec!["chat".into()],
            reasoning_modes: vec![],
            default_reasoning_mode: None,
            tier_assignment: tier,
            cost_hint: None,
            privacy_note: None,
        }
    }

    #[test]
    fn create_manual_then_get_round_trips() {
        let conn = test_conn();
        let created = create_manual(&conn, &req("gpt-x", Some(ModelTier::Economy)))
            .unwrap()
            .expect("created");
        assert_eq!(created.provenance, ModelProvenance::Manual);
        assert!(created.manual_origin);
        assert_eq!(created.tier_assignment, Some(ModelTier::Economy));
        assert_eq!(
            created.last_seen_at, None,
            "manual is not a live observation"
        );

        let fetched = get(&conn, "http:connection-a", "gpt-x").unwrap().unwrap();
        assert_eq!(fetched.id, created.id);
    }

    #[test]
    fn create_manual_rejects_duplicate_identity() {
        let conn = test_conn();
        create_manual(&conn, &req("gpt-x", None)).unwrap();
        let second = create_manual(&conn, &req("gpt-x", None)).unwrap();
        assert!(second.is_none());
    }

    #[test]
    fn create_manual_rejects_mismatched_cli_projection_and_unbounded_fields() {
        let conn = test_conn();
        let mismatched = UpsertManualModelRequest {
            runtime_target_id: "agent:codex".into(),
            agent_type: AgentType::Nvidia,
            ..req("model", None)
        };
        assert!(create_manual(&conn, &mismatched)
            .unwrap_err()
            .to_string()
            .contains("does not project"));

        let oversized = UpsertManualModelRequest {
            model_id: "x".repeat(257),
            ..req("model", None)
        };
        assert!(create_manual(&conn, &oversized).is_err());
    }

    #[test]
    fn runtime_target_and_canonical_identity_are_unambiguous() {
        assert!(validate_runtime_target_id("http:connection:ambiguous").is_err());
        assert!(validate_runtime_target_id("agent:").is_err());
        let left = canonical_id("http:connection-a", "vendor:model");
        let right = canonical_id("http:connection-b", "vendor:model");
        assert_ne!(left, right);
        assert!(!left.contains("http:"), "canonical ids are opaque");
    }

    #[test]
    fn tier_assignment_is_exclusive_per_agent() {
        let conn = test_conn();
        create_manual(&conn, &req("model-a", Some(ModelTier::Economy))).unwrap();
        create_manual(&conn, &req("model-b", Some(ModelTier::Economy))).unwrap();
        let a = get(&conn, "http:connection-a", "model-a").unwrap().unwrap();
        let b = get(&conn, "http:connection-a", "model-b").unwrap().unwrap();
        assert_eq!(
            a.tier_assignment, None,
            "reassigning the tier must clear the previous holder"
        );
        assert_eq!(b.tier_assignment, Some(ModelTier::Economy));
    }

    #[test]
    fn reconcile_live_promotes_manual_entry_preserving_overrides() {
        let conn = test_conn();
        create_manual(&conn, &req("shared-model", Some(ModelTier::Reasoning))).unwrap();
        update_manual(
            &conn,
            "http:connection-a",
            "shared-model",
            &UpsertManualModelRequest {
                display_name: "Operator Alias".into(),
                ..req("shared-model", Some(ModelTier::Reasoning))
            },
        )
        .unwrap();

        reconcile_live(
            &conn,
            "http:connection-a",
            &AgentType::LiteLlm,
            &[DiscoveredModel {
                model_id: "shared-model".into(),
                display_name: "Provider Name".into(),
                capabilities: vec!["chat".into(), "tools".into()],
                reasoning_modes: vec![],
                default_reasoning_mode: None,
            }],
        )
        .unwrap();

        let merged = get(&conn, "http:connection-a", "shared-model")
            .unwrap()
            .unwrap();
        assert_eq!(merged.provenance, ModelProvenance::Live);
        assert!(
            merged.manual_origin,
            "audit trail must remember the manual origin"
        );
        assert_eq!(
            merged.tier_assignment,
            Some(ModelTier::Reasoning),
            "operator tier assignment must survive reconciliation"
        );
        assert_eq!(merged.display_name, "Provider Name");
    }

    #[test]
    fn reconcile_live_is_idempotent() {
        let conn = test_conn();
        let discovered = vec![DiscoveredModel {
            model_id: "m1".into(),
            display_name: "M1".into(),
            capabilities: vec![],
            reasoning_modes: vec![],
            default_reasoning_mode: None,
        }];
        reconcile_live(&conn, "http:connection-a", &AgentType::LiteLlm, &discovered).unwrap();
        reconcile_live(&conn, "http:connection-a", &AgentType::LiteLlm, &discovered).unwrap();
        let all = list_for_target(&conn, "http:connection-a").unwrap();
        assert_eq!(
            all.len(),
            1,
            "replaying the same snapshot must not duplicate rows"
        );
    }

    #[test]
    fn disappearance_downgrades_to_cached_never_deletes() {
        let conn = test_conn();
        reconcile_live(
            &conn,
            "http:connection-a",
            &AgentType::LiteLlm,
            &[DiscoveredModel {
                model_id: "m1".into(),
                display_name: "M1".into(),
                capabilities: vec![],
                reasoning_modes: vec![],
                default_reasoning_mode: None,
            }],
        )
        .unwrap();
        reconcile_live(&conn, "http:connection-a", &AgentType::LiteLlm, &[]).unwrap();
        let entry = get(&conn, "http:connection-a", "m1").unwrap().unwrap();
        assert_eq!(entry.provenance, ModelProvenance::Cached);
        assert_eq!(entry.availability, ModelAvailability::Unavailable);
        assert_eq!(
            entry.unavailable_reason,
            Some(ModelUnavailableReason::Disappeared)
        );
    }

    #[test]
    fn failed_refresh_downgrades_live_provenance_without_claiming_disappearance() {
        let conn = test_conn();
        reconcile_live(
            &conn,
            "http:connection-a",
            &AgentType::LiteLlm,
            &[DiscoveredModel {
                model_id: "m1".into(),
                display_name: "M1".into(),
                capabilities: vec![],
                reasoning_modes: vec![],
                default_reasoning_mode: None,
            }],
        )
        .unwrap();
        let seen_at = get(&conn, "http:connection-a", "m1")
            .unwrap()
            .unwrap()
            .last_seen_at;
        record_refresh_failure(
            &conn,
            "http:connection-a",
            &AgentType::LiteLlm,
            ModelUnavailableReason::Timeout,
            "timed out",
        )
        .unwrap();
        let cached = get(&conn, "http:connection-a", "m1").unwrap().unwrap();
        assert_eq!(cached.provenance, ModelProvenance::Cached);
        assert_eq!(cached.availability, ModelAvailability::Available);
        assert_eq!(cached.unavailable_reason, None);
        assert_eq!(cached.last_seen_at, seen_at);
    }

    #[test]
    fn mark_unavailable_then_reappear_reactivates() {
        let conn = test_conn();
        create_manual(&conn, &req("flaky", None)).unwrap();
        mark_unavailable(
            &conn,
            "http:connection-a",
            "flaky",
            ModelUnavailableReason::Disappeared,
            Some("absent from last catalogue"),
        )
        .unwrap();
        let gone = get(&conn, "http:connection-a", "flaky").unwrap().unwrap();
        assert_eq!(gone.availability, ModelAvailability::Unavailable);
        assert_eq!(
            gone.unavailable_reason,
            Some(ModelUnavailableReason::Disappeared)
        );

        mark_available(&conn, "http:connection-a", "flaky").unwrap();
        let back = get(&conn, "http:connection-a", "flaky").unwrap().unwrap();
        assert_eq!(back.availability, ModelAvailability::Available);
        assert_eq!(back.unavailable_reason, None);
    }

    #[test]
    fn delete_manual_refuses_live_entry() {
        let conn = test_conn();
        reconcile_live(
            &conn,
            "http:connection-a",
            &AgentType::LiteLlm,
            &[DiscoveredModel {
                model_id: "m1".into(),
                display_name: "M1".into(),
                capabilities: vec![],
                reasoning_modes: vec![],
                default_reasoning_mode: None,
            }],
        )
        .unwrap();
        let err = delete_manual(&conn, "http:connection-a", "m1").unwrap_err();
        assert!(err.to_string().contains("live-sourced"));
    }

    #[test]
    fn resolve_tier_entry_prefers_higher_provenance() {
        let live = CatalogModelEntry {
            provenance: ModelProvenance::Live,
            ..fixture_entry("live-model")
        };
        let migrated = CatalogModelEntry {
            provenance: ModelProvenance::Migrated,
            ..fixture_entry("migrated-model")
        };
        let entries = [migrated, live.clone()];
        let picked = resolve_tier_entry(&entries, ModelTier::Economy).unwrap();
        assert_eq!(picked.model_id, live.model_id);
    }

    fn fixture_entry(model_id: &str) -> CatalogModelEntry {
        let now = Utc::now();
        CatalogModelEntry {
            id: canonical_id("http:connection-a", model_id),
            runtime_target_id: "http:connection-a".into(),
            agent_type: AgentType::LiteLlm,
            model_id: model_id.into(),
            display_name: model_id.into(),
            display_alias: None,
            provenance: ModelProvenance::Manual,
            availability: ModelAvailability::Available,
            unavailable_reason: None,
            unavailable_detail: None,
            capabilities: vec![],
            reasoning_modes: vec![],
            default_reasoning_mode: None,
            tier_assignment: Some(ModelTier::Economy),
            cost_hint: None,
            privacy_note: None,
            manual_origin: true,
            first_seen_at: now,
            last_seen_at: None,
            last_checked_at: now,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn identical_http_model_ids_are_isolated_by_connection() {
        let conn = test_conn();
        create_manual(&conn, &req("shared", Some(ModelTier::Economy))).unwrap();
        create_manual(
            &conn,
            &UpsertManualModelRequest {
                runtime_target_id: "http:connection-b".into(),
                display_name: "Connection B model".into(),
                ..req("shared", Some(ModelTier::Reasoning))
            },
        )
        .unwrap();

        let a = get(&conn, "http:connection-a", "shared").unwrap().unwrap();
        let b = get(&conn, "http:connection-b", "shared").unwrap().unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(a.tier_assignment, Some(ModelTier::Economy));
        assert_eq!(b.tier_assignment, Some(ModelTier::Reasoning));
    }

    #[test]
    fn opencode_zen_overlay_is_structural_not_a_model_list() {
        // Positive: any `opencode/<anything>` id under the OpenCode runtime
        // target — never a hardcoded model name.
        for model_id in ["opencode/big-pickle", "opencode/claude-sonnet-5", "opencode/whatever-ships-next"] {
            let (cost, note) = derive_opencode_zen_overlay("agent:opencode", model_id);
            assert_eq!(cost, Some(ModelCostHint::Unknown), "{model_id}");
            assert!(note.is_some(), "{model_id}");
        }
    }

    #[test]
    fn opencode_zen_overlay_ignores_non_zen_opencode_models_and_other_runtimes() {
        // A model configured directly (not via the opencode/ provider
        // namespace) on the OpenCode runtime is not Zen-routed.
        assert_eq!(
            derive_opencode_zen_overlay("agent:opencode", "anthropic/claude-sonnet-5"),
            (None, None)
        );
        assert_eq!(
            derive_opencode_zen_overlay("agent:opencode", "gpt-5.6-sol"),
            (None, None)
        );
        // Same-shaped id on a different runtime must not match.
        assert_eq!(
            derive_opencode_zen_overlay("agent:codex", "opencode/big-pickle"),
            (None, None)
        );
        assert_eq!(
            derive_opencode_zen_overlay("http:connection-a", "opencode/big-pickle"),
            (None, None)
        );
    }

    #[test]
    fn reconcile_live_auto_tags_new_opencode_zen_models() {
        let conn = test_conn();
        reconcile_live(
            &conn,
            "agent:opencode",
            &AgentType::OpenCode,
            &[
                DiscoveredModel {
                    model_id: "opencode/big-pickle".into(),
                    display_name: "Big Pickle".into(),
                    capabilities: vec!["chat".into()],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                },
                DiscoveredModel {
                    model_id: "anthropic/claude-sonnet-5".into(),
                    display_name: "Claude Sonnet 5".into(),
                    capabilities: vec!["chat".into()],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                },
            ],
        )
        .unwrap();

        let zen = get(&conn, "agent:opencode", "opencode/big-pickle")
            .unwrap()
            .unwrap();
        assert_eq!(zen.cost_hint, Some(ModelCostHint::Unknown));
        assert!(zen.privacy_note.is_some());

        let direct = get(&conn, "agent:opencode", "anthropic/claude-sonnet-5")
            .unwrap()
            .unwrap();
        assert_eq!(direct.cost_hint, None);
        assert_eq!(direct.privacy_note, None);
    }

    #[test]
    fn reconcile_live_never_overwrites_an_existing_cost_hint_override() {
        let conn = test_conn();
        reconcile_live(
            &conn,
            "agent:opencode",
            &AgentType::OpenCode,
            &[DiscoveredModel {
                model_id: "opencode/big-pickle".into(),
                display_name: "Big Pickle".into(),
                capabilities: vec![],
                reasoning_modes: vec![],
                default_reasoning_mode: None,
            }],
        )
        .unwrap();
        update_manual(
            &conn,
            "agent:opencode",
            "opencode/big-pickle",
            &UpsertManualModelRequest {
                runtime_target_id: "agent:opencode".into(),
                agent_type: AgentType::OpenCode,
                cost_hint: Some(ModelCostHint::Free),
                privacy_note: Some("Operator-confirmed free promo.".into()),
                ..req("opencode/big-pickle", None)
            },
        )
        .unwrap();

        // Re-running discovery (e.g. next TTL refresh) must not clobber the
        // operator's correction with the auto-derived Unknown default.
        reconcile_live(
            &conn,
            "agent:opencode",
            &AgentType::OpenCode,
            &[DiscoveredModel {
                model_id: "opencode/big-pickle".into(),
                display_name: "Big Pickle".into(),
                capabilities: vec![],
                reasoning_modes: vec![],
                default_reasoning_mode: None,
            }],
        )
        .unwrap();

        let entry = get(&conn, "agent:opencode", "opencode/big-pickle")
            .unwrap()
            .unwrap();
        assert_eq!(entry.cost_hint, Some(ModelCostHint::Free));
        assert_eq!(
            entry.privacy_note.as_deref(),
            Some("Operator-confirmed free promo.")
        );
    }

    #[test]
    fn update_manual_coalesces_cost_fields_instead_of_clearing_on_unrelated_edit() {
        let conn = test_conn();
        create_manual(
            &conn,
            &UpsertManualModelRequest {
                cost_hint: Some(ModelCostHint::Paid),
                privacy_note: Some("Billed per token.".into()),
                ..req("model-a", Some(ModelTier::Economy))
            },
        )
        .unwrap();

        // An unrelated rename that doesn't send cost_hint/privacy_note must
        // not silently wipe them.
        update_manual(
            &conn,
            "http:connection-a",
            "model-a",
            &UpsertManualModelRequest {
                display_name: "Renamed".into(),
                ..req("model-a", Some(ModelTier::Economy))
            },
        )
        .unwrap();

        let entry = get(&conn, "http:connection-a", "model-a").unwrap().unwrap();
        assert_eq!(entry.display_name, "Renamed");
        assert_eq!(entry.cost_hint, Some(ModelCostHint::Paid));
        assert_eq!(entry.privacy_note.as_deref(), Some("Billed per token."));

        // An explicit correction still overwrites.
        update_manual(
            &conn,
            "http:connection-a",
            "model-a",
            &UpsertManualModelRequest {
                cost_hint: Some(ModelCostHint::Free),
                ..req("model-a", Some(ModelTier::Economy))
            },
        )
        .unwrap();
        let corrected = get(&conn, "http:connection-a", "model-a").unwrap().unwrap();
        assert_eq!(corrected.cost_hint, Some(ModelCostHint::Free));
        assert_eq!(
            corrected.privacy_note.as_deref(),
            Some("Billed per token."),
            "an unrelated field in the same request must not clear privacy_note"
        );
    }

    #[test]
    fn create_manual_persists_operator_cost_hint_and_privacy_note() {
        let conn = test_conn();
        let created = create_manual(
            &conn,
            &UpsertManualModelRequest {
                cost_hint: Some(ModelCostHint::Unknown),
                privacy_note: Some("Self-hosted, no third party.".into()),
                ..req("model-x", None)
            },
        )
        .unwrap()
        .expect("created");
        assert_eq!(created.cost_hint, Some(ModelCostHint::Unknown));
        assert_eq!(
            created.privacy_note.as_deref(),
            Some("Self-hosted, no third party.")
        );
    }
}
