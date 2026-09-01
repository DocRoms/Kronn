//! HTTP model-provider transport boundary (KT-545).
//!
//! This module owns what LiteLLM, NVIDIA and named custom HTTP connections
//! share once the wire is OpenAI-compatible: which chat codec they speak
//! and which model a connection's tier resolves to. It is deliberately not
//! part of `acp.rs` — per ADR-003, OpenAI-compatible HTTP is a separate
//! runtime-to-model-provider transport, never an ACP runtime or an MCP
//! server, and `acp::production_route`/`resolve_acp_route` already return
//! `AcpProductionRoute::HttpModelProvider` for every agent type this module
//! covers (`Ollama`, `LiteLlm`, `Nvidia`, `Custom`).
//!
//! Per-connection identity, credentials and endpoints stay owned by
//! `external_api_connections`; per-model capability/tier data stays owned by
//! `core::model_catalog`. This module is the seam between them: the explicit
//! codec choice and the pre-dispatch capability check that keeps a model the
//! catalog knows is image/video-only from ever reaching the chat codec.

use crate::agents::chat_codec::{ChatCodec, OllamaCodec, OpenAiCodec};
use crate::agents::runner::ExternalHttpRuntime;
use crate::models::{AgentType, CatalogModelEntry, ExternalApiConnection, ModelTier, TokensConfig};

/// Capability tag a catalog entry must carry to be dispatched through the
/// chat codec. Matches the free-form strings `external_api_connections::test`
/// and the model-catalog reconciliation already write (`"chat"`, `"image"`,
/// `"video"` — see `api/external_api_connections.rs` catalog tests).
pub(crate) const CAPABILITY_CHAT: &str = "chat";

/// Explicit codec selection for the shared HTTP chat path — the "codec
/// OpenAI Chat est explicite" half of KT-545's contract. `Ollama` speaks its
/// native `/api/chat`; every other HTTP-chat agent (`LiteLlm`, `Nvidia`,
/// `Custom`) speaks the OpenAI-compatible `/v1/chat/completions` wire,
/// regardless of which named connection backs it. One function, one
/// decision — mirrors `acp::resolve_acp_route` being the sole ACP route
/// decision point.
pub(crate) fn resolve_chat_codec(agent_type: &AgentType) -> Box<dyn ChatCodec> {
    if crate::agents::runner::is_openai_wire_agent(agent_type) {
        Box::new(OpenAiCodec)
    } else {
        Box::new(OllamaCodec)
    }
}

/// Whether a catalog entry is compatible with `capability`. An entry with no
/// recorded capabilities (every CLI-native discovery today, or a model never
/// covered by a connection test) is neither confirmed nor denied — consistent
/// with `model_catalog::preflight_check`'s "unknown is not blocked" policy,
/// only a *positive* mismatch (the entry lists capabilities and this one is
/// not among them) refuses.
pub(crate) fn entry_supports_capability(entry: &CatalogModelEntry, capability: &str) -> bool {
    entry.capabilities.is_empty() || entry.capabilities.iter().any(|c| c.as_str() == capability)
}

/// Resolve a named connection's model for one tier — the single rule shared
/// by discussion dispatch, Quick Prompt launches, Compare judge/improve and
/// workflow/batch-compare retries, so a connection's tier resolves
/// identically everywhere (KT-545 DoD #4). Falls back to the connection's
/// Default-tier model when the requested tier has none configured, so
/// leaving Economy/Reasoning empty at setup degrades to the connection's
/// main model instead of an unconfigured-model refusal.
pub(crate) fn connection_tier_model(
    connection: &ExternalApiConnection,
    tier: ModelTier,
) -> Option<String> {
    let selected = match tier {
        ModelTier::Economy => &connection.economy_model,
        ModelTier::Default => &connection.default_model,
        ModelTier::Reasoning => &connection.reasoning_model,
    };
    selected
        .clone()
        .or_else(|| connection.default_model.clone())
}

/// Build the HTTP runtime a dispatch actually needs (endpoint + resolved
/// credential) from a named connection — the same construction discussion
/// dispatch already does, shared here so Compare and orchestration resolve a
/// connection identically instead of reimplementing it (KT-545 DoD #4).
/// `None` when the connection has no endpoint configured yet.
pub(crate) fn external_http_runtime(
    connection: &ExternalApiConnection,
    tokens: &TokensConfig,
) -> Option<ExternalHttpRuntime> {
    let endpoint = connection.endpoint.as_ref()?;
    Some(ExternalHttpRuntime {
        display_name: connection.display_name.clone(),
        mention_alias: connection.mention_alias.clone(),
        endpoint: endpoint.clone(),
        api_key: tokens
            .active_key_for(&connection.credential_slug)
            .filter(|key| !key.trim().is_empty())
            .map(str::to_string),
    })
}

/// Validate a launch's optional connection before anything is persisted or
/// dispatched — the same "wrong connection for this agent" guard Quick
/// Prompts and workflow retries already apply, shared so Compare and
/// orchestration refuse a mismatched connection identically (KT-545 DoD #4).
/// Returns the validated connection id (or `None` for a non-`Custom` agent
/// with no connection requested).
pub(crate) async fn validate_connection_target(
    state: &crate::AppState,
    agent: &AgentType,
    connection_id: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(connection_id) = connection_id else {
        if *agent == AgentType::Custom {
            return Err("A custom external API target requires a connection_id".to_string());
        }
        return Ok(None);
    };
    let lookup_id = connection_id.to_string();
    let connection = state
        .db
        .with_read_conn(move |conn| crate::db::external_api_connections::get(conn, &lookup_id))
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("External API connection {connection_id} was not found"))?;
    let expected = crate::db::external_api_connections::target_for_connection(&connection);
    if expected.agent_type != *agent {
        return Err(format!(
            "External API connection {connection_id} does not match the selected agent"
        ));
    }
    Ok(Some(connection_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ModelAvailability, ModelProvenance};
    use chrono::Utc;

    fn entry(capabilities: Vec<&str>) -> CatalogModelEntry {
        let now = Utc::now();
        CatalogModelEntry {
            id: "id".into(),
            runtime_target_id: "http:conn-a".into(),
            agent_type: AgentType::Custom,
            model_id: "m".into(),
            display_name: "M".into(),
            display_alias: None,
            provenance: ModelProvenance::Live,
            availability: ModelAvailability::Available,
            unavailable_reason: None,
            unavailable_detail: None,
            capabilities: capabilities.into_iter().map(String::from).collect(),
            reasoning_modes: vec![],
            default_reasoning_mode: None,
            tier_assignment: None,
            manual_origin: false,
            first_seen_at: now,
            last_seen_at: Some(now),
            last_checked_at: now,
            created_at: now,
            updated_at: now,
        }
    }

    fn connection(
        economy: Option<&str>,
        default: Option<&str>,
        reasoning: Option<&str>,
    ) -> ExternalApiConnection {
        let now = Utc::now();
        ExternalApiConnection {
            id: "conn-a".into(),
            display_name: "Groq".into(),
            mention_alias: "groq".into(),
            endpoint: Some("https://api.groq.com".into()),
            credential_slug: "conn-groq".into(),
            origin_preset: crate::models::ExternalApiConnectionPreset::Other,
            economy_model: economy.map(String::from),
            default_model: default.map(String::from),
            reasoning_model: reasoning.map(String::from),
            created_at: now,
            updated_at: now,
            image_model: None,
            video_model: None,
            media_endpoint: None,
        }
    }

    #[test]
    fn resolve_chat_codec_picks_openai_wire_for_remote_agents() {
        for agent in [AgentType::LiteLlm, AgentType::Nvidia, AgentType::Custom] {
            assert_eq!(
                resolve_chat_codec(&agent).endpoint("http://h"),
                "http://h/v1/chat/completions",
                "{agent:?} must speak the OpenAI-compatible wire"
            );
        }
    }

    #[test]
    fn resolve_chat_codec_picks_native_wire_for_ollama() {
        assert_eq!(
            resolve_chat_codec(&AgentType::Ollama).endpoint("http://h"),
            "http://h/api/chat"
        );
    }

    #[test]
    fn unknown_capabilities_are_permitted() {
        assert!(entry_supports_capability(&entry(vec![]), CAPABILITY_CHAT));
    }

    #[test]
    fn chat_tagged_entry_is_permitted() {
        assert!(entry_supports_capability(
            &entry(vec!["chat"]),
            CAPABILITY_CHAT
        ));
        assert!(entry_supports_capability(
            &entry(vec!["chat", "tools"]),
            CAPABILITY_CHAT
        ));
    }

    #[test]
    fn media_only_entry_is_refused_for_chat() {
        assert!(!entry_supports_capability(
            &entry(vec!["video"]),
            CAPABILITY_CHAT
        ));
        assert!(!entry_supports_capability(
            &entry(vec!["image"]),
            CAPABILITY_CHAT
        ));
    }

    #[test]
    fn connection_tier_model_picks_the_matching_slot() {
        let c = connection(Some("eco"), Some("def"), Some("rsn"));
        assert_eq!(
            connection_tier_model(&c, ModelTier::Economy),
            Some("eco".into())
        );
        assert_eq!(
            connection_tier_model(&c, ModelTier::Default),
            Some("def".into())
        );
        assert_eq!(
            connection_tier_model(&c, ModelTier::Reasoning),
            Some("rsn".into())
        );
    }

    #[test]
    fn connection_tier_model_falls_back_to_default_when_tier_slot_is_empty() {
        let c = connection(None, Some("def"), None);
        assert_eq!(
            connection_tier_model(&c, ModelTier::Economy),
            Some("def".into())
        );
        assert_eq!(
            connection_tier_model(&c, ModelTier::Reasoning),
            Some("def".into())
        );
    }

    #[test]
    fn connection_tier_model_is_none_when_nothing_is_configured() {
        let c = connection(None, None, None);
        assert_eq!(connection_tier_model(&c, ModelTier::Default), None);
    }

    #[test]
    fn http_chat_agents_route_through_the_http_model_provider_boundary() {
        // DoD #5: the API/runner boundary never confuses ACP transport with
        // the HTTP provider route for any agent this module covers.
        for agent in [
            AgentType::Ollama,
            AgentType::LiteLlm,
            AgentType::Nvidia,
            AgentType::Custom,
        ] {
            assert_eq!(
                crate::acp::resolve_acp_route(&agent),
                crate::acp::AcpProductionRoute::HttpModelProvider,
                "{agent:?} must resolve to the HTTP boundary, never an ACP route"
            );
        }
    }
}
