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
    /// Whether a CLI agent runs with full access. A native ACP agent needs its
    /// own full-access setting (an audit is no explicit choice); every other
    /// agent keeps the audit's established full access.
    full_access: bool,
    /// The named connection the user picked for an HTTP agent (KT-980), with
    /// its resolved endpoint and key. `None` uses the provider's default slot.
    connection: Option<(
        crate::models::ExternalApiConnection,
        crate::agents::runner::ExternalHttpRuntime,
    )>,
    /// The models every step of the run reported serving (KT-997).
    provenance: crate::agents::provenance::AgentProvenanceCapture,
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
        let full_access = route != crate::acp::AcpProductionRoute::NativeAcp
            || state.config.read().await.agents.full_access_for(agent);
        Self {
            state: state.clone(),
            http,
            acp,
            full_access,
            connection: None,
            provenance: Default::default(),
        }
    }

    /// Launcher for one audit request. A named connection must exist and
    /// target this agent; an HTTP agent's `Custom` target requires one.
    pub(super) async fn for_request(
        state: &AppState,
        agent: &AgentType,
        connection_id: Option<&str>,
    ) -> Result<Self, String> {
        let mut launcher = Self::new(state, agent).await;
        // Refused before the audit starts rather than at its first step.
        if !launcher.full_access {
            let language = state.config.read().await.language.clone();
            return Err(runner::native_full_access_refusal_in(agent, &language));
        }
        let Some(id) =
            crate::http_transport::validate_connection_target(state, agent, connection_id).await?
        else {
            return Ok(launcher);
        };
        let lookup = id.clone();
        let connection = state
            .db
            .with_read_conn(move |conn| crate::db::external_api_connections::get(conn, &lookup))
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("External API connection {id} was not found"))?;
        let tokens = state.config.read().await.tokens.clone();
        let runtime = crate::http_transport::external_http_runtime(&connection, &tokens)
            .ok_or_else(|| {
                format!(
                    "External API connection {} has no endpoint configured",
                    connection.display_name
                )
            })?;
        launcher.connection = Some((connection, runtime));
        Ok(launcher)
    }

    /// The connection a validation discussion must keep using after the audit.
    pub(super) fn connection_id(&self) -> Option<String> {
        self.connection
            .as_ref()
            .map(|(connection, _)| connection.id.clone())
    }

    /// The run's model as recorded on `audit_runs.model`: observed by any of its
    /// steps so far, else the one Settings or the named connection configure.
    pub(super) async fn run_model(&self, agent: &AgentType, tier: ModelTier) -> Option<String> {
        let observed = self
            .provenance
            .lock()
            .map(|state| state.observed_models.clone())
            .unwrap_or_default();
        if !observed.is_empty() {
            return run_model_label(&observed, None);
        }
        let configured = match self.connection.as_ref().and_then(|(connection, _)| {
            crate::http_transport::connection_tier_model(connection, tier)
        }) {
            Some(model) => Some(model),
            None => match &self.http {
                Some(http) => runner::configured_model_flag(agent, tier, Some(&http.model_tiers)),
                None => {
                    let tiers = self.state.config.read().await.agents.model_tiers.clone();
                    runner::configured_model_flag(agent, tier, Some(&tiers))
                }
            },
        };
        run_model_label(&[], configured.as_deref())
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
        step_target: &str,
        activity: Option<crate::agents::activity::AgentActivitySink>,
    ) -> Result<AgentProcess, String> {
        let Some(http) = &self.http else {
            return runner::start_agent_with_config(AgentStartConfig {
                provenance: Some(self.provenance.clone()),
                activity,
                full_access: self.full_access,
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
        let connection_model = self.connection.as_ref().and_then(|(connection, _)| {
            crate::http_transport::connection_tier_model(connection, tier)
        });
        runner::start_agent_with_config(AgentStartConfig {
            provenance: Some(self.provenance.clone()),
            activity,
            full_access: true,
            tier,
            tools: Some(
                crate::api::agent_tools::KronnToolExecutor::audit_arc_for_step(
                    self.state.clone(),
                    project_path.to_path_buf(),
                    Some(step_target),
                ),
            ),
            model_tiers: Some(&http.model_tiers),
            http_endpoints: Some(&http.endpoints),
            ollama_context_overrides: Some(&http.ollama_context_overrides),
            http_request_timeout: Some(http.request_timeout),
            external_http: self.connection.as_ref().map(|(_, runtime)| runtime),
            model_override: connection_model.as_deref(),
            cancel_token: cancel,
            ..AgentStartConfig::new(agent_type, &resolved, prompt, tokens)
        })
        .await
    }
}

/// How often a step without text is looked at: its tokens and its last tool can
/// move for minutes without a line arriving (an HTTP tool loop, an ACP session).
pub(super) const ACTIVITY_TICK: Duration = Duration::from_secs(1);

/// The last tool of a step's agent, whichever channel reports it: an HTTP agent
/// writes it on its run's usage, an ACP agent on the activity sink. The usage
/// is the run's own counters, which both kinds update without a text line.
pub(super) struct AuditActivityProbe {
    http: runner::ToolActivityProbe,
    acp: Option<tokio::sync::watch::Receiver<Option<crate::models::AgentActivity>>>,
}

impl AuditActivityProbe {
    pub(super) fn new(
        http: runner::ToolActivityProbe,
        acp: Option<tokio::sync::watch::Receiver<Option<crate::models::AgentActivity>>>,
    ) -> Self {
        Self { http, acp }
    }

    /// The last tool call's category and the step's call count, `None`
    /// before the first.
    pub(super) fn tool(&self) -> Option<(String, u32)> {
        let (category, calls) = self.http.read().or_else(|| {
            let activity = self.acp.as_ref()?.borrow().clone()?;
            Some((activity.category, activity.calls))
        })?;
        Some((category.as_str().to_owned(), calls))
    }

    pub(super) fn usage(&self) -> Option<runner::ReportedUsage> {
        self.http.usage()
    }
}

/// What changed in a step since it was last looked at. Polled on every text
/// line and on every `ACTIVITY_TICK`, so a tool chain without prose still moves
/// the counters; a quiet provider moves nothing.
pub(super) struct StepActivityWatch {
    probe: AuditActivityProbe,
    agent_type: AgentType,
    seen_usage: crate::db::audit_runs::StepTokens,
    seen_tool: Option<(String, u32)>,
}

impl StepActivityWatch {
    pub(super) fn new(probe: AuditActivityProbe, agent_type: AgentType) -> Self {
        Self {
            probe,
            agent_type,
            seen_usage: crate::db::audit_runs::StepTokens::UNKNOWN,
            seen_tool: None,
        }
    }

    /// The step's usage when it moved since the last call.
    pub(super) fn tokens_moved(&mut self) -> Option<crate::db::audit_runs::StepTokens> {
        let reading = crate::db::audit_runs::StepTokens::from_reported(self.probe.usage())
            .inclusive_for(&self.agent_type);
        (reading.total().is_some() && reading != self.seen_usage).then(|| {
            self.seen_usage = reading;
            reading
        })
    }

    /// The last tool and the call count when either moved since the last call.
    pub(super) fn tool_moved(&mut self) -> Option<(String, u32)> {
        let current = self.probe.tool()?;
        (self.seen_tool.as_ref() != Some(&current)).then(|| {
            self.seen_tool = Some(current.clone());
            current
        })
    }
}

/// The running step's latest actions for the details panel, by category,
/// fed by whichever pipeline runs the agent: a CLI's stream-json lines here,
/// an HTTP or ACP run's calls through its probe.
pub(super) struct StepRecentFeed {
    local: crate::agents::activity::RecentActivity,
    probe: Option<runner::ToolActivityProbe>,
    calls: u64,
    seen: Option<crate::models::AuditRecentActivity>,
}

impl StepRecentFeed {
    /// `probe` is the run's, for an agent whose tool calls are not in its lines.
    pub(super) fn new(probe: Option<runner::ToolActivityProbe>) -> Self {
        Self {
            local: Default::default(),
            probe,
            calls: 0,
            seen: None,
        }
    }

    /// One parsed stream-json event of a CLI agent: a tool's start counts, by
    /// its category, which is returned for the chip. Its input and the prose
    /// are never read.
    pub(super) fn on_stream_event(
        &mut self,
        event: &runner::StreamJsonEvent,
    ) -> Option<crate::models::ActivityCategory> {
        let runner::StreamJsonEvent::ToolStart(name) = event else {
            return None;
        };
        self.calls += 1;
        let update =
            crate::agents::activity::ToolActivityUpdate::named(Some(self.calls.to_string()), name);
        self.local.apply(&update);
        update.category()
    }

    /// The current snapshot when it changed since the last call.
    pub(super) fn moved(&mut self) -> Option<crate::models::AuditRecentActivity> {
        let mut current = self.local.snapshot();
        if current.entries.is_empty() {
            if let Some(remote) = self.probe.as_ref().and_then(|probe| probe.recent()) {
                current = remote;
            }
        }
        if current.entries.is_empty() {
            return None;
        }
        (self.seen.as_ref() != Some(&current)).then(|| {
            self.seen = Some(current.clone());
            current
        })
    }
}

/// What woke a step's read loop: a line (or the end of the stream), the idle
/// deadline, or the activity tick.
pub(super) enum StepWake {
    Line(Option<String>),
    Idle,
    Tick,
}

/// What a step's attempts cost, as their agents reported it (KT-997). Known only
/// when every attempt that ran reported one: summing over a silent attempt would
/// show a floor as the whole.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum StepCost {
    #[default]
    NoAttempt,
    Known(u64),
    Unknown,
}

impl StepCost {
    pub(super) fn with_attempt(self, reported_usd_micros: Option<u64>) -> Self {
        match (self, reported_usd_micros) {
            (Self::Unknown, _) | (_, None) => Self::Unknown,
            (Self::NoAttempt, Some(cost)) => Self::Known(cost),
            (Self::Known(sum), Some(cost)) => Self::Known(sum.saturating_add(cost)),
        }
    }

    /// The cost to record; `None` keeps the column NULL (unknown, never 0).
    pub(super) fn usd_micros(self) -> Option<u64> {
        match self {
            Self::Known(cost) => Some(cost),
            Self::NoAttempt | Self::Unknown => None,
        }
    }
}

/// The model a run is recorded with: the one(s) its runtime reported serving,
/// else the configured one, labelled so it never passes for an observation.
pub(super) fn run_model_label(observed: &[String], configured: Option<&str>) -> Option<String> {
    if !observed.is_empty() {
        return Some(observed.join(" / "));
    }
    configured
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(|model| format!("{model} (configured)"))
}

#[cfg(test)]
#[path = "agent_launch_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_launch_acp_tests.rs"]
mod acp_tests;
