//! KT-531 model catalog resolution contract.
//!
//! Resolution order is fixed and never reordered by a caller: live discovery,
//! then the last valid live snapshot (flagged `Cached`, explicitly stale), then
//! an operator manual entry, then a one-time migrated seed. This module owns discovery
//! orchestration (timeout + normalized error classification), the TTL that
//! decides when a consumer should re-run discovery instead of reusing the
//! last snapshot, and the preflight helper shared by every launch surface
//! (discussion, Quick Prompt, comparison, workflow step).
//!
//! HTTP model providers use the same persistence and reconciliation contract
//! as CLI families. Their transport and codec remain owned by
//! `external_api_connections`; only the stable catalog target identity is
//! shared here.

pub mod acp_discovery;
pub mod claude_discovery;
pub mod codex_discovery;
pub mod ollama_discovery;

use std::time::Duration;
use std::{collections::HashMap, sync::LazyLock};

use chrono::Utc;
use tokio::time::timeout;

use crate::db::model_catalog::{self as db, DiscoveredModel};
use crate::db::Database;
use crate::models::{
    AgentType, AppConfig, CatalogModelEntry, CatalogPreflightFailure, CatalogPreflightResolution,
    CatalogPreflightWarning, CatalogReplacementSource, ModelAvailability, ModelCatalogAlert,
    ModelCatalogReference, ModelCatalogReferenceKind, ModelCatalogView, ModelProvenance, ModelTier,
    ModelTierConfig, ModelTiersConfig, ModelUnavailableReason, StepType,
};

/// How long a successful live snapshot is trusted before a consumer should
/// treat it as stale. Refresh is always triggered explicitly (a selector
/// opening, a preflight check, an operator recheck) — this constant only
/// decides whether that trigger re-runs discovery or reuses the DB snapshot.
pub const LIVE_CATALOG_TTL: Duration = Duration::from_secs(600);

/// Bound on one discovery attempt: process spawn, handshake and listing.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);

static RESOLVED_TIERS: LazyLock<std::sync::RwLock<HashMap<(String, u8), String>>> =
    LazyLock::new(|| std::sync::RwLock::new(HashMap::new()));

/// The runner starts after catalogue preflight, on a synchronous hot path. Keep
/// the discovered effort modes alongside the tier projection so it can reject a
/// stale or incompatible preset without inventing a provider-wide effort list.
type ReasoningModesByModel = HashMap<(String, String), Vec<String>>;
static RESOLVED_REASONING_MODES: LazyLock<std::sync::RwLock<ReasoningModesByModel>> =
    LazyLock::new(|| std::sync::RwLock::new(HashMap::new()));

fn tier_key(tier: ModelTier) -> u8 {
    match tier {
        ModelTier::Economy => 0,
        ModelTier::Default => 1,
        ModelTier::Reasoning => 2,
    }
}

/// Synchronous hot-path lookup used by the runner after startup/API writes
/// have projected the durable catalog into memory.
pub fn assigned_model_for_agent(agent_type: &AgentType, tier: ModelTier) -> Option<String> {
    let target = db::agent_runtime_target_id(agent_type);
    RESOLVED_TIERS
        .read()
        .ok()
        .and_then(|catalog| catalog.get(&(target, tier_key(tier))).cloned())
}

/// Returns the modes the durable catalogue discovered for this exact runtime
/// target and model. `None` means the model is not in the current projection;
/// callers must not send an effort option in that case.
pub fn reasoning_modes_for_agent_model(
    agent_type: &AgentType,
    model_id: &str,
) -> Option<Vec<String>> {
    let target = db::agent_runtime_target_id(agent_type);
    RESOLVED_REASONING_MODES
        .read()
        .ok()
        .and_then(|catalog| catalog.get(&(target, model_id.to_owned())).cloned())
}

/// Historical per-agent tier defaults used by the one-time seed migration and
/// as the last same-agent fallback when a configured model disappears. A
/// fallback is still accepted only when this exact identity is live in the
/// durable catalogue; these literals never bypass discovery.
pub fn agent_default_for_tier(agent_type: &AgentType, tier: ModelTier) -> Option<String> {
    match (agent_type, tier) {
        (AgentType::ClaudeCode, ModelTier::Economy) => Some("haiku".into()),
        (AgentType::ClaudeCode, ModelTier::Default) => Some("sonnet".into()),
        (AgentType::ClaudeCode, ModelTier::Reasoning) => Some("opus".into()),
        (AgentType::Codex, ModelTier::Economy) => Some("gpt-5.6-luna".into()),
        (AgentType::Codex, ModelTier::Reasoning) => Some("gpt-5.6-sol".into()),
        (AgentType::GeminiCli, ModelTier::Economy) => Some("gemini-2.5-flash".into()),
        (AgentType::GeminiCli, ModelTier::Reasoning) => Some("gemini-3.1-pro-preview".into()),
        (AgentType::Ollama, ModelTier::Economy | ModelTier::Default) => Some("qwen3:8b".into()),
        (AgentType::Ollama, ModelTier::Reasoning) => Some("qwen3:30b-a3b".into()),
        _ => None,
    }
}

#[cfg(test)]
pub fn migrated_default(agent_type: &AgentType, tier: ModelTier) -> Option<String> {
    agent_default_for_tier(agent_type, tier)
}

pub async fn refresh_runtime_cache(database: &Database) -> anyhow::Result<()> {
    let entries = database.with_read_conn(db::list_all).await?;
    let mut resolved = HashMap::new();
    let mut reasoning_modes = HashMap::new();
    for entry in entries {
        if entry.availability == ModelAvailability::Available {
            reasoning_modes.insert(
                (entry.runtime_target_id.clone(), entry.model_id.clone()),
                entry.reasoning_modes.clone(),
            );
        }
        let Some(tier) = entry.tier_assignment else {
            continue;
        };
        if entry.availability != ModelAvailability::Available {
            continue;
        }
        resolved.insert((entry.runtime_target_id, tier_key(tier)), entry.model_id);
    }
    if let Ok(mut catalog) = RESOLVED_TIERS.write() {
        *catalog = resolved;
    }
    if let Ok(mut catalog) = RESOLVED_REASONING_MODES.write() {
        *catalog = reasoning_modes;
    }
    Ok(())
}

/// Convert the former embedded CLI catalogue and current operator overrides
/// exactly once. These values are migration input only; live/manual catalog
/// rows become the runtime source after bootstrap.
pub async fn migrate_hardcoded_catalog_once(
    database: &Database,
    config: &AppConfig,
) -> anyhow::Result<()> {
    let already_done = database.with_read_conn(db::migration_already_ran).await?;
    if !already_done {
        let tiers = config.agents.model_tiers.clone();
        database
            .with_conn(move |conn| {
                if db::migration_already_ran(conn)? {
                    return Ok(());
                }
                let chat = vec!["chat".to_string()];
                let seed = |conn: &rusqlite::Connection,
                            agent: AgentType,
                            model: &str,
                            tier: ModelTier,
                            reasoning: &[&str]|
                 -> anyhow::Result<()> {
                    // Historical defaults are fallback data, not competing
                    // assignments for a tier the operator already configured.
                    let tier_assignment = match crate::agents::runner::configured_model_flag(
                        &agent,
                        tier,
                        Some(&tiers),
                    ) {
                        Some(configured) if configured != model => None,
                        _ => Some(tier),
                    };
                    db::insert_migrated_seed(
                        conn,
                        &agent,
                        model,
                        model,
                        tier_assignment,
                        &chat,
                        &reasoning
                            .iter()
                            .map(|value| (*value).to_string())
                            .collect::<Vec<_>>(),
                    )
                };
                seed(
                    conn,
                    AgentType::ClaudeCode,
                    "haiku",
                    ModelTier::Economy,
                    &[],
                )?;
                seed(
                    conn,
                    AgentType::ClaudeCode,
                    "sonnet",
                    ModelTier::Default,
                    &[],
                )?;
                seed(
                    conn,
                    AgentType::ClaudeCode,
                    "opus",
                    ModelTier::Reasoning,
                    &[],
                )?;
                seed(
                    conn,
                    AgentType::Codex,
                    "gpt-5.6-luna",
                    ModelTier::Economy,
                    &["low", "medium", "high"],
                )?;
                seed(
                    conn,
                    AgentType::Codex,
                    "gpt-5.6-sol",
                    ModelTier::Reasoning,
                    &["low", "medium", "high", "xhigh"],
                )?;
                seed(
                    conn,
                    AgentType::GeminiCli,
                    "gemini-2.5-flash",
                    ModelTier::Economy,
                    &[],
                )?;
                seed(conn, AgentType::Ollama, "qwen3:8b", ModelTier::Default, &[])?;
                seed(
                    conn,
                    AgentType::Ollama,
                    "qwen3:30b-a3b",
                    ModelTier::Reasoning,
                    &[],
                )?;
                seed(
                    conn,
                    AgentType::GeminiCli,
                    "gemini-3.1-pro-preview",
                    ModelTier::Reasoning,
                    &[],
                )?;

                for (agent, cfg) in [
                    (AgentType::ClaudeCode, &tiers.claude_code),
                    (AgentType::Codex, &tiers.codex),
                    (AgentType::OpenCode, &tiers.open_code),
                    (AgentType::GeminiCli, &tiers.gemini_cli),
                    (AgentType::Kiro, &tiers.kiro),
                    (AgentType::CopilotCli, &tiers.copilot_cli),
                    (AgentType::Vibe, &tiers.vibe),
                    (AgentType::Ollama, &tiers.ollama),
                ] {
                    seed_configured_tiers(conn, agent, cfg)?;
                }
                db::mark_migration_done(conn)
            })
            .await?;
    }
    refresh_runtime_cache(database).await
}

fn seed_configured_tiers(
    conn: &rusqlite::Connection,
    agent: AgentType,
    config: &ModelTierConfig,
) -> anyhow::Result<()> {
    for (tier, model) in [
        (ModelTier::Economy, config.economy.as_deref()),
        (ModelTier::Default, config.default.as_deref()),
        (ModelTier::Reasoning, config.reasoning.as_deref()),
    ] {
        if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
            db::insert_migrated_seed(
                conn,
                &agent,
                model,
                model,
                Some(tier),
                &["chat".to_string()],
                &[],
            )?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiscoveryOutcome {
    Live(Vec<DiscoveredModel>),
    AuthRequired(String),
    Timeout,
    CliMissing(String),
    InvalidCatalog(String),
    ProviderError(String),
    /// No live discovery path exists for this runtime (e.g. an ACP implementation that
    /// negotiates successfully but exposes no model catalogue) — resolution
    /// falls through to cache/manual/migrated.
    Unsupported,
}

/// The seven CLI-based runtimes KT-531 governs.
pub fn is_catalog_managed(agent_type: &AgentType) -> bool {
    matches!(
        agent_type,
        AgentType::ClaudeCode
            | AgentType::Codex
            | AgentType::OpenCode
            | AgentType::GeminiCli
            | AgentType::Kiro
            | AgentType::CopilotCli
            | AgentType::Vibe
    )
}

async fn discover(agent_type: &AgentType) -> DiscoveryOutcome {
    #[cfg(test)]
    if let Ok(outcome) = TEST_DISCOVERY.try_with(Clone::clone) {
        let _ = TEST_DISCOVERY_CALLS.try_with(|calls| calls.set(calls.get() + 1));
        return outcome;
    }
    match timeout(DISCOVERY_TIMEOUT, discover_inner(agent_type)).await {
        Ok(outcome) => outcome,
        Err(_) => DiscoveryOutcome::Timeout,
    }
}

#[cfg(test)]
tokio::task_local! {
    static TEST_DISCOVERY: DiscoveryOutcome;
    static TEST_DISCOVERY_CALLS: std::cell::Cell<usize>;
}

async fn discover_inner(agent_type: &AgentType) -> DiscoveryOutcome {
    match agent_type {
        AgentType::OpenCode
        | AgentType::GeminiCli
        | AgentType::CopilotCli
        | AgentType::Kiro
        | AgentType::Vibe => acp_discovery::discover(agent_type).await,
        AgentType::Codex => codex_discovery::discover().await,
        AgentType::ClaudeCode => claude_discovery::discover().await,
        _ => DiscoveryOutcome::Unsupported,
    }
}

fn reason_for(outcome: &DiscoveryOutcome) -> Option<(ModelUnavailableReason, String)> {
    match outcome {
        DiscoveryOutcome::Live(_) => None,
        DiscoveryOutcome::AuthRequired(detail) => {
            Some((ModelUnavailableReason::AuthRequired, detail.clone()))
        }
        DiscoveryOutcome::Timeout => Some((
            ModelUnavailableReason::Timeout,
            "discovery did not complete within the bound".into(),
        )),
        DiscoveryOutcome::CliMissing(detail) => {
            Some((ModelUnavailableReason::CliMissing, detail.clone()))
        }
        DiscoveryOutcome::InvalidCatalog(detail) => {
            Some((ModelUnavailableReason::InvalidCatalog, detail.clone()))
        }
        DiscoveryOutcome::ProviderError(detail) => {
            Some((ModelUnavailableReason::ProviderError, detail.clone()))
        }
        DiscoveryOutcome::Unsupported => Some((
            ModelUnavailableReason::Unsupported,
            "no live discovery path is implemented for this runtime".into(),
        )),
    }
}

/// Run discovery for one runtime and persist the result (reconciliation on
/// success, a refresh-log-only failure record otherwise), then return the
/// resulting view. Used by the manual "recheck" action and by the TTL-driven
/// refresh path.
pub async fn refresh_agent_catalog(
    db: &Database,
    agent_type: AgentType,
) -> anyhow::Result<ModelCatalogView> {
    refresh_if_stale(db, agent_type, true).await
}

/// Files OpenCode reads as a project's own configuration, relative to the
/// project directory. Their presence alone decides whether a project can offer
/// models the user-level configuration does not: the files are never opened, so
/// nothing they hold (a key, a header) can reach the catalogue.
const OPENCODE_PROJECT_CONFIGS: [&str; 4] = [
    "opencode.json",
    "opencode.jsonc",
    ".opencode/opencode.json",
    ".opencode/opencode.jsonc",
];

/// A project scope is one extra OpenCode process per refresh, so the number of
/// projects consulted is bounded; going past it is logged, never silent.
const MAX_OPENCODE_PROJECT_SCOPES: usize = 16;

/// OpenCode builds its model list per working directory, and a launch runs in
/// the project directory while discovery runs in a neutral one. A provider
/// declared only by a project's `opencode.json` — a local Ollama is the usual
/// case — is therefore offered to every run in that project yet invisible to
/// discovery. Keep, among `projects` (name, resolved directory), those that
/// carry their own OpenCode config; they are the only ones worth asking.
fn opencode_project_scopes(
    projects: &[(String, std::path::PathBuf)],
) -> Vec<(String, std::path::PathBuf)> {
    let mut scopes: Vec<(String, std::path::PathBuf)> = projects
        .iter()
        .filter(|(_, dir)| {
            dir.is_dir()
                && OPENCODE_PROJECT_CONFIGS
                    .iter()
                    .any(|config| dir.join(config).is_file())
        })
        .cloned()
        .collect();
    if scopes.len() > MAX_OPENCODE_PROJECT_SCOPES {
        tracing::warn!(
            projects = scopes.len(),
            limit = MAX_OPENCODE_PROJECT_SCOPES,
            "more projects carry an OpenCode config than one refresh consults; the rest are left out"
        );
        scopes.truncate(MAX_OPENCODE_PROJECT_SCOPES);
    }
    scopes
}

/// Fold what OpenCode offers inside some projects into what it offers
/// everywhere. Every model comes from an OpenCode session response — nothing is
/// added by Kronn — and one already offered everywhere is not repeated. A model
/// only a project offers says so in its description, since it is absent from a
/// run in any other directory. A failed project scope is left out and logged
/// (its reason only): it never turns the refresh into a failure, because one
/// project's broken config must not take OpenCode away from every other project.
/// A failed global discovery is returned as it is, project results or not.
fn merge_project_scopes(
    global: DiscoveryOutcome,
    scoped: Vec<(String, DiscoveryOutcome)>,
) -> DiscoveryOutcome {
    let DiscoveryOutcome::Live(mut models) = global else {
        return global;
    };
    for (project, outcome) in scoped {
        match outcome {
            DiscoveryOutcome::Live(offered) => {
                for model in offered {
                    if models.iter().any(|known| known.model_id == model.model_id) {
                        continue;
                    }
                    let note = format!("Offered by OpenCode only inside the project {project}.");
                    models.push(DiscoveredModel {
                        description: Some(match model.description {
                            Some(description) => format!("{description} {note}"),
                            None => note,
                        }),
                        ..model
                    });
                }
            }
            other => {
                let reason = reason_for(&other).map(|(reason, _)| reason);
                tracing::warn!(
                    project = %project,
                    ?reason,
                    "OpenCode discovery failed in a project directory; its own models are left out of this snapshot"
                );
            }
        }
    }
    DiscoveryOutcome::Live(models)
}

/// OpenCode discovery: the neutral directory and every project scope, run
/// together so the whole refresh keeps one discovery bound.
async fn discover_opencode(db: &Database) -> DiscoveryOutcome {
    discover_opencode_with(db, |dir| async move {
        match timeout(
            DISCOVERY_TIMEOUT,
            acp_discovery::discover_in(&AgentType::OpenCode, &dir),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(_) => DiscoveryOutcome::Timeout,
        }
    })
    .await
}

/// `discover_opencode` with the per-directory discovery supplied, so the
/// project listing, the scope choice and the merge are testable without an
/// OpenCode process.
async fn discover_opencode_with<F, Fut>(db: &Database, discover_scope: F) -> DiscoveryOutcome
where
    F: Fn(std::path::PathBuf) -> Fut,
    Fut: std::future::Future<Output = DiscoveryOutcome>,
{
    let projects = db
        .with_conn(crate::db::projects::list_projects)
        .await
        .map(|projects| {
            projects
                .into_iter()
                .map(|project| {
                    let dir = crate::core::scanner::resolve_host_path(&project.path);
                    (project.name, dir)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "could not list projects for OpenCode discovery; only the user-level catalogue is read");
            Vec::new()
        });
    let scopes = opencode_project_scopes(&projects);
    let discover_scope = &discover_scope;
    let scoped = futures::future::join_all(
        scopes
            .into_iter()
            .map(|(name, dir)| async move { (name, discover_scope(dir).await) }),
    );
    let (global, scoped) = tokio::join!(discover(&AgentType::OpenCode), scoped);
    merge_project_scopes(global, scoped)
}

async fn discover_and_reconcile(
    db: &Database,
    agent_type: AgentType,
) -> anyhow::Result<ModelCatalogView> {
    let runtime_target_id = db::agent_runtime_target_id(&agent_type);
    let outcome = match agent_type {
        AgentType::OpenCode => discover_opencode(db).await,
        _ => discover(&agent_type).await,
    };
    let at = agent_type.clone();
    let target = runtime_target_id.clone();
    match outcome {
        DiscoveryOutcome::Live(models) => {
            db.with_conn(move |conn| {
                let transaction = conn.unchecked_transaction()?;
                db::reconcile_live(&transaction, &target, &at, &models)?;
                transaction.commit()?;
                Ok(())
            })
            .await?;
        }
        other => {
            if let Some((reason, detail)) = reason_for(&other) {
                db.with_conn(move |conn| {
                    db::record_refresh_failure(conn, &target, &at, reason, &detail)
                })
                .await?;
            }
        }
    }
    refresh_runtime_cache(db).await?;
    build_view(db, runtime_target_id, agent_type).await
}

/// Discover and reconcile the configured Ollama HTTP catalogue.
pub async fn refresh_ollama_catalog_at(
    database: &Database,
    base_url: &str,
) -> anyhow::Result<(
    ModelCatalogView,
    Result<Vec<ollama_discovery::OllamaTag>, DiscoveryOutcome>,
)> {
    let outcome = ollama_discovery::discover(base_url).await;
    let target = db::agent_runtime_target_id(&AgentType::Ollama);
    match &outcome {
        Ok(tags) => {
            let models = ollama_discovery::discovered_models(tags);
            let reconcile_target = target.clone();
            database
                .with_conn(move |conn| {
                    let transaction = conn.unchecked_transaction()?;
                    db::reconcile_live(
                        &transaction,
                        &reconcile_target,
                        &AgentType::Ollama,
                        &models,
                    )?;
                    transaction.commit()?;
                    Ok(())
                })
                .await?;
            refresh_runtime_cache(database).await?;
        }
        Err(error) => {
            if let Some((reason, detail)) = reason_for(error) {
                let failure_target = target.clone();
                database
                    .with_conn(move |conn| {
                        let transaction = conn.unchecked_transaction()?;
                        db::record_refresh_failure(
                            &transaction,
                            &failure_target,
                            &AgentType::Ollama,
                            reason,
                            &detail,
                        )?;
                        transaction.commit()?;
                        Ok(())
                    })
                    .await?;
            }
        }
    }
    let view = build_view(database, target, AgentType::Ollama).await?;
    Ok((view, outcome))
}

/// Persist a successful provider catalogue for one named HTTP connection.
pub async fn reconcile_http_catalog(
    database: &Database,
    connection_id: &str,
    agent_type: AgentType,
    models: Vec<DiscoveredModel>,
) -> anyhow::Result<ModelCatalogView> {
    let runtime_target_id = db::http_runtime_target_id(connection_id);
    let target = runtime_target_id.clone();
    let at = agent_type.clone();
    database
        .with_conn(move |conn| db::reconcile_live(conn, &target, &at, &models))
        .await?;
    refresh_runtime_cache(database).await?;
    build_view(database, runtime_target_id, agent_type).await
}

pub async fn record_http_refresh_failure(
    database: &Database,
    connection_id: &str,
    agent_type: AgentType,
    reason: ModelUnavailableReason,
    detail: String,
) -> anyhow::Result<()> {
    let target = db::http_runtime_target_id(connection_id);
    database
        .with_conn(move |conn| {
            db::record_refresh_failure(conn, &target, &agent_type, reason, &detail)
        })
        .await
}

/// Build the current view from whatever is already persisted, without
/// running discovery. Cheap — used for repeated reads within the TTL window.
pub async fn build_view(
    db: &Database,
    runtime_target_id: String,
    agent_type: AgentType,
) -> anyhow::Result<ModelCatalogView> {
    let target = runtime_target_id.clone();
    let (models, log) = db
        .with_conn(move |conn| {
            let models = db::list_for_target(conn, &target)?;
            let log = db::get_refresh_log(conn, &target)?;
            Ok((models, log))
        })
        .await?;
    let last_live_success_at = log.as_ref().and_then(|l| l.last_live_success_at);
    let live_refresh_ok = log
        .as_ref()
        .is_some_and(|l| l.last_error_reason.is_none() && l.last_live_success_at.is_some());
    let stale = match last_live_success_at {
        Some(at) => {
            Utc::now()
                .signed_duration_since(at)
                .to_std()
                .unwrap_or_default()
                > LIVE_CATALOG_TTL
        }
        None => true,
    };
    Ok(ModelCatalogView {
        runtime_target_id,
        target_label: None,
        agent_type,
        models,
        live_refresh_ok,
        stale,
        last_live_success_at,
        last_attempt_at: log.as_ref().map(|l| l.last_attempt_at),
        last_error_reason: log.as_ref().and_then(|l| l.last_error_reason),
        last_error_detail: log.as_ref().and_then(|l| l.last_error_detail.clone()),
        alerts: Vec::new(),
    })
}

fn live_replacement_for_entry(
    entries: &[CatalogModelEntry],
    missing: &CatalogModelEntry,
) -> Option<String> {
    let resolved = missing.resolved_model.as_deref()?;
    let alias_stem = missing.model_id.split_once('[').map(|(stem, _suffix)| stem);
    entries
        .iter()
        .filter(|candidate| {
            candidate.model_id != missing.model_id
                && candidate.provenance == ModelProvenance::Live
                && candidate.availability == ModelAvailability::Available
                && candidate.resolved_model.as_deref() == Some(resolved)
        })
        .min_by_key(|candidate| {
            (
                alias_stem != Some(candidate.model_id.as_str()),
                candidate.model_id == "default",
                candidate.model_id.clone(),
            )
        })
        .map(|candidate| candidate.model_id.clone())
}

fn available_chat_entry<'a>(
    entries: &'a [CatalogModelEntry],
    model_id: &str,
) -> Option<&'a CatalogModelEntry> {
    entries.iter().find(|entry| {
        entry.model_id == model_id
            && entry.availability == ModelAvailability::Available
            && crate::http_transport::entry_supports_capability(
                entry,
                crate::http_transport::CAPABILITY_CHAT,
            )
    })
}

fn tier_replacement_for_entry(
    entries: &[CatalogModelEntry],
    missing: &CatalogModelEntry,
    agent_type: &AgentType,
    selected_tier: ModelTier,
    model_tiers: Option<&ModelTiersConfig>,
) -> Option<(String, ModelTier)> {
    let equivalent_tier = missing.tier_assignment.unwrap_or(selected_tier);
    let configured =
        crate::agents::runner::configured_model_flag(agent_type, equivalent_tier, model_tiers);
    configured
        .as_deref()
        .filter(|candidate| *candidate != missing.model_id)
        .and_then(|candidate| available_chat_entry(entries, candidate))
        .or_else(|| {
            db::resolve_tier_entry(entries, equivalent_tier).filter(|candidate| {
                crate::http_transport::entry_supports_capability(
                    candidate,
                    crate::http_transport::CAPABILITY_CHAT,
                )
            })
        })
        .filter(|candidate| candidate.model_id != missing.model_id)
        .or_else(|| {
            agent_default_for_tier(agent_type, equivalent_tier)
                .filter(|candidate| *candidate != missing.model_id)
                .and_then(|candidate| available_chat_entry(entries, &candidate))
        })
        .map(|candidate| (candidate.model_id.clone(), equivalent_tier))
}

fn configured_tier_models(
    config: &ModelTiersConfig,
    agent_type: &AgentType,
) -> Vec<(ModelTier, String)> {
    let tier_config = match agent_type {
        AgentType::ClaudeCode => Some(&config.claude_code),
        AgentType::Codex => Some(&config.codex),
        AgentType::OpenCode => Some(&config.open_code),
        AgentType::GeminiCli => Some(&config.gemini_cli),
        AgentType::Kiro => Some(&config.kiro),
        AgentType::Vibe => Some(&config.vibe),
        AgentType::CopilotCli => Some(&config.copilot_cli),
        AgentType::Ollama => Some(&config.ollama),
        AgentType::LiteLlm => Some(&config.lite_llm),
        AgentType::Nvidia => Some(&config.nvidia),
        AgentType::Custom => None,
    };
    let Some(tier_config) = tier_config else {
        return Vec::new();
    };
    [
        (ModelTier::Economy, tier_config.economy.as_ref()),
        (ModelTier::Default, tier_config.default.as_ref()),
        (ModelTier::Reasoning, tier_config.reasoning.as_ref()),
    ]
    .into_iter()
    .filter_map(|(tier, model)| {
        model
            .filter(|model| !model.trim().is_empty())
            .map(|model| (tier, model.clone()))
    })
    .collect()
}

fn reference_runtime_target(agent_type: &AgentType, connection_id: Option<&str>) -> String {
    connection_id
        .map(db::http_runtime_target_id)
        .unwrap_or_else(|| db::agent_runtime_target_id(agent_type))
}

/// Attach warnings for persisted references to models that disappeared from
/// the latest successful catalogue. The warning is recalculated from current
/// workflow, tier and Quick Prompt state, so fixing the last reference clears
/// it without mutating catalogue history.
pub async fn populate_reference_alerts(
    database: &Database,
    config: &AppConfig,
    view: &mut ModelCatalogView,
) -> anyhow::Result<()> {
    if !view.models.iter().any(|entry| {
        entry.availability == ModelAvailability::Unavailable
            && entry.unavailable_reason == Some(ModelUnavailableReason::Disappeared)
    }) {
        view.alerts.clear();
        return Ok(());
    }
    let (workflows, quick_prompts) = database
        .with_read_conn(|conn| {
            Ok((
                crate::db::workflows::list_workflows(conn)?,
                crate::db::quick_prompts::list_quick_prompts(conn)?,
            ))
        })
        .await?;
    let tier_models = configured_tier_models(&config.agents.model_tiers, &view.agent_type);
    let mut alerts = Vec::new();

    for missing in view.models.iter().filter(|entry| {
        entry.availability == ModelAvailability::Unavailable
            && entry.unavailable_reason == Some(ModelUnavailableReason::Disappeared)
    }) {
        let mut references = Vec::new();
        for workflow in &workflows {
            for step in workflow.steps.iter().chain(&workflow.on_failure) {
                if !matches!(step.step_type, StepType::Agent | StepType::BatchQuickPrompt) {
                    continue;
                }
                let Some(settings) = step.agent_settings.as_ref() else {
                    continue;
                };
                if settings.model.as_deref() != Some(missing.model_id.as_str())
                    || reference_runtime_target(&step.agent, settings.connection_id.as_deref())
                        != view.runtime_target_id
                {
                    continue;
                }
                references.push(ModelCatalogReference {
                    kind: ModelCatalogReferenceKind::WorkflowStep,
                    resource_id: Some(workflow.id.clone()),
                    label: format!("{} · {}", workflow.name, step.name),
                });
            }
        }
        if view.runtime_target_id == db::agent_runtime_target_id(&view.agent_type) {
            for (tier, model) in &tier_models {
                if model == &missing.model_id {
                    references.push(ModelCatalogReference {
                        kind: ModelCatalogReferenceKind::ModelTier,
                        resource_id: None,
                        label: format!("{:?} · {tier:?}", view.agent_type),
                    });
                }
            }
        }
        for prompt in &quick_prompts {
            let Some(settings) = prompt.agent_settings.as_ref() else {
                continue;
            };
            let connection_id = settings
                .connection_id
                .as_deref()
                .or(prompt.connection_id.as_deref());
            if settings.model.as_deref() != Some(missing.model_id.as_str())
                || reference_runtime_target(&prompt.agent, connection_id) != view.runtime_target_id
            {
                continue;
            }
            references.push(ModelCatalogReference {
                kind: ModelCatalogReferenceKind::QuickPrompt,
                resource_id: Some(prompt.id.clone()),
                label: prompt.name.clone(),
            });
        }
        references.sort_by(|left, right| left.label.cmp(&right.label));
        if !references.is_empty() {
            alerts.push(ModelCatalogAlert {
                model_id: missing.model_id.clone(),
                replacement: live_replacement_for_entry(&view.models, missing),
                references,
            });
        }
    }
    alerts.sort_by(|left, right| left.model_id.cmp(&right.model_id));
    view.alerts = alerts;
    Ok(())
}

/// Serve the current snapshot, refreshing first when it is stale (or when
/// `force` requests an explicit recheck). Ollama only probes its HTTP endpoint
/// on a forced recheck; ordinary reads/preflight use its durable rows. Named
/// HTTP connections are refreshed by their connection-test boundary instead.
pub async fn refresh_if_stale(
    db: &Database,
    agent_type: AgentType,
    force: bool,
) -> anyhow::Result<ModelCatalogView> {
    let runtime_target_id = db::agent_runtime_target_id(&agent_type);
    if agent_type == AgentType::Ollama && force {
        let base = crate::api::ollama::resolve_base_url_pub(None);
        return refresh_ollama_catalog_at(db, &base)
            .await
            .map(|(view, _)| view);
    }
    if !is_catalog_managed(&agent_type) {
        return build_view(db, runtime_target_id, agent_type).await;
    }
    let requested_at = Utc::now();
    let _refresh_guard = db.lock_catalog_refresh(&runtime_target_id).await?;
    let target = runtime_target_id.clone();
    let log = db
        .with_conn(move |conn| db::get_refresh_log(conn, &target))
        .await?;
    let needs_refresh = match log {
        None => true,
        Some(log) => {
            log.last_attempt_at < requested_at
                && (force
                    || Utc::now()
                        .signed_duration_since(log.last_attempt_at)
                        .to_std()
                        .unwrap_or_default()
                        > LIVE_CATALOG_TTL)
        }
    };
    if needs_refresh {
        discover_and_reconcile(db, agent_type).await
    } else {
        build_view(db, runtime_target_id, agent_type).await
    }
}

fn recommended_action_for(reason: ModelUnavailableReason) -> &'static str {
    match reason {
        ModelUnavailableReason::CliMissing => "install_cli",
        ModelUnavailableReason::AuthRequired => "authenticate",
        ModelUnavailableReason::Timeout | ModelUnavailableReason::ProviderError => {
            "recheck_catalog"
        }
        ModelUnavailableReason::Disappeared => "choose_replacement",
        // The proxy lists it but refuses to serve it: only the operator can
        // pick another model (or fix the proxy), Kronn must not substitute one.
        ModelUnavailableReason::NotFound | ModelUnavailableReason::AccessDenied => {
            "choose_another_model"
        }
        ModelUnavailableReason::InvalidCatalog | ModelUnavailableReason::Unsupported => {
            "configure_manual_model"
        }
    }
}

/// Catalog-driven proactive preflight for one launch target. A disappeared
/// identity resolves to a same-agent replacement when possible, while every
/// other positive incompatibility remains a refusal. Unknown identities still
/// pass through unchanged. HTTP targets read their own durable identity but
/// never trigger CLI discovery here; reachability remains owned by their
/// transport preflight.
pub async fn preflight_resolve(
    db: &Database,
    runtime_target_id: Option<&str>,
    agent_type: AgentType,
    tier: ModelTier,
    model_override: Option<&str>,
    model_tiers: Option<&ModelTiersConfig>,
) -> Result<CatalogPreflightResolution, Box<CatalogPreflightFailure>> {
    let configured_model = model_override
        .filter(|model| !model.trim().is_empty())
        .map(str::to_string)
        .or_else(|| crate::agents::runner::configured_model_flag(&agent_type, tier, model_tiers));
    let runtime_target_id = runtime_target_id
        .map(str::to_string)
        .unwrap_or_else(|| db::agent_runtime_target_id(&agent_type));

    // Resolve from durable assignments, including unavailable ones. The hot
    // execution cache intentionally excludes them; using it here could hide a
    // disappeared tier behind an available HTTP Default (or a CLI default).
    let model_id = if let Some(model) = configured_model {
        model
    } else {
        let target = runtime_target_id.clone();
        let http_default = crate::agents::runner::is_http_chat_agent(&agent_type);
        let result = db
            .with_read_conn(move |conn| {
                let entries = db::list_for_target(conn, &target)?;
                Ok(entries
                    .iter()
                    .find(|entry| entry.tier_assignment == Some(tier))
                    .or_else(|| {
                        http_default
                            .then(|| {
                                entries
                                    .iter()
                                    .find(|entry| entry.tier_assignment == Some(ModelTier::Default))
                            })
                            .flatten()
                    })
                    .map(|entry| entry.model_id.clone()))
            })
            .await;
        match result {
            Ok(Some(model)) => model,
            Ok(None) => {
                return Ok(CatalogPreflightResolution {
                    requested_model: None,
                    effective_model: None,
                    warning: None,
                })
            }
            Err(error) => {
                return Err(Box::new(CatalogPreflightFailure {
                    runtime_target_id,
                    agent_type,
                    model_id: None,
                    reason: ModelUnavailableReason::ProviderError,
                    detail: format!("catalog assignment lookup failed: {error}"),
                    last_checked_at: Utc::now(),
                    recommended_action: "recheck_catalog".into(),
                    replacement: None,
                }))
            }
        }
    };

    // A launch is a freshness trigger, not a blind read. CLI discovery is
    // bounded by `DISCOVERY_TIMEOUT`; named HTTP targets consume the latest
    // bounded connection-test result because credentials remain owned by the
    // external-connection subsystem.
    let refresh_view = if is_catalog_managed(&agent_type)
        && runtime_target_id == db::agent_runtime_target_id(&agent_type)
    {
        match refresh_if_stale(db, agent_type.clone(), false).await {
            Ok(view) => Some(view),
            Err(error) => {
                return Err(Box::new(CatalogPreflightFailure {
                    runtime_target_id,
                    agent_type,
                    model_id: Some(model_id.clone()),
                    reason: ModelUnavailableReason::ProviderError,
                    detail: format!("catalog refresh failed: {error}"),
                    last_checked_at: Utc::now(),
                    recommended_action: "recheck_catalog".into(),
                    replacement: None,
                }));
            }
        }
    } else if runtime_target_id.starts_with("http:") {
        build_view(db, runtime_target_id.clone(), agent_type.clone())
            .await
            .ok()
    } else {
        None
    };

    if let Some(view) = refresh_view {
        if let Some(reason) = view.last_error_reason.filter(|reason| {
            // Claude discovery is not an account-access check. A transient
            // probe failure does not invalidate an exact model still recorded
            // Available; keep the error visible and let execution authenticate.
            // Missing CLI/auth, unknown or disappeared identities and other
            // runtimes retain their existing refusal policy.
            let retained_claude_model = agent_type == AgentType::ClaudeCode
                && matches!(
                    reason,
                    ModelUnavailableReason::Timeout | ModelUnavailableReason::ProviderError
                )
                && view.models.iter().any(|entry| {
                    entry.runtime_target_id == runtime_target_id
                        && entry.model_id == model_id
                        && entry.availability == ModelAvailability::Available
                });
            !matches!(reason, ModelUnavailableReason::Unsupported) && !retained_claude_model
        }) {
            return Err(Box::new(CatalogPreflightFailure {
                runtime_target_id,
                agent_type,
                model_id: Some(model_id.clone()),
                reason,
                detail: view
                    .last_error_detail
                    .unwrap_or_else(|| "the runtime catalog could not be refreshed".into()),
                last_checked_at: view.last_attempt_at.unwrap_or_else(Utc::now),
                recommended_action: recommended_action_for(reason).to_string(),
                replacement: None,
            }));
        }
    }
    let target = runtime_target_id.clone();
    let mid = model_id.clone();
    let (entry, entries) = db
        .with_read_conn(move |conn| {
            let entries = db::list_for_target(conn, &target)?;
            let entry = entries.iter().find(|entry| entry.model_id == mid).cloned();
            Ok((entry, entries))
        })
        .await
        .unwrap_or((None, Vec::new()));
    match entry {
        Some(entry) if entry.availability == ModelAvailability::Unavailable => {
            let reason = entry
                .unavailable_reason
                .unwrap_or(ModelUnavailableReason::Disappeared);
            let equivalent_tier = entry.tier_assignment.unwrap_or(tier);
            let replacement = (reason == ModelUnavailableReason::Disappeared)
                .then(|| live_replacement_for_entry(&entries, &entry))
                .flatten()
                .filter(|candidate| available_chat_entry(&entries, candidate).is_some())
                .map(|model| {
                    (
                        model,
                        CatalogReplacementSource::ResolvedModel,
                        equivalent_tier,
                    )
                })
                .or_else(|| {
                    (reason == ModelUnavailableReason::Disappeared)
                        .then(|| {
                            tier_replacement_for_entry(
                                &entries,
                                &entry,
                                &agent_type,
                                tier,
                                model_tiers,
                            )
                        })
                        .flatten()
                        .map(|(model, tier)| {
                            (model, CatalogReplacementSource::EquivalentTier, tier)
                        })
                });
            if let Some((effective_model, replacement_source, equivalent_tier)) = replacement {
                return Ok(CatalogPreflightResolution {
                    requested_model: Some(model_id.clone()),
                    effective_model: Some(effective_model.clone()),
                    warning: Some(CatalogPreflightWarning {
                        requested_model: model_id,
                        effective_model,
                        reason,
                        detail: entry.unavailable_detail.unwrap_or_else(|| {
                            "this model disappeared from the latest live catalogue".into()
                        }),
                        replacement_source,
                        equivalent_tier,
                    }),
                });
            }
            let detail = if reason == ModelUnavailableReason::Disappeared {
                format!(
                    "model `{model_id}` disappeared and no available replacement exists for the {:?} tier on agent {:?}",
                    equivalent_tier, agent_type
                )
            } else {
                entry
                    .unavailable_detail
                    .unwrap_or_else(|| "this model is not currently available".into())
            };
            Err(Box::new(CatalogPreflightFailure {
                runtime_target_id,
                agent_type,
                model_id: Some(model_id),
                reason,
                detail,
                last_checked_at: entry.last_checked_at,
                recommended_action: recommended_action_for(reason).to_string(),
                replacement: None,
            }))
        }
        // KT-545 DoD #3: a model the catalog positively tags with
        // capabilities that exclude chat (e.g. an image/video-only entry)
        // is refused here, before any request reaches the provider — never
        // just silently sent to the wrong endpoint. An entry with no
        // recorded capabilities is unaffected (see `entry_supports_capability`).
        Some(entry)
            if !crate::http_transport::entry_supports_capability(
                &entry,
                crate::http_transport::CAPABILITY_CHAT,
            ) =>
        {
            Err(Box::new(CatalogPreflightFailure {
                runtime_target_id,
                agent_type,
                model_id: Some(model_id),
                reason: ModelUnavailableReason::Unsupported,
                detail: format!(
                    "model `{}` does not support chat (catalog capabilities: {})",
                    entry.model_id,
                    entry.capabilities.join(", ")
                ),
                last_checked_at: entry.last_checked_at,
                recommended_action: recommended_action_for(ModelUnavailableReason::Unsupported)
                    .to_string(),
                replacement: None,
            }))
        }
        _ => Ok(CatalogPreflightResolution {
            requested_model: Some(model_id.clone()),
            effective_model: Some(model_id),
            warning: None,
        }),
    }
}

/// Compatibility helper for launch surfaces that only need a blocking
/// verdict. Callers that dispatch a model must use [`preflight_resolve`] so a
/// non-blocking replacement reaches the actual runtime.
pub async fn preflight_check(
    db: &Database,
    runtime_target_id: Option<&str>,
    agent_type: AgentType,
    tier: ModelTier,
    model_override: Option<&str>,
    model_tiers: Option<&ModelTiersConfig>,
) -> Option<CatalogPreflightFailure> {
    preflight_resolve(
        db,
        runtime_target_id,
        agent_type,
        tier,
        model_override,
        model_tiers,
    )
    .await
    .err()
    .map(|failure| *failure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model_catalog as db;
    use crate::models::{ModelProvenance, UpsertManualModelRequest};

    fn test_db() -> Database {
        Database::open_in_memory().unwrap()
    }

    fn fable() -> DiscoveredModel {
        DiscoveredModel {
            model_id: "claude-fable-5-1[1m]".into(),
            display_name: "Fable".into(),
            resolved_model: Some("claude-fable-5-1".into()),
            description: Some("Long-context Fable".into()),
            capabilities: vec!["chat".into()],
            reasoning_modes: vec!["low".into(), "max".into()],
            default_reasoning_mode: None,
        }
    }

    #[tokio::test]
    async fn overlapping_forced_refreshes_share_one_attempt_but_an_explicit_later_recheck_runs() {
        let db = test_db();
        TEST_DISCOVERY_CALLS
            .scope(
                std::cell::Cell::new(0),
                TEST_DISCOVERY.scope(DiscoveryOutcome::Live(vec![fable()]), async {
                    let guard = db.lock_catalog_refresh("agent:claude-code").await.unwrap();
                    let mut left =
                        std::pin::pin!(refresh_if_stale(&db, AgentType::ClaudeCode, true));
                    let mut right =
                        std::pin::pin!(refresh_if_stale(&db, AgentType::ClaudeCode, true));
                    assert!(futures::poll!(left.as_mut()).is_pending());
                    assert!(futures::poll!(right.as_mut()).is_pending());
                    drop(guard);
                    let (left, right) = tokio::join!(left, right);
                    assert_eq!(left.unwrap().models[0].model_id, fable().model_id);
                    assert_eq!(right.unwrap().models[0].model_id, fable().model_id);
                    assert_eq!(TEST_DISCOVERY_CALLS.with(std::cell::Cell::get), 1);
                    refresh_if_stale(&db, AgentType::ClaudeCode, false)
                        .await
                        .unwrap();
                    assert_eq!(
                        TEST_DISCOVERY_CALLS.with(std::cell::Cell::get),
                        1,
                        "fresh cache is reused"
                    );
                    refresh_if_stale(&db, AgentType::ClaudeCode, true)
                        .await
                        .unwrap();
                    assert_eq!(
                        TEST_DISCOVERY_CALLS.with(std::cell::Cell::get),
                        2,
                        "later force is not a cached promise"
                    );
                }),
            )
            .await;
    }

    #[tokio::test]
    async fn claude_refresh_preserves_manual_tiers_http_namespace_and_last_good_on_failure() {
        let database = test_db();
        database
            .with_conn(|conn| {
                db::insert_migrated_seed(
                    conn,
                    &AgentType::ClaudeCode,
                    "opus",
                    "My Opus",
                    Some(ModelTier::Reasoning),
                    &["chat".into()],
                    &[],
                )?;
                db::reconcile_live(
                    conn,
                    "http:claude-api",
                    &AgentType::ClaudeCode,
                    &[DiscoveredModel {
                        model_id: "claude-fable-5-1".into(),
                        ..fable()
                    }],
                )
            })
            .await
            .unwrap();
        let live = TEST_DISCOVERY
            .scope(
                DiscoveryOutcome::Live(vec![fable()]),
                refresh_if_stale(&database, AgentType::ClaudeCode, true),
            )
            .await
            .unwrap();
        assert!(live.live_refresh_ok);
        assert_eq!(
            live.models
                .iter()
                .find(|model| model.model_id == "opus")
                .unwrap()
                .tier_assignment,
            Some(ModelTier::Reasoning)
        );
        assert_eq!(
            live.models
                .iter()
                .find(|model| model.model_id == fable().model_id)
                .unwrap()
                .tier_assignment,
            None
        );
        let failed = TEST_DISCOVERY
            .scope(
                DiscoveryOutcome::Timeout,
                refresh_if_stale(&database, AgentType::ClaudeCode, true),
            )
            .await
            .unwrap();
        assert!(!failed.live_refresh_ok);
        assert_eq!(
            failed.last_error_reason,
            Some(ModelUnavailableReason::Timeout)
        );
        assert_eq!(failed.last_live_success_at, live.last_live_success_at);
        assert_eq!(
            failed
                .models
                .iter()
                .find(|model| model.model_id == fable().model_id)
                .unwrap()
                .availability,
            ModelAvailability::Available
        );
        let cached = TEST_DISCOVERY
            .scope(
                DiscoveryOutcome::Live(vec![]),
                refresh_if_stale(&database, AgentType::ClaudeCode, false),
            )
            .await
            .unwrap();
        assert_eq!(
            cached.last_attempt_at, failed.last_attempt_at,
            "failed attempts have a retry cooldown"
        );
        let absent = TEST_DISCOVERY
            .scope(
                DiscoveryOutcome::Live(vec![]),
                refresh_if_stale(&database, AgentType::ClaudeCode, true),
            )
            .await
            .unwrap();
        assert_eq!(
            absent
                .models
                .iter()
                .find(|model| model.model_id == fable().model_id)
                .unwrap()
                .availability,
            ModelAvailability::Unavailable
        );
        assert_eq!(
            absent
                .models
                .iter()
                .find(|model| model.model_id == "opus")
                .unwrap()
                .availability,
            ModelAvailability::Available
        );
        let returned = TEST_DISCOVERY
            .scope(
                DiscoveryOutcome::Live(vec![fable()]),
                refresh_if_stale(&database, AgentType::ClaudeCode, true),
            )
            .await
            .unwrap();
        assert_eq!(
            returned
                .models
                .iter()
                .find(|model| model.model_id == fable().model_id)
                .unwrap()
                .availability,
            ModelAvailability::Available
        );
        let http = build_view(&database, "http:claude-api".into(), AgentType::ClaudeCode)
            .await
            .unwrap();
        assert_eq!(http.models.len(), 1);
        assert_eq!(http.models[0].model_id, "claude-fable-5-1");
        assert!(http.live_refresh_ok);
    }

    #[test]
    fn is_catalog_managed_covers_exactly_the_dod_runtimes() {
        for agent in [
            AgentType::ClaudeCode,
            AgentType::Codex,
            AgentType::OpenCode,
            AgentType::GeminiCli,
            AgentType::Kiro,
            AgentType::CopilotCli,
            AgentType::Vibe,
        ] {
            assert!(
                is_catalog_managed(&agent),
                "{agent:?} must be catalog-managed"
            );
        }
        for agent in [
            AgentType::Ollama,
            AgentType::LiteLlm,
            AgentType::Nvidia,
            AgentType::Custom,
        ] {
            assert!(
                !is_catalog_managed(&agent),
                "{agent:?} must stay out of scope"
            );
        }
    }

    /// KT-543 — a model discovered over ACP feeds the tiers, and its
    /// disappearance takes the tier with it rather than serving a stale id.
    #[tokio::test]
    async fn a_tier_follows_its_model_out_of_the_catalogue_and_back() {
        let db = test_db();
        let target = db::agent_runtime_target_id(&AgentType::OpenCode);
        let request = UpsertManualModelRequest {
            runtime_target_id: target.clone(),
            agent_type: AgentType::OpenCode,
            model_id: "zen/coder".into(),
            display_name: "Zen Coder".into(),
            capabilities: vec!["chat".into()],
            reasoning_modes: vec!["high".into()],
            default_reasoning_mode: Some("high".into()),
            tier_assignment: Some(ModelTier::Reasoning),
            cost_hint: None,
            privacy_note: None,
        };
        db.with_conn(move |conn| {
            db::create_manual(conn, &request)?;
            Ok(())
        })
        .await
        .unwrap();
        refresh_runtime_cache(&db).await.unwrap();
        assert_eq!(
            assigned_model_for_agent(&AgentType::OpenCode, ModelTier::Reasoning).as_deref(),
            Some("zen/coder"),
        );

        // The provider stops listing it. The row is NEVER deleted — the
        // operator's assignment and the audit trail survive — but the tier
        // must stop resolving, because dispatching a model the runtime no
        // longer serves fails after the request left.
        let gone = target.clone();
        db.with_conn(move |conn| {
            db::mark_unavailable(
                conn,
                &gone,
                "zen/coder",
                ModelUnavailableReason::Unsupported,
                Some("absent from the live catalogue"),
            )?;
            Ok(())
        })
        .await
        .unwrap();
        refresh_runtime_cache(&db).await.unwrap();
        assert_eq!(
            assigned_model_for_agent(&AgentType::OpenCode, ModelTier::Reasoning),
            None,
            "an unavailable model must not keep answering for its tier",
        );

        // Reappearing under the same canonical identity restores the tier by
        // itself: the assignment was never lost, only suspended.
        let back = target.clone();
        db.with_conn(move |conn| {
            db::mark_available(conn, &back, "zen/coder")?;
            Ok(())
        })
        .await
        .unwrap();
        refresh_runtime_cache(&db).await.unwrap();
        assert_eq!(
            assigned_model_for_agent(&AgentType::OpenCode, ModelTier::Reasoning).as_deref(),
            Some("zen/coder"),
        );
    }

    #[tokio::test]
    async fn preflight_check_passes_for_out_of_scope_runtime() {
        let db = test_db();
        let failure =
            preflight_check(&db, None, AgentType::Ollama, ModelTier::Default, None, None).await;
        assert!(failure.is_none());
    }

    #[tokio::test]
    async fn preflight_check_passes_when_identity_unknown() {
        let db = test_db();
        db.with_conn(|conn| {
            db::reconcile_live(
                conn,
                &db::agent_runtime_target_id(&AgentType::Codex),
                &AgentType::Codex,
                &[DiscoveredModel {
                    model_id: "some-other-model".into(),
                    display_name: "Some other model".into(),
                    resolved_model: None,
                    description: None,
                    capabilities: vec!["chat".into()],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                }],
            )
        })
        .await
        .unwrap();
        let failure =
            preflight_check(&db, None, AgentType::Codex, ModelTier::Economy, None, None).await;
        // No configured or assigned identity is not an unavailable identity.
        assert!(failure.is_none());
    }

    #[tokio::test]
    async fn preflight_of_a_never_checked_target_performs_a_bounded_refresh() {
        let db = test_db();
        let target = db::agent_runtime_target_id(&AgentType::ClaudeCode);
        assert!(db
            .with_conn({
                let target = target.clone();
                move |conn| db::get_refresh_log(conn, &target)
            })
            .await
            .unwrap()
            .is_none());

        // For an explicitly selected model, preflight must record the bounded
        // discovery decision even when this runtime has no live discovery.
        let failure = TEST_DISCOVERY
            .scope(
                DiscoveryOutcome::Unsupported,
                preflight_check(
                    &db,
                    None,
                    AgentType::ClaudeCode,
                    ModelTier::Default,
                    Some("operator-claude-model"),
                    None,
                ),
            )
            .await;
        assert!(
            failure.is_none(),
            "unsupported live discovery keeps fallbacks usable"
        );
        let log = db
            .with_conn(move |conn| db::get_refresh_log(conn, &target))
            .await
            .unwrap()
            .expect("preflight must record its refresh attempt");
        assert_eq!(
            log.last_error_reason,
            Some(ModelUnavailableReason::Unsupported)
        );
    }

    #[tokio::test]
    async fn preflight_check_blocks_chat_incompatible_model() {
        // KT-545 DoD #3: a named HTTP connection whose catalog marks a model
        // video/image-only must never reach the chat codec.
        let db = test_db();
        db.with_conn(|conn| {
            db::reconcile_live(
                conn,
                "http:connection-a",
                &AgentType::Custom,
                &[DiscoveredModel {
                    model_id: "seedance-2.0-mini".into(),
                    display_name: "Seedance 2.0 mini".into(),
                    resolved_model: None,
                    description: None,
                    capabilities: vec!["video".into()],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                }],
            )
        })
        .await
        .unwrap();

        let failure = preflight_check(
            &db,
            Some("http:connection-a"),
            AgentType::Custom,
            ModelTier::Default,
            Some("seedance-2.0-mini"),
            None,
        )
        .await
        .expect("a video-only catalog entry must refuse a chat launch");
        assert_eq!(failure.reason, ModelUnavailableReason::Unsupported);
        assert_eq!(failure.recommended_action, "configure_manual_model");
        assert!(failure.detail.contains("does not support chat"));
        assert!(failure.detail.contains("video"));
    }

    #[tokio::test]
    async fn preflight_check_passes_chat_tagged_model_on_a_connection() {
        let db = test_db();
        db.with_conn(|conn| {
            db::reconcile_live(
                conn,
                "http:connection-a",
                &AgentType::Custom,
                &[DiscoveredModel {
                    model_id: "llama-3.3-70b".into(),
                    display_name: "Llama 3.3 70B".into(),
                    resolved_model: None,
                    description: None,
                    capabilities: vec!["chat".into()],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                }],
            )
        })
        .await
        .unwrap();

        let failure = preflight_check(
            &db,
            Some("http:connection-a"),
            AgentType::Custom,
            ModelTier::Default,
            Some("llama-3.3-70b"),
            None,
        )
        .await;
        assert!(failure.is_none());
    }

    #[tokio::test]
    async fn preflight_check_blocks_known_unavailable_model() {
        let db = test_db();
        db.with_conn(|conn| {
            db::insert_migrated_seed(
                conn,
                &AgentType::Codex,
                "gpt-5.6-luna",
                "GPT-5.6 Luna",
                Some(ModelTier::Economy),
                &[],
                &[],
            )?;
            db::mark_unavailable(
                conn,
                &db::agent_runtime_target_id(&AgentType::Codex),
                "gpt-5.6-luna",
                ModelUnavailableReason::Disappeared,
                Some("absent from the last live catalogue"),
            )?;
            db::reconcile_live(
                conn,
                &db::agent_runtime_target_id(&AgentType::Codex),
                &AgentType::Codex,
                &[DiscoveredModel {
                    model_id: "other-model".into(),
                    display_name: "Other model".into(),
                    resolved_model: None,
                    description: None,
                    capabilities: vec![],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                }],
            )?;
            db::mark_unavailable(
                conn,
                &db::agent_runtime_target_id(&AgentType::Codex),
                "gpt-5.6-luna",
                ModelUnavailableReason::Disappeared,
                Some("absent from the last live catalogue"),
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let failure = preflight_check(&db, None, AgentType::Codex, ModelTier::Economy, None, None)
            .await
            .expect("known-unavailable model must fail preflight");
        assert_eq!(failure.model_id.as_deref(), Some("gpt-5.6-luna"));
        assert_eq!(failure.reason, ModelUnavailableReason::Disappeared);
        assert_eq!(failure.recommended_action, "choose_replacement");
    }

    #[tokio::test]
    async fn a_listed_model_the_proxy_refuses_is_refused_not_swapped_for_another() {
        // KT-941 — `vertex_ai/claude-sonnet-5` is listed but answers 404 (or is
        // denied by the proxy's tags). A healthy model sits right next to it, and
        // the launch must still stop and name the refused one: choosing for the
        // operator is a silent substitution.
        for reason in [
            ModelUnavailableReason::NotFound,
            ModelUnavailableReason::AccessDenied,
        ] {
            let db = test_db();
            db.with_conn(move |conn| {
                let listed = |id: &str| DiscoveredModel {
                    model_id: id.into(),
                    display_name: id.into(),
                    resolved_model: None,
                    description: None,
                    capabilities: vec![],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                };
                db::reconcile_live(
                    conn,
                    "http:connection-a",
                    &AgentType::LiteLlm,
                    &[
                        listed("vertex_ai/claude-sonnet-5"),
                        listed("claude-sonnet-4-6"),
                    ],
                )?;
                db::mark_unavailable(
                    conn,
                    "http:connection-a",
                    "vertex_ai/claude-sonnet-5",
                    reason,
                    Some("Not found or access denied (HTTP 404)"),
                )?;
                Ok(())
            })
            .await
            .unwrap();

            let failure = preflight_check(
                &db,
                Some("http:connection-a"),
                AgentType::LiteLlm,
                ModelTier::Default,
                Some("vertex_ai/claude-sonnet-5"),
                None,
            )
            .await
            .expect("a model the proxy refuses must fail preflight");
            assert_eq!(
                failure.model_id.as_deref(),
                Some("vertex_ai/claude-sonnet-5")
            );
            assert_eq!(failure.reason, reason);
            assert_eq!(failure.recommended_action, "choose_another_model");
            assert!(failure.replacement.is_none(), "{failure:?}");

            // The healthy sibling still launches when it is the one asked for.
            assert!(preflight_check(
                &db,
                Some("http:connection-a"),
                AgentType::LiteLlm,
                ModelTier::Default,
                Some("claude-sonnet-4-6"),
                None,
            )
            .await
            .is_none());
        }
    }

    #[tokio::test]
    async fn disappeared_claude_alias_names_live_resolved_model_replacement() {
        let database = test_db();
        database
            .with_conn(|conn| {
                let target = db::agent_runtime_target_id(&AgentType::ClaudeCode);
                let alias = DiscoveredModel {
                    model_id: "opus[1m]".into(),
                    display_name: "Opus 1M".into(),
                    resolved_model: Some("claude-opus-5-5".into()),
                    description: Some("Legacy long-context alias".into()),
                    capabilities: vec!["chat".into()],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                };
                let live = DiscoveredModel {
                    model_id: "opus".into(),
                    display_name: "Opus".into(),
                    resolved_model: Some("claude-opus-5-5".into()),
                    description: Some("Current Opus alias".into()),
                    capabilities: vec!["chat".into()],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                };
                let default = DiscoveredModel {
                    model_id: "default".into(),
                    display_name: "Default".into(),
                    resolved_model: Some("claude-opus-5-5".into()),
                    description: Some("Recommended model".into()),
                    capabilities: vec!["chat".into()],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                };
                db::reconcile_live(
                    conn,
                    &target,
                    &AgentType::ClaudeCode,
                    &[alias, default.clone(), live.clone()],
                )?;
                db::reconcile_live(conn, &target, &AgentType::ClaudeCode, &[default, live])
            })
            .await
            .unwrap();

        let resolution = preflight_resolve(
            &database,
            None,
            AgentType::ClaudeCode,
            ModelTier::Reasoning,
            Some("opus[1m]"),
            None,
        )
        .await
        .expect("the disappeared alias has a same-agent live replacement");
        assert_eq!(resolution.requested_model.as_deref(), Some("opus[1m]"));
        assert_eq!(resolution.effective_model.as_deref(), Some("opus"));
        let warning = resolution.warning.expect("the replacement is never silent");
        assert_eq!(warning.reason, ModelUnavailableReason::Disappeared);
        assert_eq!(warning.requested_model, "opus[1m]");
        assert_eq!(warning.effective_model, "opus");
        assert_eq!(
            warning.replacement_source,
            CatalogReplacementSource::ResolvedModel
        );
        assert_eq!(warning.equivalent_tier, ModelTier::Reasoning);
    }

    #[tokio::test]
    async fn disappeared_model_without_resolved_identity_uses_equivalent_tier_default() {
        let database = test_db();
        database
            .with_conn(|conn| {
                let target = db::agent_runtime_target_id(&AgentType::ClaudeCode);
                let missing = DiscoveredModel {
                    model_id: "retired-reasoner".into(),
                    display_name: "Retired reasoner".into(),
                    resolved_model: None,
                    description: None,
                    capabilities: vec!["chat".into()],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                };
                let live = DiscoveredModel {
                    model_id: "opus".into(),
                    display_name: "Opus".into(),
                    resolved_model: None,
                    description: None,
                    capabilities: vec!["chat".into()],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                };
                db::reconcile_live(
                    conn,
                    &target,
                    &AgentType::ClaudeCode,
                    &[missing, live.clone()],
                )?;
                db::reconcile_live(conn, &target, &AgentType::ClaudeCode, &[live])
            })
            .await
            .unwrap();
        let mut tiers = ModelTiersConfig::default();
        tiers.claude_code.reasoning = Some("opus".into());

        let resolution = preflight_resolve(
            &database,
            None,
            AgentType::ClaudeCode,
            ModelTier::Reasoning,
            Some("retired-reasoner"),
            Some(&tiers),
        )
        .await
        .expect("the live equivalent-tier default replaces the missing model");
        assert_eq!(resolution.effective_model.as_deref(), Some("opus"));
        let warning = resolution.warning.unwrap();
        assert_eq!(
            warning.replacement_source,
            CatalogReplacementSource::EquivalentTier
        );
        assert_eq!(warning.equivalent_tier, ModelTier::Reasoning);
    }

    #[tokio::test]
    async fn disappeared_model_without_tier_config_uses_live_agent_default() {
        let database = test_db();
        database
            .with_conn(|conn| {
                let target = db::agent_runtime_target_id(&AgentType::ClaudeCode);
                let missing = DiscoveredModel {
                    model_id: "retired-reasoner".into(),
                    display_name: "Retired reasoner".into(),
                    resolved_model: None,
                    description: None,
                    capabilities: vec!["chat".into()],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                };
                let live_default = DiscoveredModel {
                    model_id: "opus".into(),
                    display_name: "Opus".into(),
                    resolved_model: None,
                    description: None,
                    capabilities: vec!["chat".into()],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                };
                db::reconcile_live(
                    conn,
                    &target,
                    &AgentType::ClaudeCode,
                    &[missing, live_default.clone()],
                )?;
                db::reconcile_live(conn, &target, &AgentType::ClaudeCode, &[live_default])
            })
            .await
            .unwrap();

        let resolution = preflight_resolve(
            &database,
            None,
            AgentType::ClaudeCode,
            ModelTier::Reasoning,
            Some("retired-reasoner"),
            None,
        )
        .await
        .expect("the live built-in tier default replaces the missing model");
        assert_eq!(resolution.effective_model.as_deref(), Some("opus"));
        let warning = resolution.warning.unwrap();
        assert_eq!(
            warning.replacement_source,
            CatalogReplacementSource::EquivalentTier
        );
        assert_eq!(warning.equivalent_tier, ModelTier::Reasoning);
    }

    #[tokio::test]
    async fn disappeared_model_without_same_agent_tier_replacement_is_explicitly_refused() {
        let database = test_db();
        database
            .with_conn(|conn| {
                let target = db::agent_runtime_target_id(&AgentType::ClaudeCode);
                let missing = DiscoveredModel {
                    model_id: "retired-reasoner".into(),
                    display_name: "Retired reasoner".into(),
                    resolved_model: None,
                    description: None,
                    capabilities: vec!["chat".into()],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                };
                db::reconcile_live(conn, &target, &AgentType::ClaudeCode, &[missing])?;
                db::reconcile_live(conn, &target, &AgentType::ClaudeCode, &[])
            })
            .await
            .unwrap();

        let failure = preflight_resolve(
            &database,
            None,
            AgentType::ClaudeCode,
            ModelTier::Reasoning,
            Some("retired-reasoner"),
            None,
        )
        .await
        .expect_err("no other agent or unavailable model may be selected");
        assert_eq!(failure.reason, ModelUnavailableReason::Disappeared);
        assert!(failure.detail.contains("no available replacement"));
        assert!(failure.detail.contains("Reasoning"));
        assert!(failure.detail.contains("ClaudeCode"));
    }

    #[tokio::test]
    async fn available_model_passes_preflight_without_rewrite_or_warning() {
        let database = test_db();
        database
            .with_conn(|conn| {
                db::reconcile_live(
                    conn,
                    &db::agent_runtime_target_id(&AgentType::ClaudeCode),
                    &AgentType::ClaudeCode,
                    &[DiscoveredModel {
                        model_id: "opus".into(),
                        display_name: "Opus".into(),
                        resolved_model: Some("claude-opus-5-5".into()),
                        description: None,
                        capabilities: vec!["chat".into()],
                        reasoning_modes: vec![],
                        default_reasoning_mode: None,
                    }],
                )
            })
            .await
            .unwrap();

        let resolution = preflight_resolve(
            &database,
            None,
            AgentType::ClaudeCode,
            ModelTier::Reasoning,
            Some("opus"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(resolution.requested_model, resolution.effective_model);
        assert!(resolution.warning.is_none());
    }

    #[tokio::test]
    async fn refresh_alert_names_workflow_tier_and_quick_prompt_references() {
        let database = test_db();
        let target = db::agent_runtime_target_id(&AgentType::ClaudeCode);
        let alias = DiscoveredModel {
            model_id: "opus[1m]".into(),
            display_name: "Opus 1M".into(),
            resolved_model: Some("claude-opus-5-5".into()),
            description: None,
            capabilities: vec!["chat".into()],
            reasoning_modes: vec![],
            default_reasoning_mode: None,
        };
        let live = DiscoveredModel {
            model_id: "opus".into(),
            display_name: "Opus".into(),
            resolved_model: Some("claude-opus-5-5".into()),
            description: None,
            capabilities: vec!["chat".into()],
            reasoning_modes: vec![],
            default_reasoning_mode: None,
        };
        database
            .with_conn({
                let target = target.clone();
                let alias = alias.clone();
                let live = live.clone();
                move |conn| {
                    db::reconcile_live(
                        conn,
                        &target,
                        &AgentType::ClaudeCode,
                        &[alias, live],
                    )?;
                    let now = Utc::now().to_rfc3339();
                    conn.execute(
                        "INSERT INTO workflows \
                         (id, name, trigger_json, steps_json, created_at, updated_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                        rusqlite::params![
                            "workflow-release",
                            "Release workflow",
                            r#"{"type":"Manual"}"#,
                            r#"[{"name":"orchestrator","step_type":{"type":"Agent"},"agent":"ClaudeCode","prompt_template":"Ship","agent_settings":{"model":"opus[1m]"}}]"#,
                            now,
                        ],
                    )?;
                    conn.execute(
                        "INSERT INTO quick_prompts \
                         (id, name, prompt_template, agent, tier, agent_settings_json, created_at, updated_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                        rusqlite::params![
                            "qp-framing",
                            "Framing analysis",
                            "Analyse",
                            "ClaudeCode",
                            "reasoning",
                            r#"{"model":"opus[1m]"}"#,
                            now,
                        ],
                    )?;
                    Ok(())
                }
            })
            .await
            .unwrap();
        let mut config = crate::core::config::default_config();
        config.agents.model_tiers.claude_code.reasoning = Some("opus[1m]".into());

        let mut view = TEST_DISCOVERY
            .scope(
                DiscoveryOutcome::Live(vec![live]),
                refresh_if_stale(&database, AgentType::ClaudeCode, true),
            )
            .await
            .unwrap();
        populate_reference_alerts(&database, &config, &mut view)
            .await
            .unwrap();

        assert_eq!(view.alerts.len(), 1);
        let alert = &view.alerts[0];
        assert_eq!(alert.model_id, "opus[1m]");
        assert_eq!(alert.replacement.as_deref(), Some("opus"));
        assert_eq!(
            alert
                .references
                .iter()
                .map(|reference| reference.label.as_str())
                .collect::<Vec<_>>(),
            [
                "ClaudeCode · Reasoning",
                "Framing analysis",
                "Release workflow · orchestrator",
            ]
        );
    }

    #[tokio::test]
    async fn preflight_surfaces_recent_refresh_failure_even_with_cached_model() {
        let db = test_db();
        db.with_conn(|conn| {
            let target = db::agent_runtime_target_id(&AgentType::Codex);
            db::insert_migrated_seed(
                conn,
                &AgentType::Codex,
                "gpt-5.6-luna",
                "GPT-5.6 Luna",
                Some(ModelTier::Economy),
                &[],
                &[],
            )?;
            db::record_refresh_failure(
                conn,
                &target,
                &AgentType::Codex,
                ModelUnavailableReason::AuthRequired,
                "login required",
            )
        })
        .await
        .unwrap();

        let failure = preflight_check(&db, None, AgentType::Codex, ModelTier::Economy, None, None)
            .await
            .expect("a recent failed bounded refresh must block launch");
        assert_eq!(failure.reason, ModelUnavailableReason::AuthRequired);
        assert_eq!(failure.recommended_action, "authenticate");
        assert_eq!(failure.detail, "login required");
    }

    #[tokio::test]
    async fn claude_transient_discovery_failure_keeps_a_known_available_model_launchable() {
        for reason in [
            ModelUnavailableReason::Timeout,
            ModelUnavailableReason::ProviderError,
        ] {
            let database = test_db();
            database
                .with_conn(move |conn| {
                    let target = db::agent_runtime_target_id(&AgentType::ClaudeCode);
                    db::insert_migrated_seed(
                        conn,
                        &AgentType::ClaudeCode,
                        &fable().model_id,
                        "Fable",
                        Some(ModelTier::Reasoning),
                        &["chat".into()],
                        &[],
                    )?;
                    db::reconcile_live(conn, &target, &AgentType::ClaudeCode, &[fable()])?;
                    db::record_refresh_failure(
                        conn,
                        &target,
                        &AgentType::ClaudeCode,
                        reason,
                        "temporary discovery failure",
                    )
                })
                .await
                .unwrap();
            for selected in [None, Some(fable().model_id)] {
                let failure = preflight_check(
                    &database,
                    None,
                    AgentType::ClaudeCode,
                    ModelTier::Reasoning,
                    selected.as_deref(),
                    None,
                )
                .await;
                assert!(
                    failure.is_none(),
                    "cached Available Claude model must survive {reason:?}: {failure:?}"
                );
            }
            let view = build_view(&database, "agent:claude-code".into(), AgentType::ClaudeCode)
                .await
                .unwrap();
            assert!(
                !view.live_refresh_ok,
                "launchability must not relabel a failed discovery as verified"
            );
            assert_eq!(view.last_error_reason, Some(reason));
            assert_eq!(view.models[0].provenance, ModelProvenance::Cached);
        }
    }

    #[tokio::test]
    async fn cached_claude_model_still_refuses_auth_missing_cli_and_unavailable_models() {
        for reason in [
            ModelUnavailableReason::AuthRequired,
            ModelUnavailableReason::CliMissing,
            ModelUnavailableReason::Timeout,
            ModelUnavailableReason::ProviderError,
        ] {
            let database = test_db();
            database
                .with_conn(move |conn| {
                    let target = db::agent_runtime_target_id(&AgentType::ClaudeCode);
                    db::reconcile_live(conn, &target, &AgentType::ClaudeCode, &[fable()])?;
                    if matches!(
                        reason,
                        ModelUnavailableReason::Timeout | ModelUnavailableReason::ProviderError
                    ) {
                        db::mark_unavailable(
                            conn,
                            &target,
                            &fable().model_id,
                            ModelUnavailableReason::Disappeared,
                            Some("removed from catalogue"),
                        )?;
                    }
                    db::record_refresh_failure(
                        conn,
                        &target,
                        &AgentType::ClaudeCode,
                        reason,
                        "discovery failed",
                    )
                })
                .await
                .unwrap();
            let failure = preflight_check(
                &database,
                None,
                AgentType::ClaudeCode,
                ModelTier::Default,
                Some(&fable().model_id),
                None,
            )
            .await
            .expect("real refusal must remain blocking");
            assert_eq!(failure.reason, reason);
            assert_eq!(failure.model_id.as_deref(), Some(fable().model_id.as_str()));
        }
    }

    #[tokio::test]
    async fn transient_catalog_failure_does_not_allow_unknown_claude_or_change_codex_policy() {
        for agent in [AgentType::ClaudeCode, AgentType::Codex] {
            for reason in [
                ModelUnavailableReason::Timeout,
                ModelUnavailableReason::ProviderError,
            ] {
                let database = test_db();
                let source_agent = agent.clone();
                database
                    .with_conn(move |conn| {
                        let target = db::agent_runtime_target_id(&source_agent);
                        if source_agent == AgentType::Codex {
                            db::reconcile_live(conn, &target, &source_agent, &[fable()])?;
                        }
                        db::record_refresh_failure(
                            conn,
                            &target,
                            &source_agent,
                            reason,
                            "discovery failed",
                        )
                    })
                    .await
                    .unwrap();
                let failure = preflight_check(
                    &database,
                    None,
                    agent.clone(),
                    ModelTier::Default,
                    Some(&fable().model_id),
                    None,
                )
                .await
                .expect("exception is only for known Available Claude entries");
                assert_eq!(failure.reason, reason);
            }
        }
    }

    #[tokio::test]
    async fn build_view_reports_stale_when_never_refreshed() {
        let db = test_db();
        let view = build_view(
            &db,
            db::agent_runtime_target_id(&AgentType::Codex),
            AgentType::Codex,
        )
        .await
        .unwrap();
        assert!(view.stale);
        assert!(!view.live_refresh_ok);
        assert!(view.models.is_empty());
    }

    fn offered(model_id: &str) -> DiscoveredModel {
        DiscoveredModel {
            model_id: model_id.into(),
            display_name: model_id.into(),
            resolved_model: None,
            description: None,
            capabilities: Vec::new(),
            reasoning_modes: Vec::new(),
            default_reasoning_mode: None,
        }
    }

    fn ids(models: &[DiscoveredModel]) -> Vec<&str> {
        models.iter().map(|model| model.model_id.as_str()).collect()
    }

    fn project_at(id: &str, name: &str, path: &std::path::Path) -> crate::models::Project {
        let now = Utc::now();
        crate::models::Project {
            id: id.into(),
            name: name.into(),
            path: path.to_string_lossy().into_owned(),
            repo_url: None,
            token_override: None,
            ai_config: crate::models::AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: crate::models::AiAuditStatus::NoTemplate,
            ai_todo_count: 0,
            tech_debt_count: 0,
            needs_docs_migration: false,
            path_exists: true,
            write_access: None,
            mcp_sync_report: None,
            default_skill_ids: vec![],
            default_profile_id: None,
            briefing_notes: None,
            linked_repos: vec![],
            workspace: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn only_projects_with_their_own_opencode_config_are_asked() {
        let root = tempfile::TempDir::new().unwrap();
        let bench = root.path().join("bench");
        std::fs::create_dir_all(&bench).unwrap();
        std::fs::write(bench.join("opencode.json"), "{}").unwrap();
        let app = root.path().join("app");
        std::fs::create_dir_all(app.join(".opencode")).unwrap();
        std::fs::write(app.join(".opencode/opencode.jsonc"), "{}").unwrap();
        let plain = root.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        // A directory that is only named like a config is not a config.
        let impostor = root.path().join("impostor");
        std::fs::create_dir_all(impostor.join("opencode.json")).unwrap();
        let gone = root.path().join("gone");

        let scopes = opencode_project_scopes(&[
            ("bench".into(), bench.clone()),
            ("app".into(), app.clone()),
            ("plain".into(), plain),
            ("impostor".into(), impostor),
            ("gone".into(), gone),
        ]);

        assert_eq!(
            scopes,
            vec![("bench".to_string(), bench), ("app".to_string(), app)]
        );
    }

    /// A model only a project offers joins the list once, says where it is
    /// offered, and everything that reaches the list came from a session
    /// response: nothing is added, and what the user-level config offers is
    /// left exactly as discovered.
    #[test]
    fn a_project_only_model_joins_the_global_list_once_and_nothing_is_invented() {
        let global = DiscoveryOutcome::Live(vec![
            offered("opencode/big-pickle"),
            offered("anthropic/claude-sonnet"),
        ]);
        let merged = merge_project_scopes(
            global,
            vec![
                (
                    "bench".into(),
                    DiscoveryOutcome::Live(vec![
                        offered("opencode/big-pickle"),
                        offered("ollama/qwen3.8:27b"),
                    ]),
                ),
                (
                    "other".into(),
                    DiscoveryOutcome::Live(vec![offered("ollama/qwen3.8:27b")]),
                ),
            ],
        );

        let DiscoveryOutcome::Live(models) = merged else {
            panic!("a live global discovery stays live");
        };
        assert_eq!(
            ids(&models),
            vec![
                "opencode/big-pickle",
                "anthropic/claude-sonnet",
                "ollama/qwen3.8:27b"
            ]
        );
        assert_eq!(models[0].description, None);
        assert_eq!(models[1].description, None);
        let note = models[2].description.as_deref().expect("scope note");
        assert!(
            note.contains("bench") && !note.contains("other"),
            "the first project that offers it is named: {note}"
        );
    }

    #[test]
    fn a_failed_project_scope_leaves_the_rest_of_the_refresh_intact() {
        let merged = merge_project_scopes(
            DiscoveryOutcome::Live(vec![offered("opencode/big-pickle")]),
            vec![
                ("broken".into(), DiscoveryOutcome::Timeout),
                (
                    "denied".into(),
                    DiscoveryOutcome::AuthRequired("login".into()),
                ),
                ("nothing".into(), DiscoveryOutcome::Unsupported),
                (
                    "bench".into(),
                    DiscoveryOutcome::Live(vec![offered("ollama/qwen3.8:27b")]),
                ),
            ],
        );

        let DiscoveryOutcome::Live(models) = merged else {
            panic!("one project's failure must not fail the refresh");
        };
        assert_eq!(
            ids(&models),
            vec!["opencode/big-pickle", "ollama/qwen3.8:27b"]
        );
    }

    #[test]
    fn a_failed_user_level_discovery_is_never_masked_by_project_results() {
        for failure in [
            DiscoveryOutcome::Timeout,
            DiscoveryOutcome::CliMissing("opencode".into()),
            DiscoveryOutcome::Unsupported,
        ] {
            let merged = merge_project_scopes(
                failure.clone(),
                vec![(
                    "bench".into(),
                    DiscoveryOutcome::Live(vec![offered("ollama/qwen3.8:27b")]),
                )],
            );
            assert_eq!(merged, failure);
        }
    }

    /// OpenCode is wired to a local Ollama through a project's own config: the
    /// catalogue must list that model with the rest, and only what OpenCode
    /// reported. Drives the whole path — project list, scope choice, merge,
    /// persistence, the view Settings reads.
    #[tokio::test]
    async fn every_model_opencode_offers_reaches_the_catalogue_local_providers_included() {
        let root = tempfile::TempDir::new().unwrap();
        let bench = root.path().join("bench");
        std::fs::create_dir_all(&bench).unwrap();
        std::fs::write(bench.join("opencode.json"), "{}").unwrap();
        let plain = root.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();

        let database = test_db();
        let (bench_project, plain_project) = (
            project_at("p-bench", "bench", &bench),
            project_at("p-plain", "plain", &plain),
        );
        database
            .with_conn(move |conn| {
                crate::db::projects::insert_project(conn, &bench_project)?;
                crate::db::projects::insert_project(conn, &plain_project)?;
                Ok(())
            })
            .await
            .unwrap();

        // What each directory's OpenCode session lists. Only `bench` has a config
        // of its own, so only `bench` may be asked.
        let asked = std::sync::Mutex::new(Vec::new());
        let bench_dir = bench.clone();
        let outcome = TEST_DISCOVERY
            .scope(
                DiscoveryOutcome::Live(vec![
                    offered("opencode/big-pickle"),
                    offered("anthropic/claude-sonnet"),
                ]),
                discover_opencode_with(&database, |dir| {
                    asked.lock().unwrap().push(dir.clone());
                    let answer = if dir == bench_dir {
                        DiscoveryOutcome::Live(vec![
                            offered("opencode/big-pickle"),
                            offered("anthropic/claude-sonnet"),
                            offered("ollama/llama3.3:70b"),
                            offered("ollama/qwen3.8:27b"),
                        ])
                    } else {
                        DiscoveryOutcome::Unsupported
                    };
                    async move { answer }
                }),
            )
            .await;
        assert_eq!(*asked.lock().unwrap(), vec![bench]);

        let DiscoveryOutcome::Live(models) = outcome else {
            panic!("a live discovery stays live");
        };
        let target = db::agent_runtime_target_id(&AgentType::OpenCode);
        let reconcile_target = target.clone();
        database
            .with_conn(move |conn| {
                db::reconcile_live(conn, &reconcile_target, &AgentType::OpenCode, &models)
            })
            .await
            .unwrap();

        let view = build_view(&database, target, AgentType::OpenCode)
            .await
            .unwrap();
        let mut listed: Vec<&str> = view.models.iter().map(|m| m.model_id.as_str()).collect();
        listed.sort_unstable();
        assert_eq!(
            listed,
            vec![
                "anthropic/claude-sonnet",
                "ollama/llama3.3:70b",
                "ollama/qwen3.8:27b",
                "opencode/big-pickle",
            ],
            "every model OpenCode reported, and none it did not"
        );
        assert!(view
            .models
            .iter()
            .all(|model| model.availability == ModelAvailability::Available
                && model.provenance == ModelProvenance::Live));
    }
}
