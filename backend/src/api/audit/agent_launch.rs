// Starting the agent of one audit step (Full and partial share it).
//
// A CLI agent brings its own filesystem: the step hands it the prompt and the
// project path and it writes `docs/` itself. An HTTP agent (Ollama, LiteLLM) has
// none — every read and write it makes is a native tool Kronn executes for it
// (KT-338), and those tools only exist when the run is given an executor. The
// audit used to pass none, so admitting such an agent would have produced steps
// that "succeed" without writing anything (KT-924). Here the executor is scoped
// to the project directory the pipeline validates, and the settings an HTTP
// provider needs — tier models, endpoints, context ceilings, wall-clock — are
// read from the config the way a workflow or a discussion reads them.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::agents::runner::{self, AgentProcess, AgentStartConfig};
use crate::models::setup::HttpEndpoints;
use crate::models::{AgentType, ModelTier, ModelTiersConfig, TokensConfig};
use crate::AppState;

/// What an HTTP provider reads from Settings. Snapshotted once per audit, so every
/// step of a run resolves the same models and endpoints even if Settings change
/// underneath it.
struct HttpSettings {
    model_tiers: ModelTiersConfig,
    endpoints: HttpEndpoints,
    ollama_context_overrides: HashMap<String, u64>,
    request_timeout: Duration,
}

impl HttpSettings {
    async fn load(state: &AppState, agent: &AgentType) -> Self {
        let cfg = state.config.read().await;
        let minutes = if *agent == AgentType::Ollama {
            cfg.server.local_agent_global_timeout_min
        } else {
            cfg.server.agent_global_timeout_min
        };
        Self {
            model_tiers: cfg.agents.model_tiers.clone(),
            endpoints: HttpEndpoints::from_agents(&cfg.agents),
            ollama_context_overrides: cfg.server.ollama_context_overrides.clone(),
            request_timeout: Duration::from_secs(
                u64::from(crate::models::clamp_agent_global_timeout_min(u64::from(
                    minutes,
                ))) * 60,
            ),
        }
    }
}

pub(super) struct AuditAgentLauncher {
    state: AppState,
    /// `None` for a CLI agent: its spawn is left exactly as it always was.
    http: Option<HttpSettings>,
}

impl AuditAgentLauncher {
    pub(super) async fn new(state: &AppState, agent: &AgentType) -> Self {
        let http = if runner::is_http_chat_agent(agent) {
            Some(HttpSettings::load(state, agent).await)
        } else {
            None
        };
        Self {
            state: state.clone(),
            http,
        }
    }

    /// Whether the agent runs in Kronn's own tool loop, so that stopping it means
    /// cancelling a task rather than killing a process.
    pub(super) fn is_http(&self) -> bool {
        self.http.is_some()
    }

    /// Spawn the agent of one step. `project_path` is the directory the pipeline
    /// resolved and validates; an HTTP agent's file tools are scoped to it and
    /// cannot leave it. `cancel` is only honoured by an HTTP agent (see
    /// `is_http`), whose provider and tool loop no process kill can reach.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn start(
        &self,
        agent_type: &AgentType,
        tier: ModelTier,
        project_path: &Path,
        project_path_str: &str,
        prompt: &str,
        tokens: &TokensConfig,
        cancel: Option<CancellationToken>,
    ) -> Result<AgentProcess, String> {
        let Some(http) = &self.http else {
            return runner::start_agent_with_config(AgentStartConfig {
                full_access: true,
                tier,
                // A CLI agent reaches the project through its own filesystem;
                // native tools stay absent rather than inherited by omission.
                tools: None,
                ..AgentStartConfig::new(agent_type, project_path_str, prompt, tokens)
            })
            .await;
        };
        // The resolved path, not the stored one: inside a container the stored
        // host path does not exist, and the runner reads the project's docs from it.
        let resolved = project_path.to_string_lossy();
        runner::start_agent_with_config(AgentStartConfig {
            full_access: true,
            tier,
            tools: Some(crate::api::agent_tools::KronnToolExecutor::audit_arc(
                self.state.clone(),
                project_path.to_path_buf(),
            )),
            model_tiers: Some(&http.model_tiers),
            http_endpoints: Some(&http.endpoints),
            ollama_context_overrides: Some(&http.ollama_context_overrides),
            http_request_timeout: Some(http.request_timeout),
            cancel_token: cancel,
            ..AgentStartConfig::new(agent_type, &resolved, prompt, tokens)
        })
        .await
    }
}

#[cfg(test)]
#[path = "agent_launch_tests.rs"]
mod tests;
