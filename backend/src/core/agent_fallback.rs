//! KT-1518 / KT-1693 — the agent fallback.
//!
//! Each agent has, per mode (eco, standard, advanced, decision, image, video),
//! at most one agent/model that takes over when the PROVIDER cannot serve it
//! (`AgentConfig::mode_fallbacks`, e.g. Claude Code standard → Codex): a rate
//! limit, a quota or credits spent, an outage that survived its retries, a CLI
//! that is not authenticated. Never a task failure, a refusal, a test failure or
//! the timeout of the work itself: [`classify_trigger`] answers `None` for
//! those, and every surface asks it before it switches anything.
//!
//! This module is the policy and nothing else — no spawning, no rooms. The three
//! surfaces (a discussion turn, a workflow Agent step, a task worker) each ask
//! [`select`] for the next link and apply it their own way, so a link is chosen
//! by the same rules everywhere:
//!
//! * a fallback goes to the same KIND of mode: text (eco, standard or advanced)
//!   to text, decision to decision, image to image, video to video
//!   ([`check_shape`]); when it is itself unavailable, the fallback set on ITS
//!   own row applies ([`effective_chain`]), at most [`MAX_CHAIN_DEPTH`] links,
//!   an agent never twice and never itself (refused when saved, re-checked
//!   here); the discussion's / step's override replaces the first link, an
//!   empty override is "no fallback";
//! * a link on cooldown, not installed, or lacking something the turn needs (MCP
//!   tools, workspace writes, a shell, image input, a long context) is skipped,
//!   and the walk goes on; there is no "paid" rule: enabling a fallback is the
//!   consent;
//! * nothing usable ends in a clear message, not a silent stop.
//!
//! The same TIER is kept (economy stays economy): a fallback carries the tier of
//! the interrupted turn and resolves it on its own agent. A model name is
//! provider-specific, so the failing agent's model is never carried over; only
//! the link's own explicit `model` replaces the tier.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{
    AgentFallback, AgentType, AgentsConfig, FallbackGap, FallbackKind, FallbackMode,
    FallbackTransport, ModeFallback, ModelTier,
};

/// Links followed from one agent. Bounded so a long chain cannot turn one
/// refused turn into a tour of every provider the owner has.
pub const MAX_CHAIN_DEPTH: usize = 4;

/// A transcript past this many characters is a "long context" turn: an agent
/// that serves a capped window (a local Ollama model) is skipped for it.
pub const LONG_CONTEXT_CHARS: usize = 150_000;

/// Why a provider could not serve the turn, and so why a fallback may run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Trigger {
    /// HTTP 429 / the provider's rate-limit wording.
    RateLimit,
    /// Plan, quota or credits spent.
    QuotaExhausted,
    /// 5xx / overloaded / unavailable, after the retries.
    Outage,
    /// The CLI or API key is not authenticated.
    NotAuthenticated,
    /// KT-1758 — the agent cannot be launched as this kind of worker at all
    /// (a native ACP agent refused as a task worker, a missing binary). Waiting
    /// never changes it, and it says nothing against the agent for other turns:
    /// no cooldown is written for it, only the next link is asked for.
    LaunchRefused,
    /// KT-1758 — a task worker whose launch kept failing for a cause that can
    /// change, until its retries ran out. Not a refusal: nothing says the agent
    /// cannot be this kind of worker, only that it was not there this time. Like
    /// [`Trigger::LaunchRefused`], it writes no cooldown.
    LaunchUnavailable,
}

impl Trigger {
    /// A stop at the launch of a worker, which is about this worker's start and
    /// not about the provider: no cooldown, no "until".
    pub fn is_launch_stop(self) -> bool {
        matches!(self, Trigger::LaunchRefused | Trigger::LaunchUnavailable)
    }

    pub fn code(self) -> &'static str {
        match self {
            Trigger::RateLimit => "rate_limit",
            Trigger::QuotaExhausted => "quota_exhausted",
            Trigger::Outage => "outage",
            Trigger::NotAuthenticated => "not_authenticated",
            Trigger::LaunchRefused => "launch_refused",
            Trigger::LaunchUnavailable => "launch_unavailable",
        }
    }

    /// The words the room uses for the reason.
    pub fn reason(self) -> &'static str {
        match self {
            Trigger::RateLimit => "rate limited",
            Trigger::QuotaExhausted => "quota exhausted",
            Trigger::Outage => "provider unavailable",
            Trigger::NotAuthenticated => "not authenticated",
            Trigger::LaunchRefused => "cannot be launched as a task worker",
            Trigger::LaunchUnavailable => "unavailable at launch",
        }
    }

    /// How long the agent is left alone when the provider announced no reset.
    /// It is a re-try window, not a guess of the provider's schedule: after it
    /// the next turn simply tries the agent again, and a new refusal renews it.
    pub fn default_cooldown(self) -> Duration {
        match self {
            Trigger::RateLimit | Trigger::Outage => Duration::minutes(5),
            Trigger::QuotaExhausted => crate::workflows::quota_wait::RECHECK_INTERVAL,
            Trigger::NotAuthenticated => Duration::minutes(30),
            // Not a cooldown: see `Trigger::LaunchRefused`.
            Trigger::LaunchRefused | Trigger::LaunchUnavailable => Duration::zero(),
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [
            Trigger::RateLimit,
            Trigger::QuotaExhausted,
            Trigger::Outage,
            Trigger::NotAuthenticated,
            Trigger::LaunchRefused,
            Trigger::LaunchUnavailable,
        ]
        .into_iter()
        .find(|trigger| trigger.code() == code)
    }
}

/// Which provider-side failure `error` is, if it is one. `None` for everything
/// else — in particular the stall or timeout of the work itself, which says
/// nothing about the provider: a fallback would only repeat the same wait.
pub fn classify_trigger(error: &str) -> Option<Trigger> {
    use crate::core::quota_text::is_hard_quota_exhausted;
    let lower = error.to_lowercase();
    if lower.contains("agent stalled") || lower.contains("global timeout") {
        return None;
    }
    if is_hard_quota_exhausted(error) {
        return Some(Trigger::QuotaExhausted);
    }
    if crate::workflows::quota_wait::is_quota_refusal(error) {
        return Some(Trigger::RateLimit);
    }
    if is_outage(&lower) {
        return Some(Trigger::Outage);
    }
    if is_not_authenticated(&lower) {
        return Some(Trigger::NotAuthenticated);
    }
    None
}

fn is_outage(lower: &str) -> bool {
    [
        "overloaded",
        "http 529",
        "error: 529",
        "status 529",
        "service unavailable",
        "internal server error",
        "502 bad gateway",
        "503 service unavailable",
        "504 gateway timeout",
        "api error: 500",
        "api error: 502",
        "api error: 503",
        "http 500",
        "http 502",
        "http 503",
        "http 504",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn is_not_authenticated(lower: &str) -> bool {
    [
        "authentication_error",
        "invalid authentication",
        "invalid x-api-key",
        "not authenticated",
        "not logged in",
        "api error: 401",
        "http 401",
        "status 401",
        "401 unauthorized",
        "please run /login",
        "please log in",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// When the provider says the agent is back: its own announcement, else `None`.
pub fn announced_reset(error: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    crate::workflows::quota_wait::announced_reset(error, now)
}

// ─── Capabilities ────────────────────────────────────────────────────────

/// What a turn needs from whoever runs it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TurnNeeds {
    /// MCP tools (a CLI agent's bridge). An HTTP agent has none: what it can do
    /// is a catalogue Kronn executes for it.
    pub mcp_tools: bool,
    /// Writes in the worktree.
    pub write_access: bool,
    /// A task worker must also perform Git writes in a linked worktree.
    pub task_worker: bool,
    /// KT-1991 — a shell on the host: build, run the tests, use Git. A task
    /// worker with no prelocalised scope (`worker_scope`) needs one; an HTTP
    /// agent has none, whatever workspace writes Kronn executes for it.
    pub shell: bool,
    /// Image attachments to read.
    pub image_input: bool,
    /// A transcript past [`LONG_CONTEXT_CHARS`].
    pub long_context: bool,
}

/// What an agent can give a turn, as far as its TYPE tells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capabilities {
    pub mcp_tools: bool,
    pub write_access: bool,
    pub shell: bool,
    pub image_input: bool,
    pub long_context: bool,
}

/// `docs/architecture/http-agent-capabilities.md`: a CLI agent runs as a local
/// process with a filesystem, a shell and the MCP bridge; an HTTP agent only has
/// the tools Kronn executes for it (workspace writes included, no MCP, no shell:
/// it cannot build, test or commit). An image
/// reaches a CLI agent as a path it opens itself; an HTTP model sees it only when
/// that model is known to (`agents/vision.rs` treats "unknown" as "no"), so an
/// HTTP agent is not counted on for images. A local Ollama window is capped.
pub fn capabilities(agent: &AgentType) -> Capabilities {
    let http = crate::agents::runner::is_http_chat_agent(agent);
    Capabilities {
        mcp_tools: !http,
        write_access: !matches!(agent, AgentType::Custom),
        shell: !http,
        image_input: !http,
        long_context: !matches!(agent, AgentType::Ollama),
    }
}

/// KT-1991 — the gap a room reads when a task worker would land on an HTTP
/// model (« LiteLLM cannot take this turn: it has no shell … »).
pub const NO_SHELL: &str = "it has no shell to build, run the tests or commit";

/// The first thing `needs` asks that `agent` cannot give.
pub fn capability_gap(agent: &AgentType, needs: &TurnNeeds) -> Option<&'static str> {
    if needs.task_worker && *agent == AgentType::Codex {
        return Some("Codex task-worker fallback is disabled: its sandbox cannot write the linked worktree Git administrative directories; server-side commit alone does not enable merges");
    }
    let has = capabilities(agent);
    if needs.mcp_tools && !has.mcp_tools {
        return Some("it has no MCP tools");
    }
    if needs.write_access && !has.write_access {
        return Some("it cannot write in the worktree");
    }
    if needs.shell && !has.shell {
        return Some(NO_SHELL);
    }
    if needs.image_input && !has.image_input {
        return Some("it cannot be counted on to read images");
    }
    if needs.long_context && !has.long_context {
        return Some("its context window is too small for this transcript");
    }
    None
}

/// KT-1749 — the typed identity a task worker on `agent` is launched with: an
/// HTTP provider (Ollama, LiteLLM, NVIDIA) is a `discussion_agent`, a host CLI
/// process an `agent`. The same rule `ensure_task_worker_transport_compatible`
/// enforces, so a fallback never builds a target the launch rejects.
pub fn task_worker_target(agent: &AgentType) -> crate::models::MessageTarget {
    if crate::agents::runner::is_http_chat_agent(agent) {
        crate::models::MessageTarget::discussion_agent(agent.clone())
    } else {
        crate::models::MessageTarget::agent(agent.clone())
    }
}

/// KT-1749 — why the launch of a task worker would refuse `agent` before any
/// spawn (transport, native ACP route, typed-delivery capability): the very
/// check the launch boundary applies to a typed worker target, so the fallback
/// walk skips such a link instead of moving the execution onto it.
pub fn task_worker_launch_refusal(agent: &AgentType) -> Option<String> {
    crate::api::orchestration::worker_static_refusal(&task_worker_target(agent))
        .map(|reason| reason.detail)
}

/// How an agent is reached: a CLI process (OpenCode's ACP included), a remote
/// HTTP API (LiteLLM, NVIDIA, a named connection) or a local model server
/// (Ollama). Shown as a badge on every fallback.
pub fn transport(agent: &AgentType) -> FallbackTransport {
    match agent {
        AgentType::Ollama => FallbackTransport::Local,
        AgentType::LiteLlm | AgentType::Nvidia | AgentType::Custom => FallbackTransport::Http,
        _ => FallbackTransport::Cli,
    }
}

/// The label a person reads.
pub fn display_name(agent: &AgentType) -> &'static str {
    match agent {
        AgentType::ClaudeCode => "Claude Code",
        AgentType::Codex => "Codex",
        AgentType::OpenCode => "OpenCode",
        AgentType::Vibe => "Vibe",
        AgentType::GeminiCli => "Gemini CLI",
        AgentType::Kiro => "Kiro",
        AgentType::CopilotCli => "Copilot CLI",
        AgentType::Ollama => "Ollama",
        AgentType::LiteLlm => "LiteLLM",
        AgentType::Nvidia => "NVIDIA",
        AgentType::Custom => "Custom",
    }
}

// ─── The chain ───────────────────────────────────────────────────────────

/// Why a fallback, or a chain, is refused.
#[derive(Debug, Clone, PartialEq)]
pub enum ChainError {
    SelfFallback(AgentType),
    NoCardFor(AgentType),
    /// An agent twice in the chain (or the starting agent in it): the walk would
    /// come back to where it was.
    Cycle(Vec<AgentType>),
    TooDeep {
        from: Option<AgentType>,
        links: usize,
    },
    BroaderAccess {
        from: AgentType,
        to: AgentType,
    },
    BadModel(&'static str),
    /// The fallback's mode is not of the kind of the mode it replaces: text goes
    /// to text, decision to decision, image to image, video to video.
    WrongKind {
        mode: FallbackMode,
        target: FallbackMode,
    },
}

fn kind_label(kind: FallbackKind) -> &'static str {
    match kind {
        FallbackKind::Text => "text (eco, standard or advanced)",
        FallbackKind::Decision => "decision",
        FallbackKind::Image => "image",
        FallbackKind::Video => "video",
    }
}

impl std::fmt::Display for ChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainError::SelfFallback(agent) => write!(
                f,
                "{} cannot be its own fallback.",
                display_name(agent)
            ),
            ChainError::NoCardFor(agent) => write!(
                f,
                "{} is a named connection chosen in the workflow, not an agent that can take part in a fallback chain.",
                display_name(agent)
            ),
            ChainError::Cycle(path) => write!(
                f,
                "This fallback would loop: {}.",
                path.iter().map(display_name).collect::<Vec<_>>().join(" → ")
            ),
            ChainError::TooDeep { links, .. } => write!(
                f,
                "A fallback chain would be {links} links long; at most {MAX_CHAIN_DEPTH} are allowed."
            ),
            ChainError::BroaderAccess { from, to } => write!(
                f,
                "{} has full access and {} does not: a fallback never gets more access than the agent it replaces. Grant {} the same access first, or pick another fallback.",
                display_name(to),
                display_name(from),
                display_name(from),
            ),
            ChainError::BadModel(why) => write!(f, "The fallback model {why}."),
            ChainError::WrongKind { mode, target } => write!(
                f,
                "A {} fallback must be a {} model; {} is not one.",
                mode.label(),
                kind_label(mode.kind()),
                target.label()
            ),
        }
    }
}

impl std::error::Error for ChainError {}

/// The fallback `agent` has set on its `mode` row.
pub fn row<'a>(
    agents: &'a AgentsConfig,
    agent: &AgentType,
    mode: FallbackMode,
) -> Option<&'a AgentFallback> {
    agents
        .agent_config(agent)?
        .mode_fallbacks
        .iter()
        .find(|entry| entry.row == mode)
        .map(|entry| &entry.fallback)
}

/// Sets (`Some`) or removes (`None`) the `mode` row of `agent`. `false` when the
/// agent has no card (a named connection).
pub fn set_row(
    agents: &mut AgentsConfig,
    agent: &AgentType,
    mode: FallbackMode,
    fallback: Option<AgentFallback>,
) -> bool {
    let Some(config) = agents.agent_config_mut(agent) else {
        return false;
    };
    config.mode_fallbacks.retain(|entry| entry.row != mode);
    if let Some(fallback) = fallback {
        config.mode_fallbacks.push(ModeFallback {
            row: mode,
            fallback,
        });
        config.mode_fallbacks.sort_by_key(|entry| entry.row);
    }
    true
}

/// The mode a link runs in: its own, else the one it replaces.
pub fn link_mode(link: &AgentFallback, replaced: FallbackMode) -> FallbackMode {
    link.mode.unwrap_or(replaced)
}

/// Goes on from the last link of `chain` through the rows its agent has set —
/// the fallback of a fallback — until a row is missing, would come back to an
/// agent already on the walk (a loop) or the chain is [`MAX_CHAIN_DEPTH`] long.
/// `origin` and `mode` are where the walk started. Returns where the first
/// added link comes from, per added link: the agent whose row it is.
fn extend_through_rows(
    agents: &AgentsConfig,
    origin: Option<&AgentType>,
    mode: FallbackMode,
    chain: &mut Vec<AgentFallback>,
) -> Vec<AgentType> {
    let mut seen: Vec<AgentType> = origin.cloned().into_iter().collect();
    let mut tail_mode = mode;
    for link in chain.iter() {
        seen.push(link.agent.clone());
        tail_mode = link_mode(link, tail_mode);
    }
    let mut tail_agent = chain
        .last()
        .map(|link| link.agent.clone())
        .or_else(|| origin.cloned());
    let mut sources = Vec::new();
    while chain.len() < MAX_CHAIN_DEPTH {
        let Some(owner) = tail_agent.clone() else {
            break;
        };
        let Some(next) = row(agents, &owner, tail_mode) else {
            break;
        };
        let next_mode = link_mode(next, tail_mode);
        if next.agent == AgentType::Custom
            || seen.contains(&next.agent)
            || next_mode.kind() != tail_mode.kind()
        {
            break;
        }
        seen.push(next.agent.clone());
        sources.push(owner);
        tail_agent = Some(next.agent.clone());
        tail_mode = next_mode;
        chain.push(next.clone());
    }
    sources
}

/// The chain a turn that starts on `origin` in `mode` follows: the override's
/// links when there is one (an empty override is "no fallback"), else the row of
/// `origin`; either way continued through the rows of the last link.
pub fn effective_chain(
    agents: &AgentsConfig,
    origin: &AgentType,
    mode: FallbackMode,
    chain_override: Option<&[AgentFallback]>,
) -> Vec<AgentFallback> {
    let mut chain = match chain_override {
        Some([]) => return Vec::new(),
        Some(links) => links.to_vec(),
        None => Vec::new(),
    };
    extend_through_rows(agents, Some(origin), mode, &mut chain);
    chain
}

/// The default chain of `from` in `mode`: its own row, then the rows of the
/// fallbacks. Empty when the row is off.
pub fn default_chain(
    agents: &AgentsConfig,
    from: &AgentType,
    mode: FallbackMode,
) -> Vec<AgentFallback> {
    effective_chain(agents, from, mode, None)
}

/// What is wrong with a list of links whatever the configuration: too long, the
/// starting agent or one agent twice in it (a loop), an agent with no card (a
/// named connection), a bad model, a link of another kind than `mode` (`None`:
/// not tied to one mode). `from` is the agent the chain belongs to (`None` for
/// an override that is not tied to one agent, as a discussion's is).
pub fn check_shape(
    from: Option<&AgentType>,
    mode: Option<FallbackMode>,
    chain: &[AgentFallback],
) -> Result<(), ChainError> {
    if chain.len() > MAX_CHAIN_DEPTH {
        return Err(ChainError::TooDeep {
            from: from.cloned(),
            links: chain.len(),
        });
    }
    let mut seen: Vec<AgentType> = from.cloned().into_iter().collect();
    for link in chain {
        if from == Some(&link.agent) {
            return Err(ChainError::SelfFallback(link.agent.clone()));
        }
        if link.agent == AgentType::Custom {
            return Err(ChainError::NoCardFor(link.agent.clone()));
        }
        if seen.contains(&link.agent) {
            let mut path = seen.clone();
            path.push(link.agent.clone());
            return Err(ChainError::Cycle(path));
        }
        seen.push(link.agent.clone());
        if let (Some(mode), Some(target)) = (mode, link.mode) {
            if target.kind() != mode.kind() {
                return Err(ChainError::WrongKind { mode, target });
            }
        }
        if let Some(model) = &link.model {
            if model.trim().is_empty() {
                return Err(ChainError::BadModel("is empty"));
            }
            if model.chars().count() > 200 || model.chars().any(char::is_control) {
                return Err(ChainError::BadModel(
                    "must be one line of at most 200 characters",
                ));
            }
        }
    }
    Ok(())
}

/// Checks a list of links before it is stored: [`check_shape`], then no access
/// escalation (a link never gets more access than the agent it replaces).
/// Capability gaps that depend on the TURN are not refused here (a chain may end
/// on a local Ollama on purpose); [`chain_view`] shows them and [`select`] skips
/// such a link when a turn needs what it lacks.
pub fn validate_chain(
    agents: &AgentsConfig,
    from: Option<&AgentType>,
    mode: Option<FallbackMode>,
    chain: &[AgentFallback],
) -> Result<(), ChainError> {
    check_shape(from, mode, chain)?;
    if let Some(from) = from {
        for link in chain {
            if agents.full_access_for(&link.agent) && !agents.full_access_for(from) {
                return Err(ChainError::BroaderAccess {
                    from: from.clone(),
                    to: link.agent.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Checks a row before it is stored: the link itself ([`validate_chain`]), then
/// the chain it would make through the rows of the fallbacks — no loop, not
/// longer than [`MAX_CHAIN_DEPTH`]. Nothing is stored here.
pub fn validate_row(
    agents: &AgentsConfig,
    from: &AgentType,
    mode: FallbackMode,
    link: &AgentFallback,
) -> Result<(), ChainError> {
    validate_chain(agents, Some(from), Some(mode), std::slice::from_ref(link))?;
    let mut trial = agents.clone();
    if !set_row(&mut trial, from, mode, Some(link.clone())) {
        return Err(ChainError::NoCardFor(from.clone()));
    }
    let mut seen = vec![from.clone(), link.agent.clone()];
    let mut tail = (link.agent.clone(), link_mode(link, mode));
    while let Some(next) = row(&trial, &tail.0, tail.1) {
        if seen.contains(&next.agent) {
            let mut path = seen.clone();
            path.push(next.agent.clone());
            return Err(ChainError::Cycle(path));
        }
        seen.push(next.agent.clone());
        if seen.len() - 1 > MAX_CHAIN_DEPTH {
            return Err(ChainError::TooDeep {
                from: Some(from.clone()),
                links: seen.len() - 1,
            });
        }
        tail = (next.agent.clone(), link_mode(next, tail.1));
    }
    Ok(())
}

fn all_agents() -> [AgentType; 10] {
    [
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::OpenCode,
        AgentType::GeminiCli,
        AgentType::Kiro,
        AgentType::Vibe,
        AgentType::CopilotCli,
        AgentType::Ollama,
        AgentType::LiteLlm,
        AgentType::Nvidia,
    ]
}

/// Whether any agent has a fallback at all. Asked before anything costly (the
/// detection of the installed agents) so an install that never configured one
/// pays nothing for the feature.
pub fn is_configured(agents: &AgentsConfig) -> bool {
    all_agents().iter().any(|agent| {
        agents
            .agent_config(agent)
            .is_some_and(|config| !config.mode_fallbacks.is_empty())
    })
}

/// KT-1693 — the agent-level chain of 0.15.3 (`fallback_chain`) is carried onto
/// the text mode rows of that agent: its first link on each of eco, standard and
/// advanced, the next links on the rows of the link before them (so the walk
/// « A → B → C » is still « A → B → C »). An agent's own chain wins where a row
/// is already taken; a link that would be its own row's agent is dropped. The
/// legacy field is emptied. `true` when something was carried (config to save).
pub fn migrate_legacy_chains(agents: &mut AgentsConfig) -> bool {
    let legacy: Vec<(AgentType, Vec<AgentFallback>)> = all_agents()
        .into_iter()
        .filter_map(|agent| {
            let chain = std::mem::take(&mut agents.agent_config_mut(&agent)?.fallback_chain);
            (!chain.is_empty()).then_some((agent, chain))
        })
        .collect();
    if legacy.is_empty() {
        return false;
    }
    let carry = |agents: &mut AgentsConfig, owner: &AgentType, link: &AgentFallback| {
        if link.agent == *owner || link.agent == AgentType::Custom {
            return;
        }
        for mode in FallbackMode::TEXT {
            if row(agents, owner, mode).is_none() {
                let carried = AgentFallback {
                    agent: link.agent.clone(),
                    model: link.model.clone(),
                    mode: None,
                };
                set_row(agents, owner, mode, Some(carried));
            }
        }
    };
    for (agent, chain) in &legacy {
        if let Some(first) = chain.first() {
            carry(agents, agent, first);
        }
    }
    for (_, chain) in &legacy {
        for pair in chain.windows(2) {
            carry(agents, &pair[0].agent, &pair[1]);
        }
    }
    true
}

/// One fallback as the Settings card shows it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct LinkView {
    pub agent: AgentType,
    #[ts(optional)]
    pub model: Option<String>,
    /// The mode of the model it runs on.
    pub mode: FallbackMode,
    pub transport: FallbackTransport,
    /// What a turn could need that this link cannot give, as warnings.
    pub gaps: Vec<FallbackGap>,
}

/// A fallback reached through another agent's row: it is not set here but on
/// `source`'s card (« set on <agent>'s card »).
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct InheritedView {
    pub source: AgentType,
    pub link: LinkView,
}

/// The fallback of one mode row, for display: its own link, the links inherited
/// through the rows of the fallbacks, or the reason it is refused.
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ChainView {
    pub agent: AgentType,
    pub mode: FallbackMode,
    pub links: Vec<LinkView>,
    pub inherited: Vec<InheritedView>,
    #[ts(optional)]
    pub error: Option<String>,
}

pub fn chain_view(agents: &AgentsConfig, from: &AgentType, mode: FallbackMode) -> ChainView {
    let own: Vec<AgentFallback> = row(agents, from, mode).cloned().into_iter().collect();
    chain_view_of(agents, Some(from), mode, &own)
}

fn link_view(
    link: &AgentFallback,
    replaced: FallbackMode,
    owner: Option<&Capabilities>,
) -> LinkView {
    let has = capabilities(&link.agent);
    // A link lacks something only if the agent it replaces had it.
    let lacks =
        |have: bool, owner_has: fn(&Capabilities) -> bool| !have && owner.is_none_or(owner_has);
    let mut gaps = Vec::new();
    if lacks(has.mcp_tools, |c| c.mcp_tools) {
        gaps.push(FallbackGap::NoMcpTools);
    }
    if lacks(has.write_access, |c| c.write_access) {
        gaps.push(FallbackGap::NoWriteAccess);
    }
    if lacks(has.image_input, |c| c.image_input) {
        gaps.push(FallbackGap::NoImageInput);
    }
    if lacks(has.long_context, |c| c.long_context) {
        gaps.push(FallbackGap::CappedContext);
    }
    LinkView {
        agent: link.agent.clone(),
        model: link.model.clone(),
        mode: link_mode(link, replaced),
        transport: transport(&link.agent),
        gaps,
    }
}

/// A fallback, as Settings, a step editor or a room's settings show it. `chain`
/// is what is set here (a row, or an override); the links the rows of the
/// fallbacks add after it come back as `inherited`. Gaps are what the owner
/// agent has and the link lacks (`from` unknown: what an agent type can give).
pub fn chain_view_of(
    agents: &AgentsConfig,
    from: Option<&AgentType>,
    mode: FallbackMode,
    chain: &[AgentFallback],
) -> ChainView {
    let error = validate_chain(agents, from, Some(mode), chain)
        .err()
        .map(|error| error.to_string());
    let owner = from.map(capabilities);
    let mut at = mode;
    let links: Vec<LinkView> = chain
        .iter()
        .map(|link| {
            let view = link_view(link, at, owner.as_ref());
            at = view.mode;
            view
        })
        .collect();
    let mut extended = chain.to_vec();
    let sources = if chain.is_empty() {
        Vec::new()
    } else {
        extend_through_rows(agents, from, mode, &mut extended)
    };
    let mut at = links.last().map_or(mode, |link| link.mode);
    let inherited = extended
        .iter()
        .skip(chain.len())
        .zip(sources)
        .map(|(link, source)| {
            let link = link_view(link, at, owner.as_ref());
            at = link.mode;
            InheritedView { source, link }
        })
        .collect();
    ChainView {
        agent: from.cloned().unwrap_or(AgentType::ClaudeCode),
        mode,
        links,
        inherited,
        error,
    }
}

/// What Settings > Agents shows: every mode row that has a fallback, and who is
/// on cooldown.
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct FallbackOverview {
    pub chains: Vec<ChainView>,
    pub cooldowns: Vec<Cooldown>,
    /// KT-1706 — the rows of the named HTTP connections and the image and video rows of ComfyUI.
    pub media: Vec<crate::core::connection_fallback::ConnectionChainView>,
}

/// One view per mode row that has a fallback (the rows without one have none to
/// show), plus the cooldowns still running.
pub fn overview(agents: &AgentsConfig, cooldowns: Vec<Cooldown>) -> FallbackOverview {
    FallbackOverview {
        chains: all_agents()
            .iter()
            .flat_map(|agent| {
                FallbackMode::ALL
                    .into_iter()
                    .filter(|mode| row(agents, agent, *mode).is_some())
                    .map(|mode| chain_view(agents, agent, mode))
                    .collect::<Vec<_>>()
            })
            .collect(),
        cooldowns,
        media: crate::core::connection_fallback::overview(agents),
    }
}

// ─── Choosing the next link ──────────────────────────────────────────────

/// The outcome of [`select`].
#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    /// Continue on this link.
    Switch(AgentFallback),
    /// Nothing can take over. `why` is shown in the room.
    Stop { why: String },
}

/// The next usable link of `chain` once `failing` has failed. `origin` is the
/// agent the turn started on; `usable` is the agents installed and enabled;
/// `cooling` says until when an agent is left alone, if it is. After a hop the
/// walk goes on past the link that failed; an agent the walk is already on or
/// started on is never offered again.
pub fn select(
    agents: &AgentsConfig,
    chain: &[AgentFallback],
    origin: &AgentType,
    failing: &AgentType,
    usable: &[AgentType],
    needs: &TurnNeeds,
    cooling: &dyn Fn(&AgentType) -> Option<DateTime<Utc>>,
) -> Selection {
    if let Err(error) = validate_chain(agents, None, None, chain) {
        return Selection::Stop {
            why: error.to_string(),
        };
    }
    if chain.is_empty() {
        return Selection::Stop {
            why: format!("{} has no fallback configured.", display_name(origin)),
        };
    }
    let rest = match chain.iter().position(|link| link.agent == *failing) {
        Some(at) => &chain[at + 1..],
        None => chain,
    };
    let mut skipped: Vec<String> = Vec::new();
    let mut tried: HashSet<String> = HashSet::new();
    for link in rest {
        let name = display_name(&link.agent);
        if link.agent == *origin || link.agent == *failing || !tried.insert(name.to_string()) {
            continue;
        }
        if let Some(until) = cooling(&link.agent) {
            skipped.push(format!(
                "{name} is itself unavailable until {}",
                stamp(until)
            ));
            continue;
        }
        if !crate::workflows::step_agents::agent_can_launch(&link.agent, usable, agents) {
            skipped.push(format!("{name} is not installed or not enabled"));
            continue;
        }
        if agents.full_access_for(&link.agent) && !agents.full_access_for(origin) {
            skipped.push(format!(
                "{name} has more access than {}",
                display_name(origin)
            ));
            continue;
        }
        if let Some(gap) = capability_gap(&link.agent, needs) {
            skipped.push(format!("{name} cannot take this turn: {gap}"));
            continue;
        }
        if needs.task_worker {
            if let Some(refusal) = task_worker_launch_refusal(&link.agent) {
                skipped.push(format!("{name} cannot take this turn: {refusal}"));
                continue;
            }
        }
        return Selection::Switch(link.clone());
    }
    let why = if skipped.is_empty() {
        "the chain has no link left".to_string()
    } else {
        skipped.join("; ")
    };
    Selection::Stop {
        why: format!(
            "No fallback could take over from {}: {why}.",
            display_name(failing)
        ),
    }
}

/// `14:00 UTC`, the way the room says when an agent is back.
pub fn stamp(at: DateTime<Utc>) -> String {
    at.format("%H:%M UTC").to_string()
}

/// « Codex · gpt-5.6-sol: quota exhausted until 14:00 UTC → continued with
/// Claude Code · sonnet [CLI] ». The model is the link's own, else the mode it
/// keeps (`eco mode`); the badge is how the fallback is reached.
pub fn notice(
    failing: &AgentType,
    from_model: Option<&str>,
    trigger: Trigger,
    until: Option<DateTime<Utc>>,
    link: &AgentFallback,
    tier: ModelTier,
) -> String {
    let mode = FallbackMode::from_tier(tier);
    let until = until
        .map(|at| format!(" until {}", stamp(at)))
        .unwrap_or_default();
    let model = link
        .model
        .clone()
        .unwrap_or_else(|| format!("{} mode", link_mode(link, mode).label()));
    let from = from_model
        .map(str::to_string)
        .unwrap_or_else(|| format!("{} mode", mode.label()));
    format!(
        "{} · {from}: {}{until} → continued with {} · {model} [{}]",
        display_name(failing),
        trigger.reason(),
        display_name(&link.agent),
        transport(&link.agent).label(),
    )
}

/// The room line when no link can take over.
pub fn stop_notice(failing: &AgentType, trigger: Trigger, why: &str) -> String {
    format!(
        "{}: {} → no fallback ({why})",
        display_name(failing),
        trigger.reason()
    )
}

/// [`stop_notice`] in the room's language (the detail `why` is the policy's own
/// wording and stays as it is).
pub fn stop_notice_in(
    failing: &AgentType,
    trigger: Trigger,
    why: &str,
    lang: crate::core::room_lang::RoomLang,
) -> String {
    let reason = match trigger {
        Trigger::RateLimit => lang.pick("limite de débit atteinte", trigger.reason()),
        Trigger::QuotaExhausted => lang.pick("quota épuisé", trigger.reason()),
        Trigger::Outage => lang.pick("fournisseur indisponible", trigger.reason()),
        Trigger::NotAuthenticated => lang.pick("non authentifié", trigger.reason()),
        Trigger::LaunchRefused => lang.pick(
            "ne peut pas être lancé comme worker de tâche",
            trigger.reason(),
        ),
        Trigger::LaunchUnavailable => lang.pick("indisponible au lancement", trigger.reason()),
    };
    let none = lang.pick("aucun agent de repli", "no fallback");
    format!("{}: {reason} → {none} ({why})", display_name(failing))
}

// ─── Cooldown ────────────────────────────────────────────────────────────

/// An agent left alone until `until`: new turns go straight to its fallback.
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct Cooldown {
    pub agent: AgentType,
    pub until: DateTime<Utc>,
    /// A [`Trigger::code`].
    pub reason: String,
}

fn key(agent: &AgentType) -> String {
    crate::db::orchestration::agent_type_to_db(agent)
}

/// Starts (or renews) the cooldown of `agent`. `until` is the provider's
/// announced reset, else the trigger's re-try window from `now`. Returns the
/// instant stored.
pub fn set_cooldown(
    conn: &Connection,
    agent: &AgentType,
    trigger: Trigger,
    announced: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> anyhow::Result<DateTime<Utc>> {
    let until = announced
        .filter(|at| *at > now)
        .unwrap_or_else(|| now + trigger.default_cooldown());
    conn.execute(
        "INSERT INTO agent_fallback_cooldowns (agent, until, reason, noted_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(agent) DO UPDATE SET until = excluded.until,
             reason = excluded.reason, noted_at = excluded.noted_at",
        params![
            key(agent),
            until.to_rfc3339(),
            trigger.code(),
            now.to_rfc3339()
        ],
    )?;
    Ok(until)
}

/// Until when `agent` is left alone, if it still is. An expired row is not one:
/// the agent is back in play without anything to clean up.
pub fn active_cooldown(
    conn: &Connection,
    agent: &AgentType,
    now: DateTime<Utc>,
) -> anyhow::Result<Option<Cooldown>> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT until, reason FROM agent_fallback_cooldowns WHERE agent = ?1",
            [key(agent)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(row.and_then(|(until, reason)| {
        let until = DateTime::parse_from_rfc3339(&until)
            .ok()?
            .with_timezone(&Utc);
        (until > now).then(|| Cooldown {
            agent: agent.clone(),
            until,
            reason,
        })
    }))
}

/// Every cooldown still running, for the agent cards.
pub fn list_cooldowns(conn: &Connection, now: DateTime<Utc>) -> anyhow::Result<Vec<Cooldown>> {
    let mut statement =
        conn.prepare("SELECT agent, until, reason FROM agent_fallback_cooldowns")?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(agent, until, reason)| {
            let agent = crate::db::orchestration::agent_type_from_db(&agent).ok()?;
            let until = DateTime::parse_from_rfc3339(&until)
                .ok()?
                .with_timezone(&Utc);
            (until > now).then_some(Cooldown {
                agent,
                until,
                reason,
            })
        })
        .collect())
}

/// The provider answers again (a re-check, a login, a new key): the agent is
/// back in play now, not when the window would have run out.
pub fn clear_cooldown(conn: &Connection, agent: &AgentType) -> anyhow::Result<()> {
    conn.execute(
        "DELETE FROM agent_fallback_cooldowns WHERE agent = ?1",
        [key(agent)],
    )?;
    Ok(())
}

/// An account changed (a new key, another login): no agent is held back on the
/// strength of the old one.
pub fn clear_all_cooldowns(conn: &Connection) -> anyhow::Result<()> {
    conn.execute("DELETE FROM agent_fallback_cooldowns", [])?;
    Ok(())
}

/// The agent a new turn should start on: `agent` itself, or — while it is on
/// cooldown and a usable link of `chain` exists — the link that takes over for
/// it, so the room does not spend a turn on a provider that just refused.
/// `None`: start on `agent` as usual.
pub fn preferred_start(
    conn: &Connection,
    agents: &AgentsConfig,
    chain: &[AgentFallback],
    agent: &AgentType,
    usable: &[AgentType],
    needs: &TurnNeeds,
    now: DateTime<Utc>,
) -> anyhow::Result<Option<(AgentFallback, Cooldown)>> {
    let Some(cooldown) = active_cooldown(conn, agent, now)? else {
        return Ok(None);
    };
    let cooling = cooling_lookup(conn, now)?;
    let lookup = |candidate: &AgentType| {
        cooling
            .iter()
            .find(|(agent, _)| agent == candidate)
            .map(|(_, until)| *until)
    };
    Ok(
        match select(agents, chain, agent, agent, usable, needs, &lookup) {
            Selection::Switch(link) => Some((link, cooldown)),
            Selection::Stop { .. } => None,
        },
    )
}

/// The cooldowns running, as pairs `select`'s lookup can read.
pub fn cooling_lookup(
    conn: &Connection,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<(AgentType, DateTime<Utc>)>> {
    Ok(list_cooldowns(conn, now)?
        .into_iter()
        .map(|cooldown| (cooldown.agent, cooldown.until))
        .collect())
}

// ─── Overrides and origins ───────────────────────────────────────────────

/// A discussion's own fallback for one mode.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ModeOverride {
    pub mode: FallbackMode,
    /// The links that replace the default of the mode; empty is "no fallback".
    pub chain: Vec<AgentFallback>,
}

/// The override of `mode` among a discussion's: `None` = the default applies.
pub fn override_for(
    overrides: Option<&[ModeOverride]>,
    mode: FallbackMode,
) -> Option<&[AgentFallback]> {
    overrides?
        .iter()
        .find(|entry| entry.mode == mode)
        .map(|entry| entry.chain.as_slice())
}

/// Reads what a row stores: per-mode overrides, or the single list 0.15.3
/// stored (a chain for the whole discussion), which becomes the same override
/// on each text mode — an empty one cuts them.
fn parse_overrides(json: &str) -> anyhow::Result<Vec<ModeOverride>> {
    if let Ok(per_mode) = serde_json::from_str::<Vec<ModeOverride>>(json) {
        if !per_mode.is_empty() {
            return Ok(per_mode);
        }
    }
    let legacy = serde_json::from_str::<Vec<AgentFallback>>(json)?;
    Ok(FallbackMode::TEXT
        .into_iter()
        .map(|mode| ModeOverride {
            mode,
            chain: legacy.clone(),
        })
        .collect())
}

/// What a discussion overrides, per mode: `None` = nothing is overridden.
pub fn get_discussion_override(
    conn: &Connection,
    discussion_id: &str,
) -> anyhow::Result<Option<Vec<ModeOverride>>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT chain_json FROM discussion_fallback_overrides WHERE discussion_id = ?1",
            [discussion_id],
            |row| row.get(0),
        )
        .optional()?;
    json.map(|json| parse_overrides(&json)).transpose()
}

/// Stores (`Some`) or removes (`None`) a discussion's override of one mode; the
/// other modes' overrides stay.
pub fn set_discussion_override(
    conn: &Connection,
    discussion_id: &str,
    mode: FallbackMode,
    chain: Option<&[AgentFallback]>,
) -> anyhow::Result<()> {
    let mut overrides = get_discussion_override(conn, discussion_id)?.unwrap_or_default();
    overrides.retain(|entry| entry.mode != mode);
    if let Some(chain) = chain {
        overrides.push(ModeOverride {
            mode,
            chain: chain.to_vec(),
        });
    }
    overrides.sort_by_key(|entry| entry.mode);
    if overrides.is_empty() {
        conn.execute(
            "DELETE FROM discussion_fallback_overrides WHERE discussion_id = ?1",
            [discussion_id],
        )?;
        return Ok(());
    }
    conn.execute(
        "INSERT INTO discussion_fallback_overrides (discussion_id, chain_json, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(discussion_id) DO UPDATE SET chain_json = excluded.chain_json,
             updated_at = excluded.updated_at",
        params![
            discussion_id,
            serde_json::to_string(&overrides)?,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

/// The agent a task worker started on, if it has already moved once.
pub fn get_execution_origin(
    conn: &Connection,
    execution_id: &str,
) -> anyhow::Result<Option<AgentType>> {
    let origin: Option<String> = conn
        .query_row(
            "SELECT origin_agent FROM execution_fallback_origins WHERE execution_id = ?1",
            [execution_id],
            |row| row.get(0),
        )
        .optional()?;
    origin
        .map(|origin| crate::db::orchestration::agent_type_from_db(&origin))
        .transpose()
}

/// Remembers where a worker started; the first call wins.
pub fn note_execution_origin(
    conn: &Connection,
    execution_id: &str,
    origin: &AgentType,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO execution_fallback_origins (execution_id, origin_agent)
         VALUES (?1, ?2)",
        params![execution_id, key(origin)],
    )?;
    Ok(())
}

/// KT-1758 — forgets where a worker started: a person's choice of worker (a
/// native reassignment or a CLI session) ends the fallback, and the chain a later
/// failure walks is the new worker's own. Kronn's own return to the original
/// worker does not clear it: the origin is then the current worker, which is how
/// the launch path reads "not a fallback" (`worker_launch_context`).
pub fn clear_execution_origin(conn: &Connection, execution_id: &str) -> anyhow::Result<()> {
    conn.execute(
        "DELETE FROM execution_fallback_origins WHERE execution_id = ?1",
        [execution_id],
    )?;
    Ok(())
}

// ─── A dispatch continued on a fallback ──────────────────────────────────

/// The decision a discussion turn runs on: which agent, which model, which
/// tier, how many hops so far.
#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub from_agent: AgentType,
    pub to_agent: AgentType,
    pub model: Option<String>,
    pub tier: ModelTier,
    pub hops: u32,
}

pub fn set_pin(
    conn: &Connection,
    dispatch_job_id: &str,
    pin: &Pin,
    reason: Trigger,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO dispatch_fallback_pins
             (dispatch_job_id, from_agent, to_agent, model, tier, hops, reason, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(dispatch_job_id) DO UPDATE SET from_agent = excluded.from_agent,
             to_agent = excluded.to_agent, model = excluded.model, tier = excluded.tier,
             hops = excluded.hops, reason = excluded.reason, created_at = excluded.created_at",
        params![
            dispatch_job_id,
            key(&pin.from_agent),
            key(&pin.to_agent),
            pin.model,
            pin.tier.as_str(),
            pin.hops,
            reason.code(),
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn get_pin(conn: &Connection, dispatch_job_id: &str) -> anyhow::Result<Option<Pin>> {
    let row: Option<(String, String, Option<String>, String, u32)> = conn
        .query_row(
            "SELECT from_agent, to_agent, model, tier, hops FROM dispatch_fallback_pins
             WHERE dispatch_job_id = ?1",
            [dispatch_job_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    row.map(|(from, to, model, tier, hops)| {
        Ok(Pin {
            from_agent: crate::db::orchestration::agent_type_from_db(&from)?,
            to_agent: crate::db::orchestration::agent_type_from_db(&to)?,
            model,
            tier: ModelTier::parse(&tier).unwrap_or_default(),
            hops,
        })
    })
    .transpose()
}

fn migrate_config(config: &mut crate::models::AppConfig) -> bool {
    migrate_legacy_chains(&mut config.agents)
}
inventory::submit! { crate::core::uplink::ConfigMigration(migrate_config) }

#[cfg(all(test, feature = "full-tests"))]
#[path = "agent_fallback_test.rs"]
mod tests;
