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
pub mod codex_discovery;

use std::time::Duration;
use std::{collections::HashMap, sync::LazyLock};

use chrono::Utc;
use tokio::time::timeout;

use crate::db::model_catalog::{self as db, DiscoveredModel};
use crate::db::Database;
use crate::models::{
    AgentType, AppConfig, CatalogPreflightFailure, ModelAvailability, ModelCatalogView, ModelTier,
    ModelTierConfig, ModelTiersConfig, ModelUnavailableReason,
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

/// Historical defaults exposed solely to the one-time migration and unit
/// tests that exercise resolution without running application bootstrap.
pub fn migrated_default(agent_type: &AgentType, tier: ModelTier) -> Option<String> {
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

pub async fn refresh_runtime_cache(database: &Database) -> anyhow::Result<()> {
    let entries = database.with_read_conn(db::list_all).await?;
    let mut resolved = HashMap::new();
    for entry in entries {
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
                    db::insert_migrated_seed(
                        conn,
                        &agent,
                        model,
                        model,
                        Some(tier),
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
    /// No live discovery path exists for this runtime today (e.g. Claude
    /// Code before its ACP bridge is installed, or an ACP implementation that
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
    match timeout(DISCOVERY_TIMEOUT, discover_inner(agent_type)).await {
        Ok(outcome) => outcome,
        Err(_) => DiscoveryOutcome::Timeout,
    }
}

async fn discover_inner(agent_type: &AgentType) -> DiscoveryOutcome {
    match agent_type {
        AgentType::OpenCode
        | AgentType::GeminiCli
        | AgentType::CopilotCli
        | AgentType::Kiro
        | AgentType::Vibe => acp_discovery::discover(agent_type).await,
        AgentType::Codex => codex_discovery::discover().await,
        AgentType::ClaudeCode
            if crate::acp::resolve_acp_route(agent_type)
                == crate::acp::AcpProductionRoute::AdaptedAcp =>
        {
            acp_discovery::discover_claude_adapter().await
        }
        // KT-542 keeps the adapter explicitly opt-in. Without that toggle,
        // the catalog falls through to manual/migrated rows rather than
        // silently activating a different execution route.
        AgentType::ClaudeCode => DiscoveryOutcome::Unsupported,
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
    let runtime_target_id = db::agent_runtime_target_id(&agent_type);
    let outcome = discover(&agent_type).await;
    let at = agent_type.clone();
    let target = runtime_target_id.clone();
    match outcome {
        DiscoveryOutcome::Live(models) => {
            db.with_conn(move |conn| db::reconcile_live(conn, &target, &at, &models))
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
    })
}

/// Serve the current snapshot, refreshing first when it is stale (or when
/// `force` requests an explicit recheck). Non-managed runtimes (Ollama,
/// LiteLLM, NVIDIA, Custom) skip discovery entirely — their catalogue comes
/// from `external_api_connections` — and just return whatever (empty) rows
/// happen to exist for them.
pub async fn refresh_if_stale(
    db: &Database,
    agent_type: AgentType,
    force: bool,
) -> anyhow::Result<ModelCatalogView> {
    let runtime_target_id = db::agent_runtime_target_id(&agent_type);
    if !is_catalog_managed(&agent_type) {
        return build_view(db, runtime_target_id, agent_type).await;
    }
    if force {
        return refresh_agent_catalog(db, agent_type).await;
    }
    let target = runtime_target_id.clone();
    let log = db
        .with_conn(move |conn| db::get_refresh_log(conn, &target))
        .await?;
    let needs_refresh = match log {
        None => true,
        Some(log) => {
            Utc::now()
                .signed_duration_since(log.last_attempt_at)
                .to_std()
                .unwrap_or_default()
                > LIVE_CATALOG_TTL
        }
    };
    if needs_refresh {
        refresh_agent_catalog(db, agent_type).await
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
        ModelUnavailableReason::InvalidCatalog | ModelUnavailableReason::Unsupported => {
            "configure_manual_model"
        }
    }
}

/// Catalog-driven proactive preflight for one launch target. Returns `None`
/// when nothing in the catalog contradicts launching (including "we simply
/// have no record of this identity" — an unknown model is not blocked on
/// absence alone, only a model the catalog has positively marked
/// unavailable). Out-of-scope runtimes (Ollama/LiteLLM/NVIDIA/Custom) always
/// pass here; they keep their existing HTTP-reachability preflight.
pub async fn preflight_check(
    db: &Database,
    runtime_target_id: Option<&str>,
    agent_type: AgentType,
    tier: ModelTier,
    model_override: Option<&str>,
    model_tiers: Option<&ModelTiersConfig>,
) -> Option<CatalogPreflightFailure> {
    let resolved_model =
        crate::agents::runner::effective_model_flag(model_override, &agent_type, tier, model_tiers);
    let model_id = resolved_model?;
    let runtime_target_id = runtime_target_id
        .map(str::to_string)
        .unwrap_or_else(|| db::agent_runtime_target_id(&agent_type));

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
                return Some(CatalogPreflightFailure {
                    runtime_target_id,
                    agent_type,
                    model_id: Some(model_id),
                    reason: ModelUnavailableReason::ProviderError,
                    detail: format!("catalog refresh failed: {error}"),
                    last_checked_at: Utc::now(),
                    recommended_action: "recheck_catalog".into(),
                });
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
        if let Some(reason) = view
            .last_error_reason
            .filter(|reason| !matches!(reason, ModelUnavailableReason::Unsupported))
        {
            return Some(CatalogPreflightFailure {
                runtime_target_id,
                agent_type,
                model_id: Some(model_id),
                reason,
                detail: view
                    .last_error_detail
                    .unwrap_or_else(|| "the runtime catalog could not be refreshed".into()),
                last_checked_at: view.last_attempt_at.unwrap_or_else(Utc::now),
                recommended_action: recommended_action_for(reason).to_string(),
            });
        }
    }

    let target = runtime_target_id.clone();
    let mid = model_id.clone();
    let entry = db
        .with_conn(move |conn| db::get(conn, &target, &mid))
        .await
        .ok()
        .flatten();
    match entry {
        Some(entry) if entry.availability == ModelAvailability::Unavailable => {
            let reason = entry
                .unavailable_reason
                .unwrap_or(ModelUnavailableReason::Disappeared);
            Some(CatalogPreflightFailure {
                runtime_target_id,
                agent_type,
                model_id: Some(model_id),
                reason,
                detail: entry
                    .unavailable_detail
                    .unwrap_or_else(|| "this model is not currently available".into()),
                last_checked_at: entry.last_checked_at,
                recommended_action: recommended_action_for(reason).to_string(),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model_catalog as db;

    fn test_db() -> Database {
        Database::open_in_memory().unwrap()
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
        // Built-in default resolves to a model id (gpt-5.6-luna) that has no
        // catalog row in a fresh DB — unknown, not unavailable, must pass.
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

        // Claude stays on its non-discoverable fallback until KT-542's ACP
        // adapter is enabled, but the preflight must still execute and record
        // the bounded discovery decision instead of reading stale rows only.
        let failure = preflight_check(
            &db,
            None,
            AgentType::ClaudeCode,
            ModelTier::Default,
            None,
            None,
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
}
