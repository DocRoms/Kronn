//! Live model/mode discovery for ACP-native runtimes (KT-531 DoD #2:
//! OpenCode, Gemini CLI, Copilot CLI, Kiro, Vibe).
//!
//! Reuses the same `AcpHost`/`AcpJsonRpcTransport` the runner uses to start a
//! real session, but only negotiates and creates a throwaway session to read
//! `session/new`'s `configOptions`, then shuts the process down. No prompt is
//! ever sent, so discovery never spends a token turn — matching KT-531's
//! "aucun token agent n'est consommé" invariant for preflight-style checks.

use std::sync::Arc;

use crate::acp::{
    acp_agent, AcpConfigOption, AcpError, AcpHost, AcpInitialize, AcpJsonRpcTransport,
    AcpSessionScope, AcpTransport, ClaudeAcpAdapter,
};
use crate::db::model_catalog::DiscoveredModel;
use crate::models::AgentType;

use super::DiscoveryOutcome;

pub async fn discover(agent_type: &AgentType) -> DiscoveryOutcome {
    let Some(acp_agent_id) = acp_agent(agent_type) else {
        return DiscoveryOutcome::Unsupported;
    };
    // Discovery sends no prompt and writes no project data; the existing OS
    // temporary directory is sufficient and avoids a runtime-only dependency.
    let cwd = std::env::temp_dir().to_string_lossy().into_owned();

    let scope = AcpSessionScope::new(None, "model-catalog-discovery");
    let transport =
        match AcpJsonRpcTransport::spawn_native(acp_agent_id, &cwd, false, None, scope).await {
            Ok(transport) => Arc::new(transport) as Arc<dyn AcpTransport>,
            Err(error) => return classify_acp_error(error),
        };
    discover_with_transport(transport, cwd).await
}

/// Claude's KT-542 adapter implements the same `AcpTransport` boundary as a
/// native ACP runtime. Today Claude exposes no machine-readable model option,
/// so this honestly resolves to `Unsupported` while still exercising the real
/// adapter contract. If Claude adds config options later, discovery starts
/// working without another provider-specific path.
pub async fn discover_claude_adapter() -> DiscoveryOutcome {
    let cwd = std::env::temp_dir().to_string_lossy().into_owned();
    let transport: Arc<dyn AcpTransport> = Arc::new(ClaudeAcpAdapter::new(
        None,
        false,
        None,
        AcpSessionScope::new(None, "model-catalog-discovery"),
    ));
    discover_with_transport(transport, cwd).await
}

async fn discover_with_transport(
    transport: Arc<dyn AcpTransport>,
    cwd: String,
) -> DiscoveryOutcome {
    let mut host = AcpHost::new(1, transport);
    if let Err(error) = host
        .negotiate(AcpInitialize {
            protocol_version: 1,
            cwd: cwd.clone(),
            mcp_servers: Vec::new(),
        })
        .await
    {
        return classify_acp_error(error);
    }
    let session = match host.create_session().await {
        Ok(session) => session,
        Err(error) => return classify_acp_error(error),
    };
    let options = host.config_options().await;
    let _ = host.shutdown().await;
    let _ = session; // discovery never prompts; the session exists only to fetch config options

    match models_from_config_options(&options) {
        Some(models) if !models.is_empty() => DiscoveryOutcome::Live(models),
        _ => DiscoveryOutcome::Unsupported,
    }
}

/// ACP's session-config-options contract does not standardize which option
/// carries the model catalogue vs. a reasoning-effort/mode dial, so this
/// looks for an option whose id names a model catalogue and folds any
/// remaining reasoning/effort-like option into every discovered model's
/// `reasoning_modes`. A negotiated session without a model-catalogue option
/// is reported as `Unsupported` by the caller: it must not be confused with
/// an authoritative live empty catalogue that would invalidate the snapshot.
fn models_from_config_options(options: &[AcpConfigOption]) -> Option<Vec<DiscoveredModel>> {
    let default_reasoning_mode = options
        .iter()
        .find(|option| is_reasoning_option(&option.id))
        .and_then(|option| option.current.clone());
    let reasoning_modes: Vec<String> = options
        .iter()
        .filter(|option| is_reasoning_option(&option.id))
        .flat_map(|option| option.available.iter().map(|value| value.id.clone()))
        .collect();

    options
        .iter()
        .find(|option| is_model_option(&option.id))
        .map(|option| {
            option
                .available
                .iter()
                .map(|value| DiscoveredModel {
                    model_id: value.id.clone(),
                    display_name: value.name.clone(),
                    capabilities: Vec::new(),
                    reasoning_modes: reasoning_modes.clone(),
                    default_reasoning_mode: default_reasoning_mode.clone(),
                })
                .collect()
        })
}

fn is_model_option(id: &str) -> bool {
    id.to_lowercase().contains("model")
}

fn is_reasoning_option(id: &str) -> bool {
    let lower = id.to_lowercase();
    !is_model_option(id)
        && (lower.contains("reason") || lower.contains("effort") || lower.contains("mode"))
}

fn classify_acp_error(error: AcpError) -> DiscoveryOutcome {
    match error {
        AcpError::Timeout(_) => DiscoveryOutcome::Timeout,
        AcpError::Transport(detail) => {
            let lower = detail.to_lowercase();
            if lower.contains("no such file")
                || lower.contains("not found")
                || lower.contains("no verified production acp command")
                || lower.contains("os error 2")
            {
                DiscoveryOutcome::CliMissing(detail)
            } else if lower.contains("auth")
                || lower.contains("unauthorized")
                || lower.contains("401")
                || lower.contains("login")
            {
                DiscoveryOutcome::AuthRequired(detail)
            } else {
                DiscoveryOutcome::ProviderError(detail)
            }
        }
        other => DiscoveryOutcome::ProviderError(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::AcpConfigValue;

    fn value(id: &str, name: &str) -> AcpConfigValue {
        AcpConfigValue {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn models_from_config_options_finds_model_option_and_attaches_reasoning() {
        let options = vec![
            AcpConfigOption {
                id: "model".into(),
                current: Some("gpt-5.6-sol".into()),
                available: vec![
                    value("gpt-5.6-sol", "GPT-5.6 Sol"),
                    value("gpt-5.6-luna", "GPT-5.6 Luna"),
                ],
            },
            AcpConfigOption {
                id: "reasoningEffort".into(),
                current: Some("medium".into()),
                available: vec![
                    value("low", "Low"),
                    value("medium", "Medium"),
                    value("high", "High"),
                ],
            },
        ];
        let models = models_from_config_options(&options).expect("model option");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model_id, "gpt-5.6-sol");
        assert_eq!(models[0].reasoning_modes, vec!["low", "medium", "high"]);
        assert_eq!(models[0].default_reasoning_mode.as_deref(), Some("medium"));
    }

    #[test]
    fn models_from_config_options_empty_when_no_model_option() {
        let options = vec![AcpConfigOption {
            id: "theme".into(),
            current: None,
            available: vec![value("dark", "Dark")],
        }];
        assert!(models_from_config_options(&options).is_none());
    }

    #[test]
    fn negotiated_session_without_model_catalog_is_not_a_live_empty_snapshot() {
        let options = vec![AcpConfigOption {
            id: "reasoningEffort".into(),
            current: Some("medium".into()),
            available: vec![value("medium", "Medium")],
        }];
        assert_eq!(models_from_config_options(&options), None);
    }

    #[test]
    fn classify_acp_error_maps_timeout_and_missing_binary() {
        assert!(matches!(
            classify_acp_error(AcpError::Timeout("session/new".into())),
            DiscoveryOutcome::Timeout
        ));
        assert!(matches!(
            classify_acp_error(AcpError::Transport(
                "spawn ACP process: No such file or directory (os error 2)".into()
            )),
            DiscoveryOutcome::CliMissing(_)
        ));
        assert!(matches!(
            classify_acp_error(AcpError::Transport(
                "ACP response error: 401 unauthorized".into()
            )),
            DiscoveryOutcome::AuthRequired(_)
        ));
    }

    #[tokio::test]
    async fn discover_returns_unsupported_for_non_acp_agent() {
        let outcome = discover(&AgentType::Ollama).await;
        assert!(matches!(outcome, DiscoveryOutcome::Unsupported));
    }

    #[tokio::test]
    async fn claude_adapter_without_catalog_options_is_unsupported_not_live_empty() {
        let outcome = discover_claude_adapter().await;
        assert!(matches!(outcome, DiscoveryOutcome::Unsupported));
    }
}
