//! KT-1518 — a NEW turn of a room whose agent is on cooldown.
//!
//! The provider of that agent refused a turn a moment ago, so a launch on it
//! would only be refused again: the turn starts on the agent's fallback. Once
//! the cooldown has passed the room's own agent is used again with nothing to
//! undo — an expired cooldown is simply not read.

use chrono::Utc;
use rusqlite::params;

use super::agent_fallback::system_notice;
use crate::core::agent_fallback::{self as fallback, Pin, Trigger, TurnNeeds};
use crate::db::agent_dispatch::AgentDispatchJob;
use crate::models::{AgentType, FallbackMode};
use crate::AppState;

/// The job to run: `job` itself, or — its agent overridden and pinned — the
/// same turn moved to the fallback. Task workers are not moved here: their
/// launch already refuses a provider with an open quota escalation.
pub async fn start_on_fallback(state: &AppState, job: AgentDispatchJob) -> AgentDispatchJob {
    // Cheap exit first: a start with no cooldown anywhere costs one read.
    let any_cooldown = state
        .db
        .with_read_conn(|conn| fallback::list_cooldowns(conn, Utc::now()))
        .await
        .map(|cooldowns| !cooldowns.is_empty())
        .unwrap_or(false);
    if !any_cooldown {
        return job;
    }
    start_on_fallback_among(state, job, None).await
}

pub async fn start_on_fallback_among(
    state: &AppState,
    job: AgentDispatchJob,
    usable: Option<Vec<AgentType>>,
) -> AgentDispatchJob {
    if job.connection_id.is_some() {
        return job;
    }
    let agents = state.config.read().await.agents.clone();
    let lookup = job.clone();
    let read = state
        .db
        .with_read_conn(move |conn| {
            if crate::db::orchestration::get_execution_for_dispatch(conn, &lookup.id)?.is_some()
                || fallback::get_pin(conn, &lookup.id)?.is_some()
            {
                return Ok(None);
            }
            let Some(discussion) =
                crate::db::discussions::get_discussion(conn, &lookup.discussion_id)?
            else {
                return Ok(None);
            };
            let targets =
                crate::db::discussions::list_message_targets(conn, &lookup.trigger_message_id)?;
            let chars: usize = discussion
                .messages
                .iter()
                .map(|message| message.content.chars().count())
                .sum();
            let chain_override = fallback::get_discussion_override(conn, &lookup.discussion_id)?;
            Ok(Some((
                discussion.agent,
                discussion.tier,
                discussion.model,
                targets,
                chars,
                chain_override,
            )))
        })
        .await
        .ok()
        .flatten();
    let Some((room_agent, room_tier, room_model, targets, chars, chain_override)) = read else {
        return job;
    };
    let agent = job.agent_override.clone().unwrap_or(room_agent);
    let tier = super::runtime::dispatch_target_tier(&targets, job.agent_override.as_ref(), None)
        .unwrap_or(room_tier);
    let mode = FallbackMode::from_tier(tier);
    let chain = fallback::effective_chain(
        &agents,
        &agent,
        mode,
        fallback::override_for(chain_override.as_deref(), mode),
    );
    if agent == AgentType::Custom || chain.is_empty() {
        return job;
    }
    let needs = TurnNeeds {
        long_context: chars > fallback::LONG_CONTEXT_CHARS,
        ..Default::default()
    };
    let now = Utc::now();
    let from = agent.clone();
    let usable = match usable {
        Some(usable) => usable,
        None => crate::api::orchestration::available_agent_types_for_state(state).await,
    };
    let preferred = state
        .db
        .with_read_conn(move |conn| {
            fallback::preferred_start(conn, &agents, &chain, &from, &usable, &needs, now)
        })
        .await
        .ok()
        .flatten();
    let Some((link, cooldown)) = preferred else {
        return job;
    };
    let trigger = Trigger::from_code(&cooldown.reason).unwrap_or(Trigger::QuotaExhausted);
    let notice = fallback::notice(
        &agent,
        room_model.as_deref(),
        trigger,
        Some(cooldown.until),
        &link,
        tier,
    );
    let message = system_notice(notice.clone(), &link.agent, tier, &job.trigger_message_id);
    let pin = Pin {
        from_agent: agent,
        to_agent: link.agent.clone(),
        model: link.model.clone(),
        tier,
        hops: 1,
    };
    let Ok(agent_json) = serde_json::to_string(&link.agent) else {
        return job;
    };
    let id = job.id.clone();
    let discussion_id = job.discussion_id.clone();
    let moved = state
        .db
        .with_conn(move |conn| {
            let transaction = conn.unchecked_transaction()?;
            let changed = transaction.execute(
                "UPDATE agent_dispatch_jobs SET agent_override_json = ?2
                 WHERE id = ?1 AND status = 'Running'",
                params![id, agent_json],
            )?;
            anyhow::ensure!(changed > 0, "dispatch is no longer running");
            fallback::set_pin(&transaction, &id, &pin, trigger)?;
            crate::db::discussions::insert_message(&transaction, &discussion_id, &message)?;
            transaction.commit()?;
            Ok(())
        })
        .await;
    match moved {
        Ok(()) => {
            tracing::info!(dispatch_job_id = %job.id, "{notice}");
            AgentDispatchJob {
                agent_override: Some(link.agent),
                ..job
            }
        }
        Err(error) => {
            tracing::warn!(dispatch_job_id = %job.id, "agent fallback could not move a new turn: {error}");
            job
        }
    }
}
