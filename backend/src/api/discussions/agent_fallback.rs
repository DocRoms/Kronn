//! KT-1518 — a discussion turn or a task worker whose provider refused the
//! turn continues on the agent's fallback chain.
//!
//! The policy lives in `core::agent_fallback`; this module applies it to a
//! settled dispatch:
//!
//! * a plain room: the SAME dispatch is re-queued on the fallback, pinned to it
//!   (agent, model, tier), the refused reply replaced by one line saying so;
//! * a task worker: the execution is reassigned to the fallback in its own
//!   room and worktree (the KT-1564 reassignment, so the delivery identity
//!   rules and the return after the reset stay what they are).
//!
//! [`intercept_failed_turn`] is asked for every failed turn before anything
//! else is decided about it. It answers `true` only when the turn went on
//! elsewhere; a failure that is not the provider's own (a task failure, a
//! refusal, a test failure, the work's timeout) is never a trigger, because
//! `classify_trigger` says `None` for it.

use chrono::{DateTime, Utc};
use rusqlite::params;
use uuid::Uuid;

use crate::core::agent_fallback::{self as fallback, Pin, Selection, Trigger, TurnNeeds};
use crate::db::agent_dispatch::AgentDispatchJob;
use crate::models::{
    AgentFallback, AgentType, DiscussionMessage, FallbackMode, MessageChannel, MessageRole,
    ModelTier,
};
use crate::AppState;

/// Everything a decision needs, read once.
struct Plan {
    failing: AgentType,
    /// The agent the turn started on: its chain (or the override) is the one walked.
    origin: AgentType,
    /// The model the failing agent ran, when one was pinned; else its tier.
    from_model: Option<String>,
    trigger: Trigger,
    until: DateTime<Utc>,
    selection: Selection,
    surface: Surface,
    tier: ModelTier,
    hops: u32,
    /// KT-1758 — the refusal's own words, for a launch the engine refused: the
    /// room says why the agent could not take the worker, not only that it
    /// could not.
    detail: Option<String>,
}

enum Surface {
    Room,
    Worker {
        execution_id: String,
        parent_room: String,
    },
}

/// Reads the turn, records the failing agent's cooldown and chooses the next
/// link. `None`: no chain is configured for the agent that failed, or the turn
/// is not one a fallback may take (a named connection, a hop limit).
async fn plan(
    state: &AppState,
    job: &AgentDispatchJob,
    trigger: Trigger,
    response: &str,
    usable: Option<Vec<AgentType>>,
) -> Option<Plan> {
    if job.connection_id.is_some() {
        return None;
    }
    let agents = state.config.read().await.agents.clone();
    let lookup = job.clone();
    let read = state
        .db
        .with_read_conn(move |conn| {
            let execution = crate::db::orchestration::get_execution_for_dispatch(conn, &lookup.id)?;
            let Some(discussion) =
                crate::db::discussions::get_discussion(conn, &lookup.discussion_id)?
            else {
                return Ok(None);
            };
            let pin = fallback::get_pin(conn, &lookup.id)?;
            let chain_override = fallback::get_discussion_override(conn, &lookup.discussion_id)?;
            let worker_origin = match &execution {
                Some(execution) => fallback::get_execution_origin(conn, &execution.id)?,
                None => None,
            };
            let targets =
                crate::db::discussions::list_message_targets(conn, &lookup.trigger_message_id)?;
            let chars: usize = discussion
                .messages
                .iter()
                .map(|message| message.content.chars().count())
                .sum();
            Ok(Some((
                execution,
                discussion,
                pin,
                targets,
                chars,
                chain_override,
                worker_origin,
            )))
        })
        .await
        .ok()
        .flatten()?;
    let (execution, discussion, pin, targets, chars, chain_override, worker_origin) = read;
    let (failing, surface, tier) = match &execution {
        Some(execution) => {
            if execution.status.is_terminal() {
                return None;
            }
            let failing = execution
                .worker_agent_type
                .as_deref()
                .and_then(|agent| crate::db::orchestration::agent_type_from_db(agent).ok())?;
            let tier = execution
                .worker_model_tier
                .as_deref()
                .and_then(crate::api::orchestration::model_tier_from_db)
                .unwrap_or_default();
            (
                failing,
                Surface::Worker {
                    execution_id: execution.id.clone(),
                    parent_room: execution.parent_discussion_id.clone(),
                },
                tier,
            )
        }
        None => {
            let failing = job
                .agent_override
                .clone()
                .unwrap_or_else(|| discussion.agent.clone());
            let tier = pin
                .as_ref()
                .map(|pin| pin.tier)
                .or_else(|| {
                    super::runtime::dispatch_target_tier(
                        &targets,
                        job.agent_override.as_ref(),
                        None,
                    )
                })
                .unwrap_or(discussion.tier);
            (failing, Surface::Room, tier)
        }
    };
    let origin = match &surface {
        Surface::Worker { .. } => worker_origin.unwrap_or_else(|| failing.clone()),
        Surface::Room => pin
            .as_ref()
            .map(|pin| pin.from_agent.clone())
            .unwrap_or_else(|| failing.clone()),
    };
    let mode = FallbackMode::from_tier(tier);
    let chain = fallback::effective_chain(
        &agents,
        &origin,
        mode,
        fallback::override_for(chain_override.as_deref(), mode),
    );
    if failing == AgentType::Custom || chain.is_empty() {
        return None;
    }
    let from_model = pin
        .as_ref()
        .and_then(|pin| pin.model.clone())
        .or_else(|| discussion.model.clone());
    let hops = pin.as_ref().map_or(0, |pin| pin.hops);
    if hops as usize >= fallback::MAX_CHAIN_DEPTH {
        return None;
    }
    let needs = match surface {
        Surface::Worker { .. } => TurnNeeds {
            write_access: true,
            task_worker: true,
            // KT-1991 — a generic worker builds, tests and commits: it never
            // moves to an HTTP model. Only a prelocalised edit can be served by
            // one, as it is launched on one in the first place. A worker whose
            // execution is unknown is read as generic (fail closed).
            shell: execution
                .as_ref()
                .is_none_or(|execution| execution.worker_scope.is_none()),
            ..Default::default()
        },
        Surface::Room => TurnNeeds {
            long_context: chars > fallback::LONG_CONTEXT_CHARS,
            ..Default::default()
        },
    };
    let now = Utc::now();
    let announced = fallback::announced_reset(response, now);
    // The installed agents are only detected once a chain is known to exist.
    let usable = match usable {
        Some(usable) => usable,
        None => crate::api::orchestration::available_agent_types_for_state(state).await,
    };
    let failing_for_db = failing.clone();
    let origin_for_db = origin.clone();
    let (until, selection) = state
        .db
        .with_conn(move |conn| {
            // A launch refusal is about this agent as a task worker, not about
            // its provider: no cooldown, so its rooms keep using it.
            let until = if trigger.is_launch_stop() {
                now
            } else {
                fallback::set_cooldown(conn, &failing_for_db, trigger, announced, now)?
            };
            let cooling = fallback::cooling_lookup(conn, now)?;
            let lookup = |candidate: &AgentType| {
                cooling
                    .iter()
                    .find(|(agent, _)| agent == candidate)
                    .map(|(_, until)| *until)
            };
            Ok((
                until,
                fallback::select(
                    &agents,
                    &chain,
                    &origin_for_db,
                    &failing_for_db,
                    &usable,
                    &needs,
                    &lookup,
                ),
            ))
        })
        .await
        .map_err(|error| tracing::warn!(dispatch_job_id = %job.id, "agent fallback lookup failed: {error}"))
        .ok()?;
    Some(Plan {
        failing,
        origin,
        from_model,
        trigger,
        until,
        selection,
        surface,
        tier,
        hops,
        detail: trigger
            .is_launch_stop()
            .then(|| response.chars().take(LAUNCH_REFUSAL_DETAIL_CHARS).collect()),
    })
}

pub use crate::api::orchestration::LAUNCH_REFUSAL_DETAIL_CHARS;

pub(super) fn system_notice(
    content: String,
    agent: &AgentType,
    tier: ModelTier,
    reply_to: &str,
) -> DiscussionMessage {
    DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        session_continuity: None,
        kind: Default::default(),
        model: None,
        lint_report: None,
        id: Uuid::new_v4().to_string(),
        role: MessageRole::System,
        channel: MessageChannel::Main,
        content,
        agent_type: Some(agent.clone()),
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: Some(tier.as_str().to_string()),
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: Some(reply_to.to_string()),
    }
}

/// A failed turn that may continue elsewhere. `true`: it did — the dispatch is
/// queued again (room) or the execution reassigned (worker) and the caller has
/// nothing more to settle. `false`: nothing changed for the turn (a stop notice
/// may have been written) and its failure is handled as before.
///
/// A task worker's QUOTA refusal is left to the caller: its settlement and the
/// campaign's own opt-in (`quota_fallback`) come first, and the chain is asked
/// from there through [`worker_after_quota`]. Any other trigger (a rate limit,
/// an outage, an authentication failure) follows the agent's chain at once.
/// KT-1991 — on every path a task worker without a prelocalised scope asks for
/// a shell, so its chain never moves it to an HTTP model.
pub async fn intercept_failed_turn(
    state: &AppState,
    job: &AgentDispatchJob,
    response: &str,
) -> bool {
    // Cheap exits first: most failed turns are not the provider's, and most
    // installs have no chain.
    if fallback::classify_trigger(response).is_none() {
        return false;
    }
    intercept_failed_turn_among(state, job, response, None).await
}

/// [`intercept_failed_turn`] with the installed agents given, so the policy is
/// testable without the CLIs of the host.
pub async fn intercept_failed_turn_among(
    state: &AppState,
    job: &AgentDispatchJob,
    response: &str,
    usable: Option<Vec<AgentType>>,
) -> bool {
    let Some(trigger) = fallback::classify_trigger(response) else {
        return false;
    };
    // A saturated provider is already retried on the same agent by the caller;
    // the chain is asked once those retries are spent.
    if trigger == Trigger::Outage
        && job.turn_attempts < 2
        && super::is_transient_provider_overload(response)
    {
        return false;
    }
    if job.connection_id.is_some() {
        return requeue_named_connection(state, job, trigger).await;
    }
    let Some(plan) = plan(state, job, trigger, response, usable).await else {
        return false;
    };
    // Any other outage on a room gets its one retry here before the chain is
    // used; a worker's dispatch has no same-agent retry to wait for.
    if trigger == Trigger::Outage && job.turn_attempts < 2 && matches!(plan.surface, Surface::Room)
    {
        return requeue_same_agent(state, job).await;
    }
    match (&plan.surface, &plan.selection) {
        (Surface::Room, Selection::Switch(link)) => requeue_on_link(state, job, &plan, link).await,
        (Surface::Worker { .. }, Selection::Switch(_)) if trigger != Trigger::QuotaExhausted => {
            settle_worker_failure(state, job, response, trigger).await;
            match apply_worker(state, job, &plan).await {
                Ok(()) => true,
                // Moved by someone else since the dispatch failed (the quota
                // return): the execution is theirs now, nothing is parked.
                Err(error) if crate::api::orchestration::is_execution_moved_on(&error) => {
                    tracing::info!(dispatch_job_id = %job.id, "agent fallback skipped: {error}");
                    true
                }
                Err(error) => {
                    tracing::warn!(dispatch_job_id = %job.id, "agent fallback of a task worker failed: {error}");
                    // The settlement held the interruption back for the link
                    // that was to take over; none did.
                    park_undelivered_worker(state, job).await;
                    false
                }
            }
        }
        (Surface::Worker { .. }, _) if trigger == Trigger::QuotaExhausted => false,
        (_, Selection::Stop { why }) => {
            post_stop(state, job, &plan, why).await;
            false
        }
        _ => false,
    }
}

/// The task worker's quota refusal, once its settlement and the campaign's own
/// fallback have been looked at: the agent chain. `Ok(true)`: the execution
/// moved to the next link.
pub async fn worker_after_quota(
    state: &AppState,
    job: &AgentDispatchJob,
    error: &str,
) -> anyhow::Result<bool> {
    worker_after_quota_among(state, job, error, None).await
}

pub async fn worker_after_quota_among(
    state: &AppState,
    job: &AgentDispatchJob,
    error: &str,
    usable: Option<Vec<AgentType>>,
) -> anyhow::Result<bool> {
    // Only a quota refusal is this function's business: the durable kind the
    // settlement wrote, or the provider's own words. Anything else is the
    // work's failure and stays where it is.
    if !error.starts_with("quota_exhausted:")
        && fallback::classify_trigger(error) != Some(Trigger::QuotaExhausted)
    {
        return Ok(false);
    }
    let Some(plan) = plan(state, job, Trigger::QuotaExhausted, error, usable).await else {
        return Ok(false);
    };
    if !matches!(plan.surface, Surface::Worker { .. }) {
        return Ok(false);
    }
    match &plan.selection {
        Selection::Switch(_) => {
            apply_worker(state, job, &plan).await?;
            Ok(true)
        }
        Selection::Stop { why } => {
            post_stop(state, job, &plan, why).await;
            Ok(false)
        }
    }
}

/// What the agent's chain did with a task worker refused at launch.
#[derive(Debug, Clone, PartialEq)]
pub enum LaunchChain {
    /// The execution moved to the next link.
    Moved,
    /// Nothing took it. `why`: the chain's own reason (« No fallback could take
    /// over from Codex: Ollama cannot take this turn: it has no shell … »),
    /// when a chain is configured; the caller's stop notice carries it.
    NoLink { why: Option<String> },
}

impl LaunchChain {
    pub fn moved(&self) -> bool {
        matches!(self, Self::Moved)
    }
}

/// KT-1758 — a task worker whose agent was refused at launch for good (a
/// native ACP agent as a task worker, a missing binary): the agent's chain, the
/// next link after the one that was refused. [`LaunchChain::Moved`]: the
/// execution moved on. [`LaunchChain::NoLink`]: nothing in the chain can take
/// it (the stop is said in the caller's own words, in both rooms, with the
/// chain's reason when there is one).
///
/// `trigger` says which stop it is, which is what the room line says:
/// [`Trigger::LaunchRefused`] for a refusal, [`Trigger::LaunchUnavailable`] for
/// retries that ran out.
pub async fn worker_after_launch_refusal_among(
    state: &AppState,
    job: &AgentDispatchJob,
    trigger: Trigger,
    reason: &str,
    usable: Option<Vec<AgentType>>,
) -> anyhow::Result<LaunchChain> {
    debug_assert!(trigger.is_launch_stop());
    let Some(plan) = plan(state, job, trigger, reason, usable).await else {
        return Ok(LaunchChain::NoLink { why: None });
    };
    if !matches!(plan.surface, Surface::Worker { .. }) {
        return Ok(LaunchChain::NoLink { why: None });
    }
    match &plan.selection {
        Selection::Switch(_) => {
            apply_worker(state, job, &plan).await?;
            Ok(LaunchChain::Moved)
        }
        Selection::Stop { why } => Ok(LaunchChain::NoLink {
            why: Some(why.clone()),
        }),
    }
}

/// A same-agent retry of a room turn (the outage's one retry).
async fn requeue_same_agent(state: &AppState, job: &AgentDispatchJob) -> bool {
    let id = job.id.clone();
    let discussion_id = job.discussion_id.clone();
    let retried = state
        .db
        .with_conn(move |conn| {
            let transaction = conn.unchecked_transaction()?;
            anyhow::ensure!(
                crate::db::agent_dispatch::retry_after(
                    &transaction,
                    &id,
                    30,
                    "provider_outage_retry"
                )?,
                "dispatch is no longer running"
            );
            crate::db::discussions::delete_dispatch_reply_messages(
                &transaction,
                &discussion_id,
                &id,
            )?;
            crate::db::discussions::set_awaiting_agent(&transaction, &discussion_id, true)?;
            transaction.commit()?;
            Ok(())
        })
        .await;
    match retried {
        Ok(()) => {
            state.agent_dispatch_notify.notify_one();
            true
        }
        Err(error) => {
            tracing::warn!(dispatch_job_id = %job.id, "outage retry failed: {error}");
            false
        }
    }
}

async fn requeue_named_connection(
    state: &AppState,
    job: &AgentDispatchJob,
    trigger: Trigger,
) -> bool {
    let Some(owner) = job.connection_id.as_deref() else {
        return false;
    };
    let lookup = job.clone();
    let context = state
        .db
        .with_read_conn(move |conn| {
            if crate::db::orchestration::get_execution_for_dispatch(conn, &lookup.id)?.is_some() {
                return Ok(None);
            }
            // Discussion overrides remain authoritative.
            if fallback::get_discussion_override(conn, &lookup.discussion_id)?.is_some() {
                return Ok(None);
            }
            let discussion = crate::db::discussions::get_discussion(conn, &lookup.discussion_id)?;
            let pin = fallback::get_pin(conn, &lookup.id)?;
            let targets =
                crate::db::discussions::list_message_targets(conn, &lookup.trigger_message_id)?;
            Ok(discussion.map(|discussion| (discussion, pin, targets)))
        })
        .await
        .ok()
        .flatten();
    let Some((discussion, previous, targets)) = context else {
        return false;
    };
    let hops = previous.as_ref().map_or(0, |pin| pin.hops);
    if hops as usize >= fallback::MAX_CHAIN_DEPTH {
        return false;
    }
    let tier = previous
        .as_ref()
        .map(|pin| pin.tier)
        .or_else(|| {
            super::runtime::dispatch_target_tier(&targets, job.agent_override.as_ref(), Some(owner))
        })
        .unwrap_or(discussion.tier);
    let Some((connection, model)) = crate::core::connection_fallback::next_text_target(
        state,
        owner,
        FallbackMode::from_tier(tier),
    )
    .await
    else {
        return false;
    };
    let agent = crate::db::external_api_connections::target_for_connection(&connection).agent_type;
    let pin = Pin {
        from_agent: job.agent_override.clone().unwrap_or(discussion.agent),
        to_agent: agent.clone(),
        model: Some(model),
        tier,
        hops: hops + 1,
    };
    let notice = system_notice(
        format!(
            "{} could not serve this turn; continued on {}",
            owner, connection.display_name
        ),
        &agent,
        tier,
        &job.trigger_message_id,
    );
    let job = job.clone();
    let changed = state.db.with_conn(move |conn| {
        let tx = conn.unchecked_transaction()?;
        let count = tx.execute("UPDATE agent_dispatch_jobs SET connection_id = ?2, agent_override_json = ?3 WHERE id = ?1 AND status = 'Running'", params![job.id, connection.id, serde_json::to_string(&agent)?])?;
        if count == 0 { return Ok(false); }
        fallback::set_pin(&tx, &job.id, &pin, trigger)?;
        anyhow::ensure!(crate::db::agent_dispatch::retry_after(&tx, &job.id, 0, "connection_fallback")?, "dispatch no longer running");
        crate::db::discussions::delete_dispatch_reply_messages(&tx, &job.discussion_id, &job.id)?;
        crate::db::discussions::set_awaiting_agent(&tx, &job.discussion_id, true)?;
        crate::db::discussions::insert_message(&tx, &job.discussion_id, &notice)?;
        tx.commit()?;
        Ok(true)
    }).await.unwrap_or(false);
    if changed {
        state.agent_dispatch_notify.notify_one();
    }
    changed
}

async fn requeue_on_link(
    state: &AppState,
    job: &AgentDispatchJob,
    plan: &Plan,
    link: &AgentFallback,
) -> bool {
    let notice = fallback::notice(
        &plan.failing,
        plan.from_model.as_deref(),
        plan.trigger,
        Some(plan.until),
        link,
        plan.tier,
    );
    let message = system_notice(
        notice.clone(),
        &link.agent,
        plan.tier,
        &job.trigger_message_id,
    );
    let id = job.id.clone();
    let discussion_id = job.discussion_id.clone();
    let pin = Pin {
        from_agent: plan.origin.clone(),
        to_agent: link.agent.clone(),
        model: link.model.clone(),
        tier: plan.tier,
        hops: plan.hops + 1,
    };
    let trigger = plan.trigger;
    let agent_json = match serde_json::to_string(&link.agent) {
        Ok(json) => json,
        Err(error) => {
            tracing::warn!("agent fallback could not encode the agent: {error}");
            return false;
        }
    };
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
            anyhow::ensure!(
                crate::db::agent_dispatch::retry_after(
                    &transaction,
                    &id,
                    0,
                    &format!("agent_fallback:{}", trigger.code()),
                )?,
                "dispatch is no longer running"
            );
            crate::db::discussions::delete_dispatch_reply_messages(
                &transaction,
                &discussion_id,
                &id,
            )?;
            crate::db::discussions::set_awaiting_agent(&transaction, &discussion_id, true)?;
            crate::db::discussions::insert_message(&transaction, &discussion_id, &message)?;
            transaction.commit()?;
            Ok(())
        })
        .await;
    match moved {
        Ok(()) => {
            tracing::info!(dispatch_job_id = %job.id, "{notice}");
            state.agent_dispatch_notify.notify_one();
            true
        }
        Err(error) => {
            tracing::warn!(dispatch_job_id = %job.id, "agent fallback could not re-queue the turn: {error}");
            false
        }
    }
}

/// The worker's failed dispatch is settled the way a refusal's is, so the
/// reassignment finds it finished.
async fn settle_worker_failure(
    state: &AppState,
    job: &AgentDispatchJob,
    response: &str,
    trigger: Trigger,
) {
    let detail: String = response.chars().take(400).collect();
    let error = format!("{}: {detail}", trigger.code());
    let id = job.id.clone();
    let discussion_id = job.discussion_id.clone();
    let group_id = job.group_id.clone();
    let kind = trigger.code().to_string();
    let settled = state
        .db
        .with_conn(move |conn| {
            super::runtime::persist_dispatch_settlement_handing_over(
                conn,
                &id,
                &discussion_id,
                group_id.as_deref(),
                crate::db::workflows::BatchChildOutcome::Failed,
                Some(&error),
                Some(&kind),
            )
        })
        .await;
    if let Err(error) = settled {
        tracing::warn!(dispatch_job_id = %job.id, "unable to settle the dispatch before its fallback: {error}");
    }
}

/// KT-1758 — no link took the worker over: the execution is parked and the
/// principal told, which [`settle_worker_failure`] held back for the link.
async fn park_undelivered_worker(state: &AppState, job: &AgentDispatchJob) {
    let (id, discussion_id) = (job.id.clone(), job.discussion_id.clone());
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            super::runtime::interrupt_undelivered_after_failure(conn, &id, &discussion_id)
        })
        .await
    {
        tracing::warn!(dispatch_job_id = %job.id, "unable to park the worker no link took over: {error}");
    }
}

/// Reassigns the worker to the chosen link, in its own room and worktree, and
/// says so in the parent room.
async fn apply_worker(state: &AppState, job: &AgentDispatchJob, plan: &Plan) -> anyhow::Result<()> {
    let (
        Surface::Worker {
            execution_id,
            parent_room,
        },
        Selection::Switch(link),
    ) = (&plan.surface, &plan.selection)
    else {
        anyhow::bail!("not a worker switch");
    };
    // KT-1749 — an HTTP link is a `discussion_agent`, a host CLI an `agent`.
    let mut target = fallback::task_worker_target(&link.agent);
    target.tier = Some(plan.tier);
    let id = execution_id.clone();
    let origin = plan.origin.clone();
    let failed_dispatch = job.id.clone();
    let profile_id = state
        .db
        .with_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &id)?;
            // KT-1758 — the execution may have been moved (the quota return)
            // since this dispatch failed: it is then not this hand-over's, and
            // the chain's memory of where it started is not touched either.
            if execution.as_ref().is_some_and(|execution| {
                execution.dispatch_job_id.as_deref() != Some(failed_dispatch.as_str())
            }) {
                return Err(
                    crate::api::orchestration::ExecutionMovedOn { execution_id: id }.into(),
                );
            }
            // Where the worker started: the chain it keeps walking is this agent's.
            fallback::note_execution_origin(conn, &id, &origin)?;
            Ok(execution.and_then(|execution| execution.worker_profile_id))
        })
        .await?;
    // A launch refusal has no "until": the agent is not unavailable, it cannot
    // be this kind of worker.
    let until = (!plan.trigger.is_launch_stop()).then_some(plan.until);
    let notice = fallback::notice(
        &plan.failing,
        plan.from_model.as_deref(),
        plan.trigger,
        until,
        link,
        plan.tier,
    );
    let notice = match &plan.detail {
        Some(detail) => format!("Task worker execution {execution_id}: {notice} — {detail}"),
        None => format!("Task worker execution {execution_id}: {notice}"),
    };
    if let Err(error) = crate::api::orchestration::reassign_native_execution_for_dispatch(
        state,
        execution_id,
        crate::models::CampaignWorkerSelection {
            target,
            model: link.model.clone(),
            profile_id,
        },
        &format!("automatic agent fallback: {notice}"),
        crate::api::orchestration::ReassignCause::QuotaFallback { reset_at: until },
        Some(&job.id),
    )
    .await
    {
        // KT-1758 — the execution moved on meanwhile (the quota return won):
        // nothing was changed and nothing is said.
        if crate::api::orchestration::is_execution_moved_on(&error) {
            return Err(error);
        }
        // KT-1749 — a refused reassignment is never only a log line: the
        // execution stays on its worker and both rooms are told why.
        let why = format!(
            "{} could not take over: {error}",
            fallback::display_name(&link.agent)
        );
        post_stop(state, job, plan, &why).await;
        return Err(error);
    }
    let message = crate::api::orchestration::orchestrator_message(
        format!("agent-fallback:{}", job.id),
        notice,
    );
    let room = parent_room.clone();
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::insert_message(conn, &room, &message)?;
            Ok(())
        })
        .await
    {
        tracing::warn!(dispatch_job_id = %job.id, "agent fallback notice could not be persisted: {error}");
    }
    state.agent_dispatch_notify.notify_one();
    Ok(())
}

/// KT-1664 — an execution parked on a quota whose provider still refuses at the
/// re-check: its agent's chain is the alternative to waiting. The same selection
/// as a failing turn (cooldown, opt-in for a paid link, capabilities), applied
/// through the same reassignment. `Ok(false)`: no chain, or nothing in it can
/// take the work.
pub async fn offer_to_escalated_execution_among(
    state: &AppState,
    execution_id: &str,
    usable: Option<Vec<AgentType>>,
) -> anyhow::Result<bool> {
    let id = execution_id.to_string();
    let job = state
        .db
        .with_read_conn(move |conn| {
            let Some(execution) = crate::db::orchestration::get_task_execution(conn, &id)? else {
                return Ok(None);
            };
            match execution.dispatch_job_id {
                Some(dispatch) => crate::db::agent_dispatch::get(conn, &dispatch),
                None => Ok(None),
            }
        })
        .await?;
    let Some(job) = job else {
        return Ok(false);
    };
    let Some(plan) = plan(state, &job, Trigger::QuotaExhausted, "", usable).await else {
        return Ok(false);
    };
    if !matches!(plan.surface, Surface::Worker { .. }) {
        return Ok(false);
    }
    match &plan.selection {
        Selection::Switch(_) => {
            apply_worker(state, &job, &plan).await?;
            Ok(true)
        }
        Selection::Stop { .. } => Ok(false),
    }
}

/// A chain is configured and nothing in it can take over: say why, where the
/// failure is read.
///
/// KT-1749 — for a task worker the reason goes to BOTH rooms, the principal's
/// (the parent) and the worker's own (the dispatch's discussion), in the
/// principal room's language: the execution stays where it is and nobody has to
/// read a log to learn why no fallback took over.
async fn post_stop(state: &AppState, job: &AgentDispatchJob, plan: &Plan, why: &str) {
    let mut posts: Vec<(String, DiscussionMessage)> = Vec::new();
    match &plan.surface {
        Surface::Room => {
            let content = fallback::stop_notice(&plan.failing, plan.trigger, why);
            posts.push((
                job.discussion_id.clone(),
                system_notice(content, &plan.failing, plan.tier, &job.trigger_message_id),
            ));
        }
        Surface::Worker {
            parent_room,
            execution_id,
        } => {
            let room = parent_room.clone();
            let lang = state
                .db
                .with_read_conn(move |conn| Ok(crate::db::discussions::room_lang(conn, &room)))
                .await
                .unwrap_or_default();
            let content = fallback::stop_notice_in(&plan.failing, plan.trigger, why, lang);
            let prefix = lang.pick("Exécution du worker de tâche", "Task worker execution");
            posts.push((
                parent_room.clone(),
                crate::api::orchestration::orchestrator_message(
                    format!("agent-fallback-stop:{}", job.id),
                    format!("{prefix} {execution_id}: {content}"),
                ),
            ));
            if job.discussion_id != *parent_room {
                let mut message =
                    system_notice(content, &plan.failing, plan.tier, &job.trigger_message_id);
                message.id = format!("agent-fallback-stop-worker:{}", job.id);
                posts.push((job.discussion_id.clone(), message));
            }
        }
    }
    for (room, message) in posts {
        if let Err(error) = state
            .db
            .with_conn(move |conn| {
                crate::db::discussions::insert_message(conn, &room, &message)?;
                Ok(())
            })
            .await
        {
            tracing::warn!(dispatch_job_id = %job.id, "agent fallback stop notice could not be persisted: {error}");
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "agent_fallback_tests.rs"]
mod tests;

fn offer_for_uplink<'a>(
    state: &'a AppState,
    execution_id: &'a str,
    usable: Option<Vec<AgentType>>,
) -> crate::core::uplink_state::Async<'a, anyhow::Result<bool>> {
    Box::pin(offer_to_escalated_execution_among(
        state,
        execution_id,
        usable,
    ))
}
inventory::submit! { crate::core::uplink_state::EscalatedFallback(offer_for_uplink) }
