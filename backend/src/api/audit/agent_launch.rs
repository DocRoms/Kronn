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

/// The tokens of an audit run so far: its finished steps added up. Unknown — not
/// zero — until a step's agent has reported something, so a runtime that reports
/// nothing never makes a run look free (KT-927).
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct RunTokens {
    total: u64,
    known: bool,
}

impl RunTokens {
    /// Count a finished step. A step of unknown cost leaves the tally as it was.
    pub(super) fn add(&mut self, step: Option<u64>) {
        if let Some(step) = step {
            self.total = self.total.saturating_add(step);
            self.known = true;
        }
    }

    /// The run's total, `None` while no step has reported.
    pub(super) fn total(&self) -> Option<u64> {
        self.known.then_some(self.total)
    }

    /// The running total with the step in progress at its latest reading.
    pub(super) fn with(&self, step: u64) -> u64 {
        self.total.saturating_add(step)
    }
}

pub(super) struct AuditAgentLauncher {
    state: AppState,
    /// `None` for a CLI agent: its spawn is left exactly as it always was.
    http: Option<HttpSettings>,
    /// The agent runs inside Kronn's ACP host: OpenCode and the other native ACP
    /// agents, and Claude and Codex through their adapters. What `start` hands
    /// back is then a lifeline process, not the agent (KT-927).
    acp: bool,
}

impl AuditAgentLauncher {
    pub(super) async fn new(state: &AppState, agent: &AgentType) -> Self {
        Self::with_route(state, agent, crate::acp::resolve_acp_route(agent)).await
    }

    /// `new` with the ACP route given rather than read from the environment, so a
    /// test can pick it without mutating process-wide variables.
    pub(super) async fn with_route(
        state: &AppState,
        agent: &AgentType,
        route: crate::acp::AcpProductionRoute,
    ) -> Self {
        let http = if runner::is_http_chat_agent(agent) {
            Some(HttpSettings::load(state, agent).await)
        } else {
            None
        };
        let acp = matches!(
            route,
            crate::acp::AcpProductionRoute::NativeAcp | crate::acp::AcpProductionRoute::AdaptedAcp
        );
        Self {
            state: state.clone(),
            http,
            acp,
        }
    }

    /// Whether stopping the agent means tripping a token rather than killing a
    /// process: an HTTP agent lives in Kronn's own tool loop, an ACP agent in a
    /// session Kronn cancels and whose process it shuts down. The PID of such a
    /// run belongs to a lifeline that does no work, so `cancel_audit` must not
    /// go looking for the agent there: killing it only made the pipeline believe
    /// the step was over while the agent kept working (KT-927, about 8 minutes).
    pub(super) fn stops_with_token(&self) -> bool {
        self.http.is_some() || self.acp
    }

    /// Spawn the agent of one step. `project_path` is the directory the pipeline
    /// resolved and validates; an HTTP agent's file tools are scoped to it and
    /// cannot leave it. `cancel` is honoured by an HTTP or ACP agent (see
    /// `stops_with_token`), whose provider, tool loop or session no process kill
    /// can reach.
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
                // A direct CLI is stopped by its PID and ignores the token.
                cancel_token: if self.acp { cancel } else { None },
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

#[cfg(test)]
#[path = "agent_launch_acp_tests.rs"]
mod acp_tests;
