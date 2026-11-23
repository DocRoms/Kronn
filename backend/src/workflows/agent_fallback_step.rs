//! KT-1518 — an Agent step whose provider cannot serve it continues on the
//! agent's fallback chain instead of failing or parking the run.
//!
//! The runner asks this module twice around the step's execution:
//!
//! * [`start_on_fallback`] before the first launch: an agent that is on cooldown
//!   (its provider refused a turn a moment ago) is skipped, so the step does not
//!   spend a launch on a provider known to refuse;
//! * [`continue_after`] after a launch that failed: when the failure is the
//!   provider's (never the work's), the step is rerun on the next usable link.
//!
//! The policy is `core::agent_fallback`; this module only applies it to a step.
//! The step keeps its tier, budget and prompt; its model is the link's own or
//! none (a model name belongs to one provider), see `step_agents::apply_to_step`.

use chrono::Utc;

use crate::core::agent_fallback::{self as fallback, Selection, Trigger, TurnNeeds};
use crate::models::{AgentType, AgentsConfig, StepAgentOverride, StepType, WorkflowStep};
use crate::AppState;

use super::steps::StepOutcome;

/// What the runner does next with a step that was launched.
pub enum Next {
    /// Done: the outcome stands (it succeeded, or the failure is not the
    /// provider's, or no fallback is configured).
    Keep,
    /// Rerun on this step; `notice` is what the room says about the switch.
    Rerun {
        step: Box<WorkflowStep>,
        notice: String,
    },
    /// The provider failed, a chain is configured and nothing in it can take
    /// over: the outcome stands, and `notice` says why.
    Stopped { notice: String },
}

/// What a step needs from whoever runs it.
fn needs_of(step: &WorkflowStep, agents: &AgentsConfig) -> TurnNeeds {
    TurnNeeds {
        mcp_tools: step
            .agent_settings
            .as_ref()
            .is_some_and(|settings| settings.tools.is_some()),
        // A step run by an agent that was granted full access is one that may
        // write; a read-only agent's step is not asked to.
        write_access: agents.full_access_for(&step.agent),
        task_worker: false,
        shell: false,
        image_input: false,
        long_context: false,
    }
}

/// The agents installed and enabled, detected only when a chain exists (the
/// detection is the costly part); a test gives its own.
async fn usable_agents(state: &AppState, given: Option<Vec<AgentType>>) -> Vec<AgentType> {
    match given {
        Some(usable) => usable,
        None => crate::api::orchestration::available_agent_types_for_state(state).await,
    }
}

/// The chain `step` follows when it started on `origin`: its own override, else
/// the agent's default chain.
fn chain_of(
    step: &WorkflowStep,
    agents: &AgentsConfig,
    origin: &AgentType,
) -> Vec<crate::models::AgentFallback> {
    fallback::effective_chain(
        agents,
        origin,
        crate::models::FallbackMode::from_tier(tier_of(step)),
        step.agent_settings
            .as_ref()
            .and_then(|settings| settings.fallback.as_deref()),
    )
}

fn model_of(step: &WorkflowStep) -> Option<&str> {
    step.agent_settings
        .as_ref()
        .and_then(|settings| settings.model.as_deref())
}

/// The link to continue on, applied to a copy of `step`. A step that declares
/// tools the link cannot use is not moved: the declaration is the author's.
fn apply(step: &WorkflowStep, link: &crate::models::AgentFallback) -> Result<WorkflowStep, String> {
    if let Some(tools) = step.agent_settings.as_ref().and_then(|s| s.tools.as_ref()) {
        tools.validate(&link.agent).map_err(|error| {
            format!(
                "step `{}` declares tools {} cannot use ({error})",
                step.name,
                fallback::display_name(&link.agent)
            )
        })?;
    }
    let mut next = step.clone();
    super::step_agents::apply_to_step(
        &mut next,
        &StepAgentOverride {
            agent: link.agent.clone(),
            model: link.model.clone(),
            reasoning_effort: None,
        },
    );
    Ok(next)
}

fn tier_of(step: &WorkflowStep) -> crate::models::ModelTier {
    step.agent_settings
        .as_ref()
        .and_then(|settings| settings.tier)
        .unwrap_or_default()
}

/// Before the first launch: `step` itself, or its copy on the fallback while
/// its agent is on cooldown. The notice is for the step's output.
pub async fn start_on_fallback(
    state: &AppState,
    agents: &AgentsConfig,
    step: &WorkflowStep,
) -> Option<(WorkflowStep, String)> {
    start_on_fallback_among(state, agents, step, None).await
}

pub async fn start_on_fallback_among(
    state: &AppState,
    agents: &AgentsConfig,
    step: &WorkflowStep,
    usable: Option<Vec<AgentType>>,
) -> Option<(WorkflowStep, String)> {
    if step.step_type != StepType::Agent || step.agent == AgentType::Custom {
        return None;
    }
    let chain = chain_of(step, agents, &step.agent);
    if chain.is_empty() {
        return None;
    }
    // Cheap exit: nothing is on cooldown, the step starts as written.
    let now = Utc::now();
    let any_cooldown = state
        .db
        .with_read_conn(move |conn| fallback::list_cooldowns(conn, now))
        .await
        .map(|cooldowns| !cooldowns.is_empty())
        .unwrap_or(false);
    if !any_cooldown {
        return None;
    }
    let usable = usable_agents(state, usable).await;
    let needs = needs_of(step, agents);
    let config = agents.clone();
    let agent = step.agent.clone();
    let preferred = state
        .db
        .with_read_conn(move |conn| {
            fallback::preferred_start(conn, &config, &chain, &agent, &usable, &needs, now)
        })
        .await
        .ok()
        .flatten()?;
    let (link, cooldown) = preferred;
    let next = apply(step, &link).ok()?;
    let trigger = Trigger::from_code(&cooldown.reason).unwrap_or(Trigger::QuotaExhausted);
    let notice = fallback::notice(
        &step.agent,
        model_of(step),
        trigger,
        Some(cooldown.until),
        &link,
        tier_of(step),
    );
    Some((next, notice))
}

/// After a launch: what to do with `outcome`. `origin` is the agent the step
/// started on (its chain is the one walked), `step` what was just launched.
pub async fn continue_after(
    state: &AppState,
    agents: &AgentsConfig,
    origin: &AgentType,
    step: &WorkflowStep,
    outcome: &StepOutcome,
) -> Next {
    continue_after_among(state, agents, origin, step, outcome, None).await
}

pub async fn continue_after_among(
    state: &AppState,
    agents: &AgentsConfig,
    origin: &AgentType,
    step: &WorkflowStep,
    outcome: &StepOutcome,
    usable: Option<Vec<AgentType>>,
) -> Next {
    use crate::models::RunStatus;
    if step.step_type != StepType::Agent || outcome.result.status != RunStatus::Failed {
        return Next::Keep;
    }
    // A failure is the provider's only when its own words say so; the step's
    // output of a failed run is the last error.
    let Some(trigger) = fallback::classify_trigger(&outcome.result.output) else {
        return Next::Keep;
    };
    if let Some(settings) = step
        .agent_settings
        .as_ref()
        .filter(|settings| settings.connection_id.is_some())
    {
        // An explicit per-step override (including off) must win over the card.
        if settings.fallback.is_some() {
            return Next::Keep;
        }
        let owner = settings.connection_id.as_deref().unwrap();
        let mode = crate::models::FallbackMode::from_tier(tier_of(step));
        if let Some((connection, model)) =
            crate::core::connection_fallback::next_text_target(state, owner, mode).await
        {
            let agent =
                crate::db::external_api_connections::target_for_connection(&connection).agent_type;
            if settings
                .tools
                .as_ref()
                .is_some_and(|tools| tools.validate(&agent).is_err())
            {
                return Next::Keep;
            }
            let mut next = step.clone();
            next.agent = agent;
            let settings = next.agent_settings.as_mut().unwrap();
            settings.connection_id = Some(connection.id.clone());
            settings.model = Some(model);
            settings.reasoning_effort = None;
            return Next::Rerun {
                step: Box::new(next),
                notice: format!(
                    "{} could not serve this step; continued on {}",
                    owner, connection.display_name
                ),
            };
        }
        return Next::Keep;
    }
    let chain = chain_of(step, agents, origin);
    if chain.is_empty() {
        return Next::Keep;
    }
    let usable = usable_agents(state, usable).await;
    let now = Utc::now();
    let announced = fallback::announced_reset(&outcome.result.output, now);
    let failing = step.agent.clone();
    let origin = origin.clone();
    let needs = needs_of(step, agents);
    let config = agents.clone();
    let selected = state
        .db
        .with_conn(move |conn| {
            let until = fallback::set_cooldown(conn, &failing, trigger, announced, now)?;
            let cooling = fallback::cooling_lookup(conn, now)?;
            let lookup = |candidate: &AgentType| {
                cooling
                    .iter()
                    .find(|(agent, _)| agent == candidate)
                    .map(|(_, until)| *until)
            };
            Ok((
                until,
                fallback::select(&config, &chain, &origin, &failing, &usable, &needs, &lookup),
            ))
        })
        .await;
    let (until, selection) = match selected {
        Ok(selected) => selected,
        Err(error) => {
            tracing::warn!(step = %step.name, "agent fallback lookup failed: {error}");
            return Next::Keep;
        }
    };
    match selection {
        Selection::Switch(link) => match apply(step, &link) {
            Ok(next) => {
                let notice = fallback::notice(
                    &step.agent,
                    model_of(step),
                    trigger,
                    Some(until),
                    &link,
                    tier_of(step),
                );
                tracing::info!(step = %step.name, "{notice}");
                Next::Rerun {
                    step: Box::new(next),
                    notice,
                }
            }
            Err(why) => Next::Stopped {
                notice: fallback::stop_notice(&step.agent, trigger, &why),
            },
        },
        Selection::Stop { why } => Next::Stopped {
            notice: fallback::stop_notice(&step.agent, trigger, &why),
        },
    }
}

/// KT-1657 — a run parked on a quota whose provider still refuses at its
/// re-check: the link of the step's chain that can take over, if any. The caller
/// switches the step to it, the same gesture as the owner's "switch agent".
pub async fn link_for_waiting_step_among(
    state: &AppState,
    agents: &AgentsConfig,
    step: &WorkflowStep,
    usable: Option<Vec<AgentType>>,
) -> Option<crate::models::AgentFallback> {
    if step.step_type != StepType::Agent || step.agent == AgentType::Custom {
        return None;
    }
    let chain = chain_of(step, agents, &step.agent);
    if chain.is_empty() {
        return None;
    }
    let usable = usable_agents(state, usable).await;
    let agents = agents.clone();
    let needs = needs_of(step, &agents);
    let failing = step.agent.clone();
    let declared_tools = step.clone();
    let now = Utc::now();
    let selection = state
        .db
        .with_read_conn(move |conn| {
            let cooling = fallback::cooling_lookup(conn, now)?;
            let lookup = |candidate: &AgentType| {
                cooling
                    .iter()
                    .find(|(agent, _)| agent == candidate)
                    .map(|(_, until)| *until)
            };
            Ok(fallback::select(
                &agents, &chain, &failing, &failing, &usable, &needs, &lookup,
            ))
        })
        .await
        .ok()?;
    match selection {
        Selection::Switch(link) => apply(&declared_tools, &link).ok().map(|_| link),
        Selection::Stop { .. } => None,
    }
}

/// The notices go first in the step's output, as quoted lines the run view
/// shows with the step's result.
pub fn with_notices(output: String, notices: &[String]) -> String {
    if notices.is_empty() {
        return output;
    }
    let lines: Vec<String> = notices.iter().map(|notice| format!("> {notice}")).collect();
    format!("{}\n\n{output}", lines.join("\n"))
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "agent_fallback_step_tests.rs"]
mod tests;
