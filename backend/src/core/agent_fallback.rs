//! KT-1518 — the agent fallback chain.
//!
//! Each agent has an ordered chain of agents that take over, one after the
//! other, when the PROVIDER cannot serve it (`AgentConfig::fallback_chain`, e.g.
//! Claude Code → Codex → Ollama): a rate limit, a quota or credits spent, an
//! outage that survived its retries, a CLI that is not authenticated. Never a
//! task failure, a refusal, a test failure or the timeout of the work itself:
//! [`classify_trigger`] answers `None` for those, and every surface asks it
//! before it switches anything.
//!
//! This module is the policy and nothing else — no spawning, no rooms. The three
//! surfaces (a discussion turn, a workflow Agent step, a task worker) each ask
//! [`select`] for the next link and apply it their own way, so a link is chosen
//! by the same rules everywhere:
//!
//! * the chain is the list of the agent the turn STARTED on (its default one, or
//!   the override of a discussion / a workflow step; an empty override is "no
//!   fallback"), at most [`MAX_CHAIN_DEPTH`] links, an agent never twice and
//!   never itself (validated when it is saved, re-checked here); after a hop the
//!   walk goes on in the same list, past the link that just failed;
//! * a link on cooldown, not installed, billed by API key without the owner's
//!   opt-in, or lacking something the turn needs (MCP tools, workspace writes,
//!   image input, a long context) is skipped, and the walk goes on;
//! * nothing usable ends in a clear message, not a silent stop.
//!
//! The same TIER is kept (economy stays economy): a fallback carries the tier of
//! the interrupted turn and resolves it on its own agent. A model name is
//! provider-specific, so the failing agent's model is never carried over; only
//! the link's own explicit `model` replaces the tier.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{AgentFallback, AgentType, AgentsConfig, ModelTier};

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
}

impl Trigger {
    pub fn code(self) -> &'static str {
        match self {
            Trigger::RateLimit => "rate_limit",
            Trigger::QuotaExhausted => "quota_exhausted",
            Trigger::Outage => "outage",
            Trigger::NotAuthenticated => "not_authenticated",
        }
    }

    /// The words the room uses for the reason.
    pub fn reason(self) -> &'static str {
        match self {
            Trigger::RateLimit => "rate limited",
            Trigger::QuotaExhausted => "quota exhausted",
            Trigger::Outage => "provider unavailable",
            Trigger::NotAuthenticated => "not authenticated",
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
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [
            Trigger::RateLimit,
            Trigger::QuotaExhausted,
            Trigger::Outage,
            Trigger::NotAuthenticated,
        ]
        .into_iter()
        .find(|trigger| trigger.code() == code)
    }
}

/// Which provider-side failure `error` is, if it is one. `None` for everything
/// else — in particular the stall or timeout of the work itself, which says
/// nothing about the provider: a fallback would only repeat the same wait.
pub fn classify_trigger(error: &str) -> Option<Trigger> {
    use crate::api::discussions::orchestration::is_hard_quota_exhausted;
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
    pub image_input: bool,
    pub long_context: bool,
}

/// `docs/architecture/http-agent-capabilities.md`: a CLI agent runs as a local
/// process with a filesystem, a shell and the MCP bridge; an HTTP agent only has
/// the tools Kronn executes for it (workspace writes included, no MCP). An image
/// reaches a CLI agent as a path it opens itself; an HTTP model sees it only when
/// that model is known to (`agents/vision.rs` treats "unknown" as "no"), so an
/// HTTP agent is not counted on for images. A local Ollama window is capped.
pub fn capabilities(agent: &AgentType) -> Capabilities {
    let http = crate::agents::runner::is_http_chat_agent(agent);
    Capabilities {
        mcp_tools: !http,
        write_access: !matches!(agent, AgentType::Custom),
        image_input: !http,
        long_context: !matches!(agent, AgentType::Ollama),
    }
}

/// The first thing `needs` asks that `agent` cannot give.
pub fn capability_gap(agent: &AgentType, needs: &TurnNeeds) -> Option<&'static str> {
    let has = capabilities(agent);
    if needs.mcp_tools && !has.mcp_tools {
        return Some("it has no MCP tools");
    }
    if needs.write_access && !has.write_access {
        return Some("it cannot write in the worktree");
    }
    if needs.image_input && !has.image_input {
        return Some("it cannot be counted on to read images");
    }
    if needs.long_context && !has.long_context {
        return Some("its context window is too small for this transcript");
    }
    None
}

/// Billed per call by API key, so never a surprise: the owner opts in per link.
/// Subscription CLIs and a local Ollama are not.
pub fn is_paid(agent: &AgentType) -> bool {
    matches!(
        agent,
        AgentType::LiteLlm | AgentType::Nvidia | AgentType::Custom
    )
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

/// Why a chain, or a link being added to it, is refused.
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
        }
    }
}

impl std::error::Error for ChainError {}

/// The default chain of `from`, as stored.
pub fn default_chain<'a>(agents: &'a AgentsConfig, from: &AgentType) -> &'a [AgentFallback] {
    agents
        .agent_config(from)
        .map(|config| config.fallback_chain.as_slice())
        .unwrap_or(&[])
}

/// The chain a turn that starts on `origin` follows: the override when there is
/// one (an empty override is "no fallback"), else the agent's default chain.
pub fn effective_chain(
    agents: &AgentsConfig,
    origin: &AgentType,
    chain_override: Option<&[AgentFallback]>,
) -> Vec<AgentFallback> {
    match chain_override {
        Some(chain) => chain.to_vec(),
        None => default_chain(agents, origin).to_vec(),
    }
}

/// What is wrong with a chain whatever the configuration: too long, the
/// starting agent or one agent twice in it (a loop), an agent with no card (a
/// named connection), a bad model. `from` is the agent the chain belongs to
/// (`None` for an override that is not tied to one agent, as a discussion's is).
pub fn check_shape(from: Option<&AgentType>, chain: &[AgentFallback]) -> Result<(), ChainError> {
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

/// Checks a whole chain before it is stored: [`check_shape`], then no access
/// escalation (a link never gets more access than the agent it replaces).
/// Capability gaps that depend on the TURN are not refused here (a chain may end
/// on a local Ollama on purpose); [`chain_view`] shows them and [`select`] skips
/// such a link when a turn needs what it lacks.
pub fn validate_chain(
    agents: &AgentsConfig,
    from: Option<&AgentType>,
    chain: &[AgentFallback],
) -> Result<(), ChainError> {
    check_shape(from, chain)?;
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
            .is_some_and(|config| !config.fallback_chain.is_empty())
    })
}

/// One link as the Settings card shows it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ChainLinkView {
    pub agent: AgentType,
    #[ts(optional)]
    pub model: Option<String>,
    pub paid: bool,
    pub allow_paid: bool,
    /// What a turn could need that this link cannot give, as warnings.
    pub gaps: Vec<String>,
}

/// The chain of one agent, for display: its links with their gaps, or the
/// reason it is refused.
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ChainView {
    pub agent: AgentType,
    pub links: Vec<ChainLinkView>,
    #[ts(optional)]
    pub error: Option<String>,
}

pub fn chain_view(agents: &AgentsConfig, from: &AgentType) -> ChainView {
    chain_view_of(agents, Some(from), default_chain(agents, from))
}

/// A chain, as Settings, a step editor or a room's settings show it. Gaps are
/// what the owner agent has and the link lacks (`from` unknown: what an agent
/// type can give at all).
pub fn chain_view_of(
    agents: &AgentsConfig,
    from: Option<&AgentType>,
    chain: &[AgentFallback],
) -> ChainView {
    let error = validate_chain(agents, from, chain)
        .err()
        .map(|error| error.to_string());
    let owner = from.map(capabilities);
    let views = chain
        .iter()
        .map(|link| {
            let has = capabilities(&link.agent);
            // A link lacks something only if the agent it replaces had it.
            let lacks = |have: bool, owner_has: fn(&Capabilities) -> bool| {
                !have && owner.as_ref().is_none_or(owner_has)
            };
            let mut gaps = Vec::new();
            if lacks(has.mcp_tools, |c| c.mcp_tools) {
                gaps.push("no MCP tools".to_string());
            }
            if lacks(has.write_access, |c| c.write_access) {
                gaps.push("cannot write in the worktree".to_string());
            }
            if lacks(has.image_input, |c| c.image_input) {
                gaps.push("images are not guaranteed".to_string());
            }
            if lacks(has.long_context, |c| c.long_context) {
                gaps.push("capped context window".to_string());
            }
            ChainLinkView {
                paid: is_paid(&link.agent),
                allow_paid: link.allow_paid,
                model: link.model.clone(),
                agent: link.agent.clone(),
                gaps,
            }
        })
        .collect();
    ChainView {
        agent: from.cloned().unwrap_or(AgentType::ClaudeCode),
        links: views,
        error,
    }
}

/// What Settings > Agents shows: every chain, and who is on cooldown.
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct FallbackOverview {
    pub chains: Vec<ChainView>,
    pub cooldowns: Vec<Cooldown>,
}

/// One chain per agent that has a fallback (the agents without one have no
/// chain to show), plus the cooldowns still running.
pub fn overview(agents: &AgentsConfig, cooldowns: Vec<Cooldown>) -> FallbackOverview {
    FallbackOverview {
        chains: all_agents()
            .iter()
            .filter(|agent| {
                agents
                    .agent_config(agent)
                    .is_some_and(|config| !config.fallback_chain.is_empty())
            })
            .map(|agent| chain_view(agents, agent))
            .collect(),
        cooldowns,
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
    if let Err(error) = validate_chain(agents, None, chain) {
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
        if is_paid(&link.agent) && !link.allow_paid {
            skipped.push(format!(
                "{name} bills by API key and its use as a fallback was not allowed"
            ));
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

/// « Codex: quota exhausted until 14:00 UTC -> continued with Claude Code
/// (sonnet) ». The model is the link's own, else the tier it keeps.
pub fn notice(
    failing: &AgentType,
    from_model: Option<&str>,
    trigger: Trigger,
    until: Option<DateTime<Utc>>,
    link: &AgentFallback,
    tier: ModelTier,
) -> String {
    let until = until
        .map(|at| format!(" until {}", stamp(at)))
        .unwrap_or_default();
    let model = link
        .model
        .clone()
        .unwrap_or_else(|| format!("{} tier", tier.as_str()));
    let from = from_model
        .map(|model| format!("{} ({model})", display_name(failing)))
        .unwrap_or_else(|| format!("{} ({} tier)", display_name(failing), tier.as_str()));
    format!(
        "{from}: {}{until} -> continued with {} ({model})",
        trigger.reason(),
        display_name(&link.agent),
    )
}

/// The room line when no link can take over.
pub fn stop_notice(failing: &AgentType, trigger: Trigger, why: &str) -> String {
    format!(
        "{}: {} -> no fallback ({why})",
        display_name(failing),
        trigger.reason()
    )
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

/// The chain a discussion overrides the default with: `None` = no override, an
/// empty list = no fallback in this discussion.
pub fn get_discussion_override(
    conn: &Connection,
    discussion_id: &str,
) -> anyhow::Result<Option<Vec<AgentFallback>>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT chain_json FROM discussion_fallback_overrides WHERE discussion_id = ?1",
            [discussion_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(json
        .map(|json| serde_json::from_str::<Vec<AgentFallback>>(&json))
        .transpose()?)
}

/// Stores (`Some`) or removes (`None`) a discussion's override.
pub fn set_discussion_override(
    conn: &Connection,
    discussion_id: &str,
    chain: Option<&[AgentFallback]>,
) -> anyhow::Result<()> {
    match chain {
        Some(chain) => {
            conn.execute(
                "INSERT INTO discussion_fallback_overrides (discussion_id, chain_json, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(discussion_id) DO UPDATE SET chain_json = excluded.chain_json,
                     updated_at = excluded.updated_at",
                params![
                    discussion_id,
                    serde_json::to_string(chain)?,
                    Utc::now().to_rfc3339()
                ],
            )?;
        }
        None => {
            conn.execute(
                "DELETE FROM discussion_fallback_overrides WHERE discussion_id = ?1",
                [discussion_id],
            )?;
        }
    }
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

#[cfg(all(test, feature = "full-tests"))]
#[path = "agent_fallback_test.rs"]
mod tests;
