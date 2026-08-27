// HTTP-facing endpoints that drive an agent: send_message (user
// types something → agent runs), run_agent (re-fire on existing
// thread), dismiss_partial (wipe a dangling boot-recovered partial),
// stop_agent (cancel a running agent via the cancel registry).
//
// All four either delegate to `super::streaming::make_agent_stream`
// or touch `state.cancel_registry` — they're the thin glue between
// the route layer and the streaming/runtime modules.

use std::convert::Infallible;

use anyhow::Context;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

use super::routing::{route_human_turn, DispatchRoute};
use super::streaming::make_agent_stream;
use super::{SseStream, MAX_CONTENT_LEN};

fn sse_events(events: Vec<Event>) -> Sse<SseStream> {
    let stream: SseStream = Box::pin(futures::stream::iter(
        events.into_iter().map(Ok::<_, Infallible>),
    ));
    Sse::new(stream)
}

fn accepted_event(message_id: &str, sort_order: i64, duplicate: bool) -> Event {
    Event::default().event("accepted").data(
        serde_json::json!({
            "message_id": message_id,
            "sort_order": sort_order,
            "duplicate": duplicate,
        })
        .to_string(),
    )
}

fn same_target_identity(left: &MessageTarget, right: &MessageTarget) -> bool {
    left.kind == right.kind
        && left.agent_type == right.agent_type
        && left.cli_session_id == right.cli_session_id
}

/// Native responders represented by a message's typed targets.
///
/// `None` means the discussion's principal agent; `Some(agent)` is a
/// punctual native agent. Joined CLI identities are deliberately excluded:
/// `/run` may start local processes, but it must never impersonate a joined
/// CLI session. An entirely untargeted message keeps the legacy principal
/// fallback.
fn native_dispatch_agents_for_targets(targets: &[MessageTarget]) -> Vec<Option<AgentType>> {
    if targets.is_empty() {
        return vec![None];
    }

    let mut agents = Vec::new();
    for target in targets {
        let candidate = match target.kind {
            MessageTargetKind::DiscussionAgent => Some(None),
            MessageTargetKind::Agent => Some(Some(target.agent_type.clone())),
            MessageTargetKind::Cli => None,
        };
        if let Some(candidate) = candidate {
            if !agents.contains(&candidate) {
                agents.push(candidate);
            }
        }
    }
    agents
}

fn enqueue_dispatches_for_trigger(
    conn: &Connection,
    discussion_id: &str,
    trigger_message_id: &str,
    trigger_sort_order: i64,
    run_key: &str,
) -> anyhow::Result<Vec<crate::db::agent_dispatch::AgentDispatchJob>> {
    let targets = crate::db::discussions::list_message_targets(conn, trigger_message_id)?;
    let dispatch_agents = native_dispatch_agents_for_targets(&targets);
    let mut jobs = Vec::with_capacity(dispatch_agents.len());
    for (position, agent_override) in dispatch_agents.iter().enumerate() {
        let job_id = Uuid::new_v4().to_string();
        let target_key = agent_override
            .as_ref()
            .map(|agent| format!("{agent:?}"))
            .unwrap_or_else(|| "discussion-agent".to_string());
        let dedupe_key = format!("force:{run_key}:{position}:{target_key}");
        let job = crate::db::agent_dispatch::enqueue(
            conn,
            crate::db::agent_dispatch::NewAgentDispatchJob {
                id: &job_id,
                discussion_id,
                trigger_message_id,
                trigger_sort_order,
                dedupe_key: &dedupe_key,
                agent_override: agent_override.as_ref(),
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )?;
        jobs.push(job);
    }
    Ok(jobs)
}

pub(crate) fn normalized_targets(
    targets: Vec<MessageTarget>,
    target_agents: Vec<AgentType>,
    legacy_target: Option<AgentType>,
) -> Vec<MessageTarget> {
    let mut targets = if !targets.is_empty() {
        targets
    } else if target_agents.is_empty() {
        legacy_target
            .into_iter()
            .map(MessageTarget::agent)
            .collect()
    } else {
        target_agents
            .into_iter()
            .map(MessageTarget::agent)
            .collect()
    };
    let mut deduped = Vec::with_capacity(targets.len());
    for target in targets.drain(..) {
        if !deduped
            .iter()
            .any(|existing| same_target_identity(existing, &target))
        {
            deduped.push(target);
        }
    }
    deduped
}

pub(crate) async fn canonical_targets(
    state: &AppState,
    discussion_id: &str,
    requested: Vec<MessageTarget>,
    target_all: bool,
) -> Result<Vec<MessageTarget>, String> {
    let did = discussion_id.to_string();
    let context = state
        .db
        .with_conn(move |conn| {
            let discussion = crate::db::discussions::get_discussion(conn, &did)?
                .ok_or_else(|| anyhow::anyhow!("discussion not found"))?;
            let sessions = crate::db::discussion_sessions::list_sessions(conn, &did, false)?;
            let no_agent = crate::db::discussions::disc_is_no_agent(conn, &did)?;
            Ok((discussion, sessions, no_agent))
        })
        .await
        .map_err(|error| error.to_string())?;
    let (discussion, sessions, no_agent) = context;

    let mut candidates = if target_all {
        let mut all = if no_agent {
            Vec::new()
        } else {
            vec![MessageTarget::discussion_agent(discussion.agent.clone())
                .with_tier(discussion.tier)]
        };
        all.extend(
            discussion
                .participants
                .iter()
                .filter(|agent| **agent != discussion.agent)
                .cloned()
                .map(|agent| MessageTarget::agent(agent).with_tier(ModelTier::Default)),
        );
        all.extend(sessions.iter().map(|session| {
            MessageTarget::cli(
                crate::db::discussions::parse_agent_type(&session.agent_type),
                session.id,
            )
        }));
        all
    } else {
        Vec::new()
    };
    candidates.extend(requested);

    let mut targets = Vec::with_capacity(candidates.len());
    for target in candidates {
        let canonical = match target.kind {
            MessageTargetKind::DiscussionAgent => {
                let mut canonical = MessageTarget::discussion_agent(discussion.agent.clone());
                canonical.tier = target.tier;
                canonical
            }
            MessageTargetKind::Agent => {
                let mut canonical = MessageTarget::agent(target.agent_type);
                canonical.tier = target.tier;
                canonical
            }
            MessageTargetKind::Cli => {
                let Some(session_id) = target.cli_session_id else {
                    return Err("CLI target requires cli_session_id".to_string());
                };
                let Some(session) = sessions.iter().find(|session| session.id == session_id) else {
                    return Err(format!(
                        "CLI target {session_id} is not part of this discussion"
                    ));
                };
                let session_agent = crate::db::discussions::parse_agent_type(&session.agent_type);
                if session_agent != target.agent_type {
                    return Err(format!(
                        "CLI target {session_id} does not match the requested agent"
                    ));
                }
                MessageTarget::cli(session_agent, session_id)
            }
        };
        if !targets
            .iter()
            .any(|existing| same_target_identity(existing, &canonical))
        {
            targets.push(canonical);
        }
    }
    Ok(targets)
}

async fn human_dispatch_route(
    state: &AppState,
    discussion_id: &str,
    target: Option<&MessageTarget>,
) -> DispatchRoute {
    let no_agent_id = discussion_id.to_string();
    let no_agent = state
        .db
        .with_conn(move |conn| crate::db::discussions::disc_is_no_agent(conn, &no_agent_id))
        .await
        .unwrap_or(false);
    route_human_turn(no_agent, target)
}

/// GET /api/discussions/running — disc ids with an in-flight agent run RIGHT
/// NOW, server-side. Source of truth is the cancel registry: every agent run
/// registers a `CancelGuard` there for its entire lifetime (removed on ANY
/// exit — completion, error, timeout, cancel), so its keys are exactly the
/// currently-running discussions, foreground OR background/batch. Page-
/// independent: the frontend polls this so a run still working after you
/// navigate away keeps showing as running, instead of looking dead and
/// tempting a needless re-launch (2026-06-24).
pub async fn running_discussions(State(state): State<AppState>) -> Json<ApiResponse<Vec<String>>> {
    let ids: Vec<String> = match state.cancel_registry.lock() {
        Ok(reg) => reg.keys().cloned().collect(),
        Err(poisoned) => poisoned.into_inner().keys().cloned().collect(),
    };
    Json(ApiResponse::ok(ids))
}

/// POST /api/discussions/:id/messages
pub async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SendMessageRequest>,
) -> Sse<SseStream> {
    // Input validation
    if req.content.len() > MAX_CONTENT_LEN {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(
                Event::default()
                    .event("error")
                    .data(serde_json::json!({ "error": "Message too long" }).to_string()),
            )
        }));
        return Sse::new(stream);
    }

    let message_id = match req.client_message_id.as_deref() {
        Some(raw) => match Uuid::parse_str(raw) {
            Ok(id) => id.to_string(),
            Err(_) => {
                return sse_events(vec![Event::default().event("error").data(
                    serde_json::json!({ "error": "Invalid client_message_id" }).to_string(),
                )]);
            }
        },
        None => Uuid::new_v4().to_string(),
    };
    // Message ids are durable opaque identifiers, not guaranteed UUIDs:
    // federated/legacy peers legitimately use ids such as `wsl-…`. The
    // same-discussion existence check below is the actual integrity boundary.
    let reply_to_message_id = req.reply_to_message_id.clone();
    if let Some(reply_id) = reply_to_message_id.as_deref() {
        let did = id.clone();
        let reply_id = reply_id.to_string();
        let belongs_to_discussion = state
            .db
            .with_conn(move |conn| {
                conn.query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM messages
                         WHERE id = ?1 AND discussion_id = ?2
                     )",
                    rusqlite::params![reply_id, did],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(anyhow::Error::from)
            })
            .await
            .unwrap_or(false);
        if !belongs_to_discussion {
            return sse_events(vec![Event::default().event("error").data(
                serde_json::json!({
                    "error": "Reply target not found in this discussion"
                })
                .to_string(),
            )]);
        }
    }

    if matches!(req.channel, MessageChannel::Note) {
        let (author_pseudo, author_avatar_email) = {
            let config = state.config.read().await;
            (
                config.server.pseudo.clone(),
                config.server.avatar_email.clone(),
            )
        };
        let note = DiscussionMessage {
            recovered_partial: false,
            session_tokens_at_message: None,
            author_cli_ordinal: None,
            model: None,
            lint_report: None,
            id: message_id.clone(),
            role: MessageRole::User,
            channel: MessageChannel::Note,
            content: req.content,
            agent_type: None,
            timestamp: Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo,
            author_avatar_email,
            source_msg_id: None,
            duration_ms: None,
            target_agent: None,
            reply_to_message_id,
        };
        let did = id.clone();
        let outcome = state
            .db
            .with_conn(move |conn| {
                let outcome = crate::db::discussions::insert_note_message(conn, &did, &note)?;
                if matches!(
                    &outcome,
                    crate::db::discussions::InsertUserMessageOutcome::Inserted { .. }
                ) {
                    if let Err(error) =
                        crate::db::discussions::link_pending_context_files_to_message(
                            conn, &did, &note.id,
                        )
                    {
                        tracing::warn!(
                            "Failed to link pending context files to note {}: {error}",
                            note.id
                        );
                    }
                }
                Ok(outcome)
            })
            .await;
        return match outcome {
            Ok(crate::db::discussions::InsertUserMessageOutcome::Inserted {
                message,
                sort_order,
                ..
            }) => {
                crate::api::federation::federate_message(&state, &id, &message).await;
                sse_events(vec![accepted_event(&message_id, sort_order, false)])
            }
            Ok(crate::db::discussions::InsertUserMessageOutcome::Duplicate { sort_order }) => {
                sse_events(vec![accepted_event(&message_id, sort_order, true)])
            }
            Ok(crate::db::discussions::InsertUserMessageOutcome::PartialPending) => {
                unreachable!("notes never participate in partial-response recovery")
            }
            Err(error) => {
                tracing::error!("Failed to save out-of-context note: {error}");
                sse_events(vec![Event::default().event("error").data(
                    serde_json::json!({ "error": "Failed to save note" }).to_string(),
                )])
            }
        };
    }

    let requested_targets = normalized_targets(
        req.targets.clone(),
        req.target_agents.clone(),
        req.target_agent.clone(),
    );
    let targets = match canonical_targets(&state, &id, requested_targets, req.target_all).await {
        Ok(targets) => targets,
        Err(error) => {
            return sse_events(vec![Event::default()
                .event("error")
                .data(serde_json::json!({ "error": error }).to_string())]);
        }
    };
    let target = targets.first().map(|target| target.agent_type.clone());
    // Resolve responder ownership before the acceptance transaction so human-
    // only/shared rooms never expose briefly-runnable local dispatch jobs.
    // Explicit plural targets are evaluated independently: a joined Claude can
    // own its reply while an absent Codex gets a durable native obligation.
    let mut routes = Vec::new();
    if targets.is_empty() {
        routes.push(human_dispatch_route(&state, &id, None).await);
    } else {
        for target in &targets {
            routes.push(human_dispatch_route(&state, &id, Some(target)).await);
        }
    }
    let local_dispatch_agents = routes
        .iter()
        .filter_map(|route| match route {
            DispatchRoute::NativePrincipal => Some(None),
            DispatchRoute::TargetedNative(agent) => Some(Some(agent.clone())),
            DispatchRoute::NoNativeResponder | DispatchRoute::JoinedPeers => None,
        })
        .collect::<Vec<_>>();
    let no_native_responder = routes
        .iter()
        .all(|route| matches!(route, DispatchRoute::NoNativeResponder));
    let joined_peers_only = !routes.is_empty()
        && routes
            .iter()
            .all(|route| matches!(route, DispatchRoute::JoinedPeers));
    // KT-157 — candidate for one-shot CLI catch-up: untargeted, main channel,
    // owned by the native principal. The CLI-presence check runs inside the
    // insert transaction, next to the row it marks.
    let mark_native_fallback = targets.is_empty()
        && matches!(req.channel, MessageChannel::Main)
        && routes
            .iter()
            .any(|route| matches!(route, DispatchRoute::NativePrincipal));

    // Read user identity from config for message attribution
    let (author_pseudo, author_avatar_email) = {
        let config = state.config.read().await;
        (
            config.server.pseudo.clone(),
            config.server.avatar_email.clone(),
        )
    };

    // Add user message to DB
    let user_msg = DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
        id: message_id.clone(),
        role: MessageRole::User,
        channel: req.channel,
        content: req.content.clone(),
        agent_type: None,
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo,
        author_avatar_email,
        source_msg_id: None,
        duration_ms: None,
        target_agent: target.clone(),
        reply_to_message_id,
    };
    let disc_id = id.clone();
    let msg = user_msg;
    let targets_clone = targets.clone();
    let local_dispatches = local_dispatch_agents
        .into_iter()
        .map(|agent| (Uuid::new_v4().to_string(), agent))
        .collect::<Vec<_>>();

    let insert_outcome = match state
        .db
        .with_conn(move |conn| {
            let dispatch_specs = local_dispatches
                .iter()
                .map(|(job_id, agent)| crate::db::discussions::UserDispatchSpec {
                    job_id,
                    agent_override: agent.as_ref(),
                    dedupe_key: None,
                })
                .collect::<Vec<_>>();
            let outcome = crate::db::discussions::insert_user_message_with_dispatches(
                conn,
                &disc_id,
                &msg,
                &targets_clone,
                &dispatch_specs,
                mark_native_fallback,
            )?;
            if matches!(
                &outcome,
                crate::db::discussions::InsertUserMessageOutcome::Inserted { .. }
            ) {
                // Pin any files the user staged in the composer to THIS message (0.8.8),
                // so they render in its bubble and clear from the input. Non-fatal: a
                // link failure must not drop the message the user just sent.
                if let Err(e) = crate::db::discussions::link_pending_context_files_to_message(
                    conn, &disc_id, &msg.id,
                ) {
                    tracing::warn!(
                        "Failed to link pending context files to message {}: {e}",
                        msg.id
                    );
                }
                // Track new participant
                for target in &targets_clone {
                    if target.kind != MessageTargetKind::Agent {
                        continue;
                    }
                    let t = &target.agent_type;
                    let disc = crate::db::discussions::get_discussion(conn, &disc_id)?;
                    if let Some(d) = disc {
                        if !d.participants.contains(t) {
                            let mut participants = d.participants;
                            participants.push(t.clone());
                            crate::db::discussions::update_discussion_participants(
                                conn,
                                &disc_id,
                                &participants,
                            )?;
                        }
                    }
                }
            }
            Ok(outcome)
        })
        .await
    {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::error!("Failed to save user message: {e}");
            let stream: SseStream = Box::pin(futures::stream::once(async move {
                Ok::<_, Infallible>(
                    Event::default()
                        .event("error")
                        .data(serde_json::json!({ "error": "Failed to save message" }).to_string()),
                )
            }));
            return Sse::new(stream);
        }
    };

    let (stored_user_msg, sort_order, claimed_dispatch) = match insert_outcome {
        crate::db::discussions::InsertUserMessageOutcome::Inserted {
            message,
            sort_order,
            dispatch_job,
        } => (*message, sort_order, dispatch_job.map(|job| *job)),
        crate::db::discussions::InsertUserMessageOutcome::Duplicate { sort_order } => {
            return sse_events(vec![accepted_event(&message_id, sort_order, true)]);
        }
        crate::db::discussions::InsertUserMessageOutcome::PartialPending => {
            // KT-251 DoD 3 — name the answer that is blocking. Without this the
            // user sees a banner about "a previous reply" they cannot identify,
            // which is exactly what was reported: "je ne vois pas encore d'id […]
            // ça t'aurait aidé au debug". The id exists from the first checkpoint
            // (migration 109), so there is nothing to compute — only to pass on.
            let blocking_id = {
                // `id` rather than `disc_id`: the latter was moved into the
                // insert closure above.
                let did = id.clone();
                state
                    .db
                    .with_read_conn(move |conn| {
                        crate::db::discussions::pending_partial_message_id(conn, &did)
                    })
                    .await
                    .ok()
                    .flatten()
            };
            return sse_events(vec![Event::default().event("error").data(
                serde_json::json!({
                    "error": "partial_pending",
                    // `null` when the checkpoint predates migration 109: unknown,
                    // which must not read as "no answer in flight".
                    "blocking_message_id": blocking_id,
                    "message": "Une réponse d'agent précédente est en cours de récupération. Patientez ou fermez la notification de récupération avant de renvoyer."
                }).to_string(),
            )]);
        }
    };
    let accepted = accepted_event(&message_id, sort_order, false);

    // F9 — human-only disc: never spawn an agent. Persist + federate the human
    // message (done above) and stop. Guarantees true human↔human chat even on
    // an instance that has an agent installed.
    if no_native_responder {
        crate::api::federation::federate_message(&state, &id, &stored_user_msg).await;
        let reason = if target.is_some() {
            "target_not_joined"
        } else {
            "no_agent"
        };
        let payload = serde_json::json!({
            "skipped": true,
            "reason": reason,
            "target_agent": target,
            "target_agents": targets,
        })
        .to_string();
        return sse_events(vec![
            accepted,
            Event::default().event("skipped_no_agent").data(payload),
        ]);
    }

    // Double-responder guard (2026-06-04, flagged by Romuald; made
    // presence-sticky 2026-06-08) — for an untargeted message, any active MCP
    // agent answers; for `@agent`, only an active session of that concrete
    // target suppresses the local runner. This lets an absent mentioned agent
    // join via the normal durable dispatch even while another peer is present.
    // Spawning the local runner too made BOTH reply to the same message
    // (reproduced on disc ca495847: Kronn's native reply + the CLI peer's
    // MCP reply to one user turn). The user message is already persisted
    // + broadcast above, so the connected agent picks it up — we simply
    // don't spawn. Emit one informational SSE event and let the stream end:
    // `parseSSEStream` fires onDone on stream-close, so the frontend's
    // "sending" state clears with no empty agent bubble (the peer's reply
    // arrives separately via the disc message list / WS).
    //
    // Membership remains sticky for the participant header, but dispatch
    // ownership requires fresh listening/reading activity. Otherwise a peer
    // that left without calling disc_leave suppresses the durable local
    // fallback until the 24h reaper runs and the room appears silent.
    if joined_peers_only {
        crate::api::federation::federate_message(&state, &id, &stored_user_msg).await;
        tracing::info!(
            "send_message: {} joined CLI target(s) on disc {id} — skipping local runner",
            targets.len()
        );
        let payload = serde_json::json!({
            "skipped": true,
            "reason": "live_mcp_agents",
            "live_agents": targets.len(),
        })
        .to_string();
        return sse_events(vec![
            accepted,
            Event::default().event("skipped_live_agents").data(payload),
        ]);
    }

    let stream = match claimed_dispatch {
        Some(job) => {
            super::runtime::stream_claimed_dispatch_job(state.clone(), job, Some(accepted)).await
        }
        None => None,
    };
    // Start/track the local durable run before awaiting federation I/O: once
    // the DB transaction returned a Running claim, no slow peer can strand it
    // between claim and execution.
    crate::api::federation::federate_message(&state, &id, &stored_user_msg).await;
    state.agent_dispatch_notify.notify_one();
    stream.unwrap_or_else(|| sse_events(vec![accepted_event(&message_id, sort_order, false)]))
}

/// POST /api/discussions/:id/messages/revise
///
/// Atomic edit/resend: CAS-edit the target User message, archive the trailing
/// reply projection, emit a fresh cursor-visible revision event and create
/// (or intentionally omit) the durable dispatch obligation in one transaction.
pub async fn revise_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ReviseMessageRequest>,
) -> Response {
    if request.content.trim().is_empty() || request.content.len() > MAX_CONTENT_LEN {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err("Invalid revised message content")),
        )
            .into_response();
    }
    if Uuid::parse_str(&request.idempotency_key).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err("Invalid idempotency_key")),
        )
            .into_response();
    }

    let requested_targets = normalized_targets(
        request.targets.clone(),
        request.target_agents.clone(),
        request.target_agent.clone(),
    );
    let targets = match canonical_targets(&state, &id, requested_targets, request.target_all).await
    {
        Ok(targets) => targets,
        Err(error) => {
            return (StatusCode::BAD_REQUEST, Json(ApiResponse::<()>::err(error))).into_response();
        }
    };
    let mut local_dispatch_agents = Vec::new();
    if targets.is_empty() {
        let route = human_dispatch_route(&state, &id, None).await;
        if matches!(route, DispatchRoute::NativePrincipal) {
            local_dispatch_agents.push(None);
        }
    } else {
        for target in &targets {
            match human_dispatch_route(&state, &id, Some(target)).await {
                DispatchRoute::NativePrincipal => local_dispatch_agents.push(None),
                DispatchRoute::TargetedNative(agent) => {
                    local_dispatch_agents.push(Some(agent));
                }
                DispatchRoute::NoNativeResponder | DispatchRoute::JoinedPeers => {}
            }
        }
    }
    let has_local_dispatch = !local_dispatch_agents.is_empty();
    let dispatches = local_dispatch_agents
        .into_iter()
        .map(|agent| (Uuid::new_v4().to_string(), agent))
        .collect::<Vec<_>>();
    let db_request = request.clone();
    let db_discussion_id = id.clone();
    let outcome = state
        .db
        .with_conn(move |connection| {
            let dispatch_specs = dispatches
                .iter()
                .map(|(job_id, agent)| crate::db::discussions::UserDispatchSpec {
                    job_id,
                    agent_override: agent.as_ref(),
                    dedupe_key: None,
                })
                .collect::<Vec<_>>();
            Ok(crate::db::discussions::revise_message_with_dispatch(
                connection,
                crate::db::discussions::ReviseMessageParams {
                    discussion_id: &db_discussion_id,
                    message_id: &db_request.message_id,
                    content: db_request.content.trim(),
                    expected_revision: &db_request.expected_revision,
                    idempotency_key: &db_request.idempotency_key,
                    targets: &targets,
                    dispatches: &dispatch_specs,
                },
            ))
        })
        .await;

    let outcome = match outcome {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(crate::db::discussions::ReviseMessageError::Conflict { current_revision })) => {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::<serde_json::Value>::err_coded(
                    ApiErrorCode::Conflict,
                    serde_json::json!({
                        "error": "revision_conflict",
                        "current_revision": current_revision,
                    })
                    .to_string(),
                )),
            )
                .into_response();
        }
        Ok(Err(crate::db::discussions::ReviseMessageError::IdempotencyConflict)) => {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::<()>::err_coded(
                    ApiErrorCode::Conflict,
                    "Idempotency key already belongs to a different revision",
                )),
            )
                .into_response();
        }
        Ok(Err(crate::db::discussions::ReviseMessageError::DispatchInProgress)) => {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::<()>::err_coded(
                    ApiErrorCode::Conflict,
                    "An agent response is already in progress",
                )),
            )
                .into_response();
        }
        Ok(Err(crate::db::discussions::ReviseMessageError::NotFound)) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<()>::err_coded(
                    ApiErrorCode::NotFound,
                    "Editable user message not found",
                )),
            )
                .into_response();
        }
        Ok(Err(error)) => {
            tracing::error!("Atomic message revision failed for {id}: {error}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err("Failed to revise message")),
            )
                .into_response();
        }
        Err(error) => {
            tracing::error!("Atomic message revision DB task failed for {id}: {error}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err("Failed to revise message")),
            )
                .into_response();
        }
    };

    let revision_event = Event::default()
        .event("message_revised")
        .data(serde_json::to_string(&outcome.receipt).unwrap_or_else(|_| "{}".into()));
    if !outcome.receipt.duplicate {
        crate::api::federation::federate_message_revision(&state, &outcome.event).await;
    }

    if !has_local_dispatch || outcome.receipt.duplicate {
        return sse_events(vec![revision_event]).into_response();
    }

    let stream = match outcome.claimed_dispatch {
        Some(job) => {
            super::runtime::stream_claimed_dispatch_job(state.clone(), job, Some(revision_event))
                .await
        }
        None => None,
    };
    state.agent_dispatch_notify.notify_one();
    stream
        .unwrap_or_else(|| sse_events(Vec::new()))
        .into_response()
}

/// POST /api/discussions/:id/run
pub async fn run_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Option<Json<RunAgentRequest>>,
) -> Sse<SseStream> {
    let enqueue_discussion_id = id.clone();
    let requested_key = request
        .and_then(|Json(request)| request.idempotency_key)
        .filter(|key| !key.trim().is_empty());
    if requested_key
        .as_ref()
        .is_some_and(|key| Uuid::parse_str(key).is_err())
    {
        return sse_events(vec![Event::default().event("error").data(
            serde_json::json!({ "error": "Invalid idempotency_key" }).to_string(),
        )]);
    }
    let run_key = requested_key.unwrap_or_else(|| Uuid::new_v4().to_string());
    let jobs = state
        .db
        .with_conn(move |conn| {
            if let Some(active) =
                crate::db::agent_dispatch::find_active_for_discussion(conn, &enqueue_discussion_id)?
            {
                return Ok((true, vec![active]));
            }
            let trigger = conn
                .query_row(
                    "SELECT id, sort_order FROM messages
                     WHERE discussion_id = ?1 AND role = 'User' AND channel = 'main'
                     ORDER BY sort_order DESC LIMIT 1",
                    [&enqueue_discussion_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()?;
            let Some((trigger_message_id, trigger_sort_order)) = trigger else {
                return Ok((false, Vec::new()));
            };
            let transaction = conn.unchecked_transaction()?;
            let jobs = enqueue_dispatches_for_trigger(
                &transaction,
                &enqueue_discussion_id,
                &trigger_message_id,
                trigger_sort_order,
                &run_key,
            )?;
            if !jobs.is_empty() {
                crate::db::discussions::set_awaiting_agent(
                    &transaction,
                    &enqueue_discussion_id,
                    true,
                )?;
            }
            transaction.commit()?;
            Ok((true, jobs))
        })
        .await;
    let job = match jobs {
        Ok((_, jobs)) if !jobs.is_empty() => jobs.into_iter().next().expect("non-empty jobs"),
        // Legacy/empty discussions have no User message to anchor a durable
        // obligation. Preserve the force-run behaviour for those rare cases.
        Ok((false, _)) => return make_agent_stream(state, id, None).await,
        // The latest message explicitly targets joined CLI identities only.
        // Its local `/run` launch is intentionally a no-op.
        Ok((true, _)) => return sse_events(Vec::new()),
        Err(error) => {
            tracing::error!("Force-run durable enqueue failed for {id}: {error}");
            return sse_events(vec![Event::default().event("error").data(
                serde_json::json!({ "error": "Failed to queue agent run" }).to_string(),
            )]);
        }
    };
    state.agent_dispatch_notify.notify_one();
    super::runtime::stream_dispatch_job(state, job.id, None)
        .await
        .unwrap_or_else(|| sse_events(Vec::new()))
}

/// POST /api/discussions/:id/agent-dispatches/retry
///
/// Retry exactly one failed native target without deleting sibling replies or
/// rebinding the request to the discussion's latest User message.
pub async fn retry_agent_dispatch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<RetryAgentDispatchRequest>,
) -> Response {
    if Uuid::parse_str(&request.idempotency_key).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err("Invalid idempotency_key")),
        )
            .into_response();
    }

    let discussion_id = id.clone();
    let failed_dispatch_id = request.dispatch_id.clone();
    let idempotency_key = request.idempotency_key.clone();
    let new_job_id = Uuid::new_v4().to_string();
    let default_agent_id = id.clone();
    let result = state
        .db
        .with_conn(move |conn| {
            let transaction = conn.unchecked_transaction()?;
            let default_agent =
                crate::db::discussions::get_discussion(&transaction, &default_agent_id)?
                    .context("discussion not found")?
                    .agent;
            let (job, duplicate) = crate::db::agent_dispatch::enqueue_retry(
                &transaction,
                &discussion_id,
                &failed_dispatch_id,
                &idempotency_key,
                &new_job_id,
            )?;
            crate::db::agent_dispatch::mark_error_retried(&transaction, &failed_dispatch_id)?;
            let should_wake = matches!(
                job.status,
                crate::db::agent_dispatch::DispatchStatus::Pending
                    | crate::db::agent_dispatch::DispatchStatus::Running
            );
            if should_wake {
                crate::db::discussions::set_awaiting_agent(&transaction, &discussion_id, true)?;
            }
            transaction.commit()?;
            Ok((job, duplicate, default_agent, should_wake))
        })
        .await;

    let (job, duplicate, default_agent, should_wake) = match result {
        Ok(value) => value,
        Err(error) => {
            let message = error.to_string();
            let status = if message.contains("not found") {
                StatusCode::NOT_FOUND
            } else if message.contains("another discussion")
                || message.contains("not failed")
                || message.contains("not supported")
            {
                StatusCode::CONFLICT
            } else {
                tracing::error!(discussion_id = %id, "Failed to retry agent dispatch: {error}");
                StatusCode::INTERNAL_SERVER_ERROR
            };
            return (status, Json(ApiResponse::<()>::err(&message))).into_response();
        }
    };
    if should_wake {
        state.agent_dispatch_notify.notify_one();
    }
    Json(ApiResponse::ok(RetryAgentDispatchResponse {
        dispatch_id: job.id,
        trigger_message_id: job.trigger_message_id,
        agent_type: job.agent_override.unwrap_or(default_agent),
        duplicate,
    }))
    .into_response()
}

/// POST /api/discussions/:id/dismiss-partial
///
/// Force-recover a pending partial_response on demand. Used by the
/// "Dismiss" button the frontend shows next to the PartialResponseRecovered
/// toast and as a fallback when the WS event missed: calls the same
/// recovery path used at boot, scoped to this one disc.
///
/// Returns `{ recovered: true }` if there was a partial to recover,
/// `{ recovered: false }` if the disc was clean (no-op, idempotent).
pub async fn dismiss_partial(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<serde_json::Value>> {
    let ids = match state
        .db
        .with_conn(move |conn| {
            // Reuses the boot recovery — process-wide (handles every disc with
            // a non-null partial), so a "dismiss" click incidentally cleans up
            // any other dangling partials too. Cheap (one indexed scan).
            crate::db::discussions::recover_partial_responses(conn)
        })
        .await
    {
        Ok(list) => list,
        Err(e) => return Json(ApiResponse::err(format!("Recovery failed: {}", e))),
    };
    let recovered_this = ids.iter().any(|d| d == &id);
    if !ids.is_empty() {
        let _ = state
            .ws_broadcast
            .send(WsMessage::PartialResponseRecovered {
                discussion_ids: ids,
            });
    }
    Json(ApiResponse::ok(
        serde_json::json!({ "recovered": recovered_this }),
    ))
}

/// POST /api/discussions/:id/stop
///
/// Abort the currently-running agent for this discussion. Triggers the active
/// dispatch token (or the discussion token for a legacy stream) when one is
/// registered in `state.cancel_registry`.
/// The agent task's `select!` picks up the cancellation, kills the spawned
/// child process, saves a partial response with an "⏹️ Interrompu" footer,
/// and broadcasts `batch_run_progress` if the disc was part of a batch.
///
/// Returns `{ cancelled: true }` if a token was registered and triggered,
/// `{ cancelled: false }` if nothing was running (agent already finished,
/// disc never started, race with backend restart, etc.) — which lets the
/// frontend show a "Rien à arrêter" toast rather than fake-confirming.
pub async fn stop_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<serde_json::Value>> {
    // Legacy streams without a durable dispatch remain discussion-keyed.
    let cancelled_legacy_process = {
        let mut map = match state.cancel_registry.lock() {
            Ok(m) => m,
            Err(_) => return Json(ApiResponse::err("Cancel registry poisoned")),
        };
        if let Some(token) = map.remove(&id) {
            token.cancel();
            true
        } else {
            false
        }
    };
    let cancel_id = id.clone();
    let cancelled_job = state
        .db
        .with_conn(move |conn| {
            let transaction = conn.unchecked_transaction()?;
            let active =
                crate::db::agent_dispatch::find_active_for_discussion(&transaction, &cancel_id)?;
            let count = crate::db::agent_dispatch::cancel_for_discussion(&transaction, &cancel_id)?;
            let resume_job_ids =
                crate::db::agent_jobs::cancel_for_discussion(&transaction, &cancel_id)?;
            let still_awaiting =
                crate::db::agent_dispatch::has_active_for_discussion(&transaction, &cancel_id)?;
            crate::db::discussions::set_awaiting_agent(&transaction, &cancel_id, still_awaiting)?;
            let mut batch_run = None;
            if count > 0 {
                if let Some(run_id) = active.as_ref().and_then(|job| job.group_id.as_deref()) {
                    batch_run = crate::db::workflows::increment_batch_progress(
                        &transaction,
                        run_id,
                        false,
                    )?;
                }
            }
            transaction.commit()?;
            Ok((active.map(|job| job.id), count, resume_job_ids, batch_run))
        })
        .await;
    let (active_dispatch_id, cancelled_jobs, resume_job_ids, batch_run) = cancelled_job
        .unwrap_or_else(|error| {
            tracing::error!("Failed to cancel durable dispatch for {id}: {error}");
            (None, 0, Vec::new(), None)
        });
    let cancelled_dispatch_process = active_dispatch_id.is_some_and(|dispatch_id| {
        let token = state
            .cancel_registry
            .lock()
            .ok()
            .and_then(|mut map| map.remove(&dispatch_id));
        if let Some(token) = token {
            token.cancel();
            true
        } else {
            false
        }
    });
    let cancelled_resume_process = if let Ok(mut registry) = state.cancel_registry.lock() {
        let mut cancelled = false;
        for job_id in &resume_job_ids {
            if let Some(token) = registry.remove(&format!("agent-job:{job_id}")) {
                token.cancel();
                cancelled = true;
            }
        }
        cancelled
    } else {
        false
    };
    if let Some(updated_run) = batch_run {
        super::streaming::broadcast_batch_progress(&state, &id, &updated_run);
    }
    let cancelled = cancelled_legacy_process
        || cancelled_dispatch_process
        || cancelled_resume_process
        || cancelled_jobs > 0
        || !resume_job_ids.is_empty();
    state.agent_dispatch_notify.notify_waiters();
    Json(ApiResponse::ok(
        serde_json::json!({ "cancelled": cancelled }),
    ))
}

/// POST /api/discussions/:id/agent-dispatches/:dispatch_id/stop
///
/// Stop one exact pending/running response while preserving every sibling
/// dispatch in the discussion. A running provider process is keyed by the
/// durable dispatch id, so cancelling an older reply cannot hit the newer one
/// that may start immediately afterwards.
pub async fn stop_agent_dispatch(
    State(state): State<AppState>,
    Path((id, dispatch_id)): Path<(String, String)>,
) -> Json<ApiResponse<serde_json::Value>> {
    let cancel_discussion_id = id.clone();
    let cancel_dispatch_id = dispatch_id.clone();
    let cancelled_job = state
        .db
        .with_conn(move |conn| {
            let transaction = conn.unchecked_transaction()?;
            let job = crate::db::agent_dispatch::get(&transaction, &cancel_dispatch_id)?;
            let Some(job) = job else {
                return Ok(None);
            };
            if job.discussion_id != cancel_discussion_id {
                return Ok(None);
            }
            let changed = crate::db::agent_dispatch::cancel_for_discussion_by_id(
                &transaction,
                &cancel_discussion_id,
                &cancel_dispatch_id,
            )?;
            let still_awaiting = crate::db::agent_dispatch::has_active_for_discussion(
                &transaction,
                &cancel_discussion_id,
            )?;
            crate::db::discussions::set_awaiting_agent(
                &transaction,
                &cancel_discussion_id,
                still_awaiting,
            )?;
            let batch_run = if changed {
                if let Some(run_id) = job.group_id.as_deref() {
                    crate::db::workflows::increment_batch_progress(&transaction, run_id, false)?
                } else {
                    None
                }
            } else {
                None
            };
            transaction.commit()?;
            Ok(Some((changed, still_awaiting, batch_run)))
        })
        .await;

    let Some((cancelled, still_awaiting, batch_run)) = (match cancelled_job {
        Ok(result) => result,
        Err(error) => {
            return Json(ApiResponse::err(format!(
                "Failed to stop agent response: {error}"
            )))
        }
    }) else {
        return Json(ApiResponse::err(
            "Agent response not found in this discussion",
        ));
    };

    if cancelled {
        // Annuler le token retourné par remove() : un job réclamé Running après
        // l'entrée du handler enregistre un token vivant qu'un snapshot perdrait.
        match state.cancel_registry.lock() {
            Ok(mut registry) => {
                if let Some(token) = registry.remove(&dispatch_id) {
                    token.cancel();
                }
            }
            // Mutex empoisonné : le job est déjà Cancelled en base, mais le process
            // ne peut plus être arrêté. Le signaler (comme `delete`) plutôt que sauter.
            Err(_) => tracing::warn!(
                "Cancel registry poisoned while stopping dispatch {dispatch_id}; \
                 process token left running"
            ),
        }
    }
    if let Some(updated_run) = batch_run {
        super::streaming::broadcast_batch_progress(&state, &id, &updated_run);
    }
    state.agent_dispatch_notify.notify_waiters();
    Json(ApiResponse::ok(serde_json::json!({
        "cancelled": cancelled,
        "dispatch_id": dispatch_id,
        "still_awaiting": still_awaiting,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::default_config;
    use crate::db::Database;
    use crate::DEFAULT_MAX_CONCURRENT_AGENTS;
    use axum::response::IntoResponse;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[test]
    fn plural_message_targets_create_independent_native_dispatches() {
        let targets = vec![
            MessageTarget::discussion_agent(AgentType::Codex).with_tier(ModelTier::Economy),
            MessageTarget::agent(AgentType::ClaudeCode).with_tier(ModelTier::Reasoning),
        ];

        assert_eq!(
            native_dispatch_agents_for_targets(&targets),
            vec![None, Some(AgentType::ClaudeCode)]
        );
    }

    #[test]
    fn native_dispatch_targets_ignore_joined_cli_and_deduplicate_agents() {
        let targets = vec![
            MessageTarget::cli(AgentType::Codex, 42),
            MessageTarget::agent(AgentType::Ollama),
            MessageTarget::agent(AgentType::Ollama),
        ];

        assert_eq!(
            native_dispatch_agents_for_targets(&targets),
            vec![Some(AgentType::Ollama)]
        );
        assert_eq!(native_dispatch_agents_for_targets(&[]), vec![None]);
    }

    #[test]
    fn initial_run_enqueues_one_durable_job_per_native_target() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO discussions (id, title, agent, participants_json, created_at, updated_at)
             VALUES ('d-initial-plural', 'Plural', 'Codex', '[\"Codex\",\"ClaudeCode\"]', ?1, ?1)",
            [&now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (
                 id, discussion_id, role, channel, content, timestamp,
                 sort_order, received_at
             ) VALUES ('u-initial', 'd-initial-plural', 'User', 'main', 'go', ?1, 0, ?1)",
            [&now],
        )
        .unwrap();
        crate::db::discussions::replace_message_targets(
            &conn,
            "u-initial",
            &[
                MessageTarget::discussion_agent(AgentType::Codex),
                MessageTarget::agent(AgentType::ClaudeCode),
            ],
        )
        .unwrap();

        let jobs =
            enqueue_dispatches_for_trigger(&conn, "d-initial-plural", "u-initial", 0, "test-run")
                .unwrap();

        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].agent_override, None);
        assert_eq!(jobs[1].agent_override, Some(AgentType::ClaudeCode));
        assert!(jobs.iter().all(|job| job.trigger_message_id == "u-initial"));
    }

    /// State with one project + one disc, mirroring the disc_invite test
    /// harness. `send_message` is a free function over extractors, so we
    /// drive it directly without spinning up axum.
    async fn make_state_with_disc(disc_id: &str) -> AppState {
        let db = Arc::new(Database::open_in_memory().expect("in-memory DB"));
        let disc_id = disc_id.to_string();
        db.with_conn(move |conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p-test', 'Test', '/tmp', ?1, ?1)",
                rusqlite::params![now],
            )?;
            conn.execute(
                "INSERT INTO discussions (id, project_id, title, created_at, updated_at)
                 VALUES (?1, 'p-test', 'Test disc', ?2, ?2)",
                rusqlite::params![disc_id, now],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let cfg = Arc::new(RwLock::new(default_config()));
        AppState::new_defaults(cfg, db, DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    async fn sse_body_to_string(resp: Sse<SseStream>) -> String {
        let response = resp.into_response();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect SSE body");
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[tokio::test]
    async fn retry_endpoint_targets_only_the_failed_agent_and_original_turn() {
        let disc = "d-targeted-retry";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                let user = DiscussionMessage {
                    id: "u-targeted-retry".into(),
                    role: MessageRole::User,
                    channel: MessageChannel::Main,
                    content: "@claude @codex @litellm compare".into(),
                    agent_type: None,
                    timestamp: Utc::now(),
                    tokens_used: 0,
                    auth_mode: None,
                    model_tier: None,
                    model: None,
                    cost_usd: None,
                    author_pseudo: None,
                    author_avatar_email: None,
                    source_msg_id: None,
                    duration_ms: None,
                    lint_report: None,
                    target_agent: None,
                    reply_to_message_id: None,
                    author_cli_ordinal: None,
                    session_tokens_at_message: None,
                    recovered_partial: false,
                };
                let sort_order = crate::db::discussions::insert_message(conn, disc, &user)?;
                let lite = crate::db::agent_dispatch::enqueue(
                    conn,
                    crate::db::agent_dispatch::NewAgentDispatchJob {
                        id: "j-lite-failed",
                        discussion_id: disc,
                        trigger_message_id: &user.id,
                        trigger_sort_order: sort_order,
                        dedupe_key: "turn:lite",
                        agent_override: Some(&AgentType::LiteLlm),
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: None,
                        group_concurrency_limit: None,
                    },
                )?;
                crate::db::agent_dispatch::mark_failed(conn, &lite.id, "vpn")?;
                Ok(())
            })
            .await
            .unwrap();

        let response = retry_agent_dispatch(
            State(state.clone()),
            Path(disc.into()),
            Json(RetryAgentDispatchRequest {
                dispatch_id: "j-lite-failed".into(),
                idempotency_key: "77777777-7777-4777-8777-777777777777".into(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let jobs = state
            .db
            .with_conn(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT agent_override_json, trigger_message_id FROM agent_dispatch_jobs
                     WHERE dedupe_key LIKE 'retry:%'",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?))
                })?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .await
            .unwrap();
        assert_eq!(
            jobs,
            vec![(Some("\"LiteLlm\"".into()), "u-targeted-retry".into())]
        );
    }

    /// Orphan subcase: a dispatch claimed Running with its live token registered
    /// between handler entry and the cancel commit must have that token actually
    /// cancelled — not merely removed from the registry, which left the process
    /// running with no way to stop it.
    #[tokio::test]
    async fn stop_agent_dispatch_cancels_the_live_token_not_a_stale_snapshot() {
        let disc = "d-stop-orphan";
        let state = make_state_with_disc(disc).await;
        let claimed = state
            .db
            .with_conn(move |conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, channel, content,
                                           timestamp, sort_order, received_at)
                     VALUES ('u-orphan', ?1, 'User', 'main', 'go', ?2, 0, ?2)",
                    rusqlite::params![disc, now],
                )?;
                crate::db::agent_dispatch::enqueue(
                    conn,
                    crate::db::agent_dispatch::NewAgentDispatchJob {
                        id: "j-orphan",
                        discussion_id: disc,
                        trigger_message_id: "u-orphan",
                        trigger_sort_order: 0,
                        dedupe_key: "turn:orphan",
                        agent_override: None,
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: None,
                        group_concurrency_limit: None,
                    },
                )?;
                Ok(crate::db::agent_dispatch::claim(conn, "j-orphan")?.is_some())
            })
            .await
            .unwrap();
        assert!(claimed, "job must reach Running before the stop");

        // The worker registered its live cancellation token under the dispatch id.
        let token = tokio_util::sync::CancellationToken::new();
        state
            .cancel_registry
            .lock()
            .unwrap()
            .insert("j-orphan".to_string(), token.clone());
        assert!(!token.is_cancelled());

        let _ =
            stop_agent_dispatch(State(state.clone()), Path((disc.into(), "j-orphan".into()))).await;

        // Core assertion: the live process token is actually cancelled, and the
        // DB row is Cancelled (proving the cancel branch ran).
        assert!(
            token.is_cancelled(),
            "the removed token must be cancelled, not orphaned"
        );
        let status = state
            .db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT status FROM agent_dispatch_jobs WHERE id = 'j-orphan'",
                    [],
                    |row| row.get::<_, String>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(status, "Cancelled");
    }

    /// Gate subcase: a job Cancelled before the worker reaches
    /// `mark_agent_started` must fail the start gate (no provider launched), and
    /// the worker's `CancelGuard` must clean its registry entry on scope exit.
    #[tokio::test]
    async fn cancel_before_mark_agent_started_fails_the_start_gate_and_guard_cleans_up() {
        let disc = "d-stop-gate";
        let state = make_state_with_disc(disc).await;
        {
            let _guard = crate::CancelGuard::insert(&state.cancel_registry, "j-gate".to_string());
            assert!(state.cancel_registry.lock().unwrap().contains_key("j-gate"));

            let started = state
                .db
                .with_conn(move |conn| {
                    let now = chrono::Utc::now().to_rfc3339();
                    conn.execute(
                        "INSERT INTO messages (id, discussion_id, role, channel, content,
                                               timestamp, sort_order, received_at)
                         VALUES ('u-gate', ?1, 'User', 'main', 'go', ?2, 0, ?2)",
                        rusqlite::params![disc, now],
                    )?;
                    crate::db::agent_dispatch::enqueue(
                        conn,
                        crate::db::agent_dispatch::NewAgentDispatchJob {
                            id: "j-gate",
                            discussion_id: disc,
                            trigger_message_id: "u-gate",
                            trigger_sort_order: 0,
                            dedupe_key: "turn:gate",
                            agent_override: None,
                            chain_prompt_ids: &[],
                            batch_item: None,
                            group_id: None,
                            group_concurrency_limit: None,
                        },
                    )?;
                    crate::db::agent_dispatch::claim(conn, "j-gate")?; // Pending -> Running
                                                                       // A stop lands before the provider starts.
                    crate::db::agent_dispatch::cancel_for_discussion_by_id(conn, disc, "j-gate")?;
                    // The gate the worker checks before launching the provider.
                    crate::db::agent_dispatch::mark_agent_started(conn, "j-gate")
                })
                .await
                .unwrap();
            assert!(
                !started,
                "a cancelled job must not pass the start gate — no provider launched"
            );
        }
        // CancelGuard removed its entry on drop — nothing dangling in the registry.
        assert!(!state.cancel_registry.lock().unwrap().contains_key("j-gate"));
    }

    #[tokio::test]
    async fn target_all_excludes_the_disabled_discussion_agent() {
        let disc = "d-all-no-agent";
        let state = make_state_with_disc(disc).await;
        let cli_session_id = state
            .db
            .with_conn(move |conn| {
                crate::db::discussions::set_disc_no_agent(conn, disc, true)?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    disc,
                    "Codex",
                    Some("sess-all"),
                    "peer",
                )
            })
            .await
            .unwrap();

        let targets = canonical_targets(&state, disc, Vec::new(), true)
            .await
            .unwrap();
        assert_eq!(
            targets,
            vec![MessageTarget::cli(AgentType::Codex, cli_session_id)]
        );
    }

    #[tokio::test]
    async fn canonical_targets_keep_explicit_tiers_and_default_target_all_punctual_agents() {
        let disc = "d-tiered-targets";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                crate::db::discussions::update_discussion_agent(conn, disc, &AgentType::LiteLlm)?;
                crate::db::discussions::update_discussion_tier(conn, disc, &ModelTier::Reasoning)?;
                crate::db::discussions::update_discussion_participants(
                    conn,
                    disc,
                    &[AgentType::LiteLlm, AgentType::Codex],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let explicit = canonical_targets(
            &state,
            disc,
            vec![MessageTarget::agent(AgentType::Codex).with_tier(ModelTier::Economy)],
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            explicit,
            vec![MessageTarget::agent(AgentType::Codex).with_tier(ModelTier::Economy)]
        );

        let all = canonical_targets(&state, disc, Vec::new(), true)
            .await
            .unwrap();
        assert_eq!(
            all,
            vec![
                MessageTarget::discussion_agent(AgentType::LiteLlm).with_tier(ModelTier::Reasoning),
                MessageTarget::agent(AgentType::Codex).with_tier(ModelTier::Default),
            ]
        );
    }

    async fn response_body_to_string(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect response body");
        String::from_utf8_lossy(&bytes).to_string()
    }

    /// The double-responder fix (2026-06-04): with a LIVE MCP agent on the
    /// disc, send_message must persist the human message + broadcast, then
    /// SKIP the local runner (emit `skipped_live_agents`) so the connected
    /// agent is the sole responder. We assert: (a) the skip event is on the
    /// wire, (b) the User message is persisted, (c) NO Agent reply was added
    /// (the runner never ran).
    #[tokio::test]
    async fn send_message_skips_local_runner_when_live_agent_connected() {
        let disc = "d-live-1";
        let state = make_state_with_disc(disc).await;
        // A live MCP agent is connected (status='active', fresh last_seen
        // from create_session).
        let cli_session_id = state
            .db
            .with_conn(move |conn| {
                let session_id = crate::db::discussion_sessions::create_session(
                    conn,
                    disc,
                    "Codex",
                    Some("sess-x"),
                    "peer",
                )?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    disc,
                    "Codex",
                    Some("sess-x"),
                    "listening",
                    300,
                )?;
                Ok(session_id)
            })
            .await
            .unwrap();

        let resp = send_message(
            State(state.clone()),
            Path(disc.to_string()),
            Json(SendMessageRequest {
                content: "hello peers".into(),
                channel: MessageChannel::Main,
                targets: vec![MessageTarget::cli(AgentType::Codex, cli_session_id)],
                target_all: false,
                target_agents: vec![],
                target_agent: None,
                client_message_id: Some("5fa2fc3c-4b92-4472-9729-faba80bf0525".into()),
                reply_to_message_id: None,
            }),
        )
        .await;
        let body = sse_body_to_string(resp).await;
        assert!(
            body.contains("skipped_live_agents"),
            "expected skip event, got: {body}"
        );
        assert!(body.contains("live_mcp_agents"), "skip reason present");
        let accepted_pos = body.find("event: accepted").expect("accepted event");
        let skipped_pos = body.find("event: skipped_live_agents").expect("skip event");
        assert!(
            accepted_pos < skipped_pos,
            "accepted receipt must be the first event: {body}"
        );

        // User message persisted, and NO Agent message (runner never ran).
        let msgs = state
            .db
            .with_conn(move |conn| crate::db::discussions::list_messages(conn, disc))
            .await
            .unwrap();
        assert_eq!(msgs.len(), 1, "only the human message is persisted");
        assert_eq!(msgs[0].role, MessageRole::User);
        assert!(
            !msgs.iter().any(|m| m.role == MessageRole::Agent),
            "no agent reply — the connected agent answers, not Kronn's runner"
        );
    }

    #[tokio::test]
    async fn retrying_client_message_id_returns_duplicate_receipt_without_rerun() {
        let disc = "d-idempotent";
        let state = make_state_with_disc(disc).await;
        let cli_session_id = state
            .db
            .with_conn(move |conn| {
                let session_id = crate::db::discussion_sessions::create_session(
                    conn,
                    disc,
                    "Codex",
                    Some("sess-idempotent"),
                    "peer",
                )?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    disc,
                    "Codex",
                    Some("sess-idempotent"),
                    "listening",
                    300,
                )?;
                Ok(session_id)
            })
            .await
            .unwrap();

        let client_message_id = "e8618b06-f4ce-42c8-9cb2-cbc0062bc995";
        let first = send_message(
            State(state.clone()),
            Path(disc.to_string()),
            Json(SendMessageRequest {
                content: "only once".into(),
                channel: MessageChannel::Main,
                targets: vec![MessageTarget::cli(AgentType::Codex, cli_session_id)],
                target_all: false,
                target_agents: vec![],
                target_agent: None,
                client_message_id: Some(client_message_id.into()),
                reply_to_message_id: None,
            }),
        )
        .await;
        let first_body = sse_body_to_string(first).await;
        assert!(first_body.contains(r#""duplicate":false"#));
        assert!(first_body.contains("skipped_live_agents"));

        let retry = send_message(
            State(state.clone()),
            Path(disc.to_string()),
            Json(SendMessageRequest {
                content: "only once".into(),
                channel: MessageChannel::Main,
                targets: vec![MessageTarget::cli(AgentType::Codex, cli_session_id)],
                target_all: false,
                target_agents: vec![],
                target_agent: None,
                client_message_id: Some(client_message_id.into()),
                reply_to_message_id: None,
            }),
        )
        .await;
        let retry_body = sse_body_to_string(retry).await;
        assert!(retry_body.contains("event: accepted"));
        assert!(retry_body.contains(r#""duplicate":true"#));
        assert!(
            !retry_body.contains("skipped_live_agents"),
            "duplicate request must stop after the receipt: {retry_body}"
        );

        let messages = state
            .db
            .with_conn(move |conn| crate::db::discussions::list_messages(conn, disc))
            .await
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, client_message_id);
    }

    #[tokio::test]
    async fn revise_message_is_idempotent_and_rejects_divergent_key_reuse() {
        let disc = "d-revise-handler";
        let state = make_state_with_disc(disc).await;
        let message_id = "2878216c-8d4e-4479-96af-5c801b98b1f1";
        state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::create_session(
                    conn,
                    disc,
                    "ClaudeCode",
                    Some("live-reviser"),
                    "peer",
                )?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    disc,
                    "ClaudeCode",
                    Some("live-reviser"),
                    "listening",
                    300,
                )?;
                crate::db::discussions::insert_message(
                    conn,
                    disc,
                    &DiscussionMessage {
                        recovered_partial: false,
                        session_tokens_at_message: None,
                        author_cli_ordinal: None,
                        id: message_id.into(),
                        role: MessageRole::User,
                        channel: MessageChannel::Main,
                        content: "before".into(),
                        timestamp: Utc::now(),
                        model: None,
                        lint_report: None,
                        agent_type: None,
                        tokens_used: 0,
                        auth_mode: None,
                        model_tier: None,
                        cost_usd: None,
                        author_pseudo: None,
                        author_avatar_email: None,
                        source_msg_id: None,
                        duration_ms: None,
                        target_agent: None,
                        reply_to_message_id: None,
                    },
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let expected_revision = state
            .db
            .with_conn(move |conn| {
                let messages = crate::db::discussions::list_messages(conn, disc)?;
                Ok(serde_json::to_value(&messages[0])?["timestamp"]
                    .as_str()
                    .expect("serialized message timestamp")
                    .to_string())
            })
            .await
            .unwrap();
        assert!(expected_revision.ends_with('Z'));
        let idempotency_key = "de60829d-250d-41b1-bb45-632c22c59f7c";
        let request = ReviseMessageRequest {
            message_id: message_id.into(),
            content: "after".into(),
            expected_revision,
            idempotency_key: idempotency_key.into(),
            targets: vec![],
            target_all: false,
            target_agent: None,
            target_agents: vec![],
        };

        let first = revise_message(
            State(state.clone()),
            Path(disc.into()),
            Json(request.clone()),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = response_body_to_string(first).await;
        assert!(first_body.contains("event: message_revised"));
        assert!(first_body.contains("\"duplicate\":false"));

        let retry = revise_message(
            State(state.clone()),
            Path(disc.into()),
            Json(request.clone()),
        )
        .await;
        assert_eq!(retry.status(), StatusCode::OK);
        let retry_body = response_body_to_string(retry).await;
        assert!(retry_body.contains("\"duplicate\":true"));

        let conflict = revise_message(
            State(state),
            Path(disc.into()),
            Json(ReviseMessageRequest {
                content: "different payload".into(),
                ..request
            }),
        )
        .await;
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn no_agent_receives_accepted_before_skip() {
        let disc = "d-no-agent";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                conn.execute("UPDATE discussions SET no_agent = 1 WHERE id = ?1", [disc])?;
                Ok(())
            })
            .await
            .unwrap();

        let response = send_message(
            State(state),
            Path(disc.to_string()),
            Json(SendMessageRequest {
                content: "human only".into(),
                channel: MessageChannel::Main,
                targets: vec![],
                target_all: false,
                target_agents: vec![],
                target_agent: None,
                client_message_id: Some("6661b620-162d-4a8a-9552-33f0896c6835".into()),
                reply_to_message_id: None,
            }),
        )
        .await;
        let body = sse_body_to_string(response).await;
        let accepted_pos = body.find("event: accepted").expect("accepted event");
        let skipped_pos = body
            .find("event: skipped_no_agent")
            .expect("no-agent skip event");
        assert!(
            accepted_pos < skipped_pos,
            "accepted receipt must precede no-agent skip: {body}"
        );
    }

    #[tokio::test]
    async fn send_message_persists_explicit_target_agent() {
        let disc = "d-target-stamp";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                // Keep the test deterministic: persistence is under test, not
                // launching a real Codex process.
                conn.execute("UPDATE discussions SET no_agent = 1 WHERE id = ?1", [disc])?;
                Ok(())
            })
            .await
            .unwrap();

        let response = send_message(
            State(state.clone()),
            Path(disc.to_string()),
            Json(SendMessageRequest {
                content: "@codex peux-tu vérifier ?".into(),
                channel: MessageChannel::Main,
                targets: vec![],
                target_all: false,
                target_agents: vec![],
                target_agent: Some(AgentType::Codex),
                client_message_id: Some("c4a768c8-48b5-4d64-9fe4-121ebf9c36ac".into()),
                reply_to_message_id: None,
            }),
        )
        .await;
        let body = sse_body_to_string(response).await;
        assert!(body.contains("event: accepted"));
        assert!(
            body.contains("target_not_joined"),
            "an absent target in a no-agent room needs an actionable reason: {body}"
        );

        let messages = state
            .db
            .with_conn(move |conn| crate::db::discussions::list_messages(conn, disc))
            .await
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].target_agent, Some(AgentType::Codex));
    }

    #[tokio::test]
    async fn accepted_receipt_precedes_agent_preflight_error() {
        let state = make_state_with_disc("existing-disc").await;
        let (response, _completion) =
            super::super::streaming::make_agent_stream_tracked_with_initial_event(
                state,
                "missing-disc".into(),
                None,
                None,
                "test-dispatch".into(),
                accepted_event("3d966785-b7b4-446b-8303-ac28f02a5427", 9, false),
            )
            .await;
        let body = sse_body_to_string(response).await;
        let accepted_pos = body.find("event: accepted").expect("accepted event");
        let error_pos = body.find("event: error").expect("preflight error event");
        assert!(
            accepted_pos < error_pos,
            "accepted receipt must precede runner preflight errors: {body}"
        );
    }

    #[tokio::test]
    async fn invalid_client_message_id_is_rejected_without_insert() {
        let disc = "d-invalid-client-id";
        let state = make_state_with_disc(disc).await;
        let response = send_message(
            State(state.clone()),
            Path(disc.to_string()),
            Json(SendMessageRequest {
                content: "must not persist".into(),
                channel: MessageChannel::Main,
                targets: vec![],
                target_all: false,
                target_agents: vec![],
                target_agent: None,
                client_message_id: Some("not-a-uuid".into()),
                reply_to_message_id: None,
            }),
        )
        .await;
        let body = sse_body_to_string(response).await;
        assert!(body.contains("Invalid client_message_id"));
        assert!(!body.contains("event: accepted"));

        let messages = state
            .db
            .with_conn(move |conn| crate::db::discussions::list_messages(conn, disc))
            .await
            .unwrap();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn send_message_persists_a_reply_target_from_the_same_discussion() {
        let disc = "d-reply-target";
        // Federated and legacy messages can carry opaque, non-UUID ids.
        let source_id = "wsl-1726cd13dff6";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                let now = Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (
                         id, discussion_id, role, content, timestamp, sort_order
                     ) VALUES (?1, ?2, 'Agent', 'Original answer', ?3, 1)",
                    rusqlite::params![source_id, disc, now],
                )?;
                conn.execute(
                    "UPDATE discussions
                     SET no_agent = 1, next_message_seq = 2, message_count = 1
                     WHERE id = ?1",
                    [disc],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let response = send_message(
            State(state.clone()),
            Path(disc.to_string()),
            Json(SendMessageRequest {
                content: "Follow-up".into(),
                channel: MessageChannel::Main,
                targets: vec![],
                target_all: false,
                target_agents: vec![],
                target_agent: None,
                client_message_id: Some("22222222-2222-4222-8222-222222222222".into()),
                reply_to_message_id: Some(source_id.into()),
            }),
        )
        .await;
        let body = sse_body_to_string(response).await;
        assert!(body.contains("event: accepted"));

        let messages = state
            .db
            .with_conn(move |conn| crate::db::discussions::list_messages(conn, disc))
            .await
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].reply_to_message_id.as_deref(), Some(source_id));
    }

    #[tokio::test]
    async fn send_message_rejects_a_reply_target_outside_the_discussion() {
        let disc = "d-missing-reply-target";
        let state = make_state_with_disc(disc).await;

        let response = send_message(
            State(state.clone()),
            Path(disc.to_string()),
            Json(SendMessageRequest {
                content: "Follow-up".into(),
                channel: MessageChannel::Main,
                targets: vec![],
                target_all: false,
                target_agents: vec![],
                target_agent: None,
                client_message_id: Some("33333333-3333-4333-8333-333333333333".into()),
                reply_to_message_id: Some("44444444-4444-4444-8444-444444444444".into()),
            }),
        )
        .await;
        let body = sse_body_to_string(response).await;
        assert!(body.contains("Reply target not found in this discussion"));
        assert!(!body.contains("event: accepted"));

        let messages = state
            .db
            .with_conn(move |conn| crate::db::discussions::list_messages(conn, disc))
            .await
            .unwrap();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn stop_cancels_only_the_current_durable_turn() {
        let disc = "d-stop-durable";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                let first = DiscussionMessage {
                    recovered_partial: false,
                    session_tokens_at_message: None,
                    author_cli_ordinal: None,
                    id: "d9158714-19d4-4dbf-9b7d-0839c93458b7".into(),
                    role: MessageRole::User,
                    channel: MessageChannel::Main,
                    content: "first".into(),
                    timestamp: Utc::now(),
                    model: None,
                    lint_report: None,
                    agent_type: None,
                    tokens_used: 0,
                    auth_mode: None,
                    model_tier: None,
                    cost_usd: None,
                    author_pseudo: None,
                    author_avatar_email: None,
                    source_msg_id: None,
                    duration_ms: None,
                    target_agent: None,
                    reply_to_message_id: None,
                };
                let second = DiscussionMessage {
                    id: "812ee54a-62d6-4426-bf19-b970416958d7".into(),
                    content: "second".into(),
                    ..first.clone()
                };
                crate::db::discussions::insert_user_message_with_dispatch(
                    conn,
                    disc,
                    &first,
                    "j-running",
                    None,
                )?;
                crate::db::discussions::insert_user_message_with_dispatch(
                    conn,
                    disc,
                    &second,
                    "j-pending",
                    None,
                )?;
                crate::db::workflows::ensure_batch_placeholder_workflow(
                    conn,
                    "qp-stop",
                    "Stop batch",
                    None,
                )?;
                crate::db::workflows::insert_run(
                    conn,
                    &crate::models::WorkflowRun {
                        id: "batch-stop".into(),
                        workflow_id: "qp:qp-stop".into(),
                        status: crate::models::RunStatus::Running,
                        trigger_context: None,
                        step_results: vec![],
                        tokens_used: 0,
                        workspace_path: None,
                        started_at: Utc::now(),
                        finished_at: None,
                        run_type: "batch".into(),
                        batch_total: 2,
                        batch_completed: 0,
                        batch_failed: 0,
                        batch_no_response: 0,
                        batch_name: Some("Stop batch".into()),
                        parent_run_id: None,
                        state: std::collections::HashMap::new(),
                        produced_branches: vec![],
                        parent_workflow_id: None,
                        parent_workflow_name: None,
                        parent_run_started_at: None,
                    },
                )?;
                conn.execute(
                    "UPDATE agent_dispatch_jobs
                     SET group_id = 'batch-stop'
                     WHERE id IN ('j-running', 'j-pending')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        state
            .db
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER reject_stop_batch_progress
                     BEFORE UPDATE ON workflow_runs
                     BEGIN
                       SELECT RAISE(ABORT, 'forced cancellation progress failure');
                     END;",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let refused = stop_agent(State(state.clone()), Path(disc.into())).await;
        assert_eq!(
            refused.0.data.unwrap()["cancelled"],
            serde_json::Value::Bool(false),
            "a failed batch update must roll the cancellation back"
        );
        state
            .db
            .with_conn(|conn| {
                let job = crate::db::agent_dispatch::get(conn, "j-running")?.unwrap();
                assert!(
                    matches!(
                        job.status,
                        crate::db::agent_dispatch::DispatchStatus::Pending
                            | crate::db::agent_dispatch::DispatchStatus::Running
                    ),
                    "the job must remain retryable after rollback"
                );
                let batch = crate::db::workflows::get_run(conn, "batch-stop")?.unwrap();
                assert_eq!(batch.batch_failed, 0);
                conn.execute_batch("DROP TRIGGER reject_stop_batch_progress;")?;
                Ok(())
            })
            .await
            .unwrap();

        let response = stop_agent(State(state.clone()), Path(disc.into())).await;
        assert_eq!(
            response.0.data.unwrap()["cancelled"],
            serde_json::Value::Bool(true)
        );
        let (running, pending, awaiting, batch) = state
            .db
            .with_conn(move |conn| {
                let running = crate::db::agent_dispatch::get(conn, "j-running")?.unwrap();
                let pending = crate::db::agent_dispatch::get(conn, "j-pending")?.unwrap();
                let awaiting = conn.query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = ?1",
                    [disc],
                    |row| row.get::<_, bool>(0),
                )?;
                let batch = crate::db::workflows::get_run(conn, "batch-stop")?.unwrap();
                Ok((running, pending, awaiting, batch))
            })
            .await
            .unwrap();
        assert_eq!(
            running.status,
            crate::db::agent_dispatch::DispatchStatus::Cancelled
        );
        assert_eq!(
            pending.status,
            crate::db::agent_dispatch::DispatchStatus::Pending
        );
        assert!(awaiting, "the queued follow-up is still owed");
        assert_eq!(batch.batch_failed, 1);
        assert_eq!(batch.status, crate::models::RunStatus::Running);
    }

    #[tokio::test]
    async fn stop_repairs_stale_awaiting_marker_without_an_active_job() {
        let disc = "d-stale-awaiting";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| crate::db::discussions::set_awaiting_agent(conn, disc, true))
            .await
            .unwrap();

        let response = stop_agent(State(state.clone()), Path(disc.into())).await;
        assert_eq!(
            response.0.data.unwrap()["cancelled"],
            serde_json::Value::Bool(false)
        );

        let awaiting = state
            .db
            .with_conn(move |conn| {
                conn.query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = ?1",
                    [disc],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(anyhow::Error::from)
            })
            .await
            .unwrap();
        assert!(!awaiting);
    }

    #[tokio::test]
    async fn targeted_stop_cancels_only_the_selected_reply_and_its_process() {
        let disc = "d-targeted-stop";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                let now = Utc::now();
                for (sort_order, (message_id, job_id)) in
                    [(1, ("u-old", "j-old")), (2, ("u-new", "j-new"))]
                {
                    conn.execute(
                        "INSERT INTO messages
                         (id, discussion_id, role, channel, content, timestamp, sort_order)
                         VALUES (?1, ?2, 'User', 'main', ?1, ?3, ?4)",
                        rusqlite::params![message_id, disc, now.to_rfc3339(), sort_order],
                    )?;
                    crate::db::agent_dispatch::enqueue(
                        conn,
                        crate::db::agent_dispatch::NewAgentDispatchJob {
                            id: job_id,
                            discussion_id: disc,
                            trigger_message_id: message_id,
                            trigger_sort_order: sort_order,
                            dedupe_key: job_id,
                            agent_override: Some(&AgentType::ClaudeCode),
                            chain_prompt_ids: &[],
                            batch_item: None,
                            group_id: None,
                            group_concurrency_limit: None,
                        },
                    )?;
                }
                crate::db::discussions::set_awaiting_agent(conn, disc, true)?;
                crate::db::agent_dispatch::claim(conn, "j-old")?;
                Ok(())
            })
            .await
            .unwrap();

        let old_token = tokio_util::sync::CancellationToken::new();
        let new_token = tokio_util::sync::CancellationToken::new();
        {
            let mut registry = state.cancel_registry.lock().unwrap();
            registry.insert("j-old".into(), old_token.clone());
            registry.insert("j-new".into(), new_token.clone());
        }

        let response =
            stop_agent_dispatch(State(state.clone()), Path((disc.into(), "j-old".into()))).await;
        let data = response.0.data.unwrap();
        assert_eq!(data["cancelled"], true);
        assert_eq!(data["still_awaiting"], true);
        assert!(old_token.is_cancelled());
        assert!(
            !new_token.is_cancelled(),
            "the newer sibling must keep running"
        );

        state
            .db
            .with_conn(move |conn| {
                assert_eq!(
                    crate::db::agent_dispatch::get(conn, "j-old")?
                        .unwrap()
                        .status,
                    crate::db::agent_dispatch::DispatchStatus::Cancelled,
                );
                assert_eq!(
                    crate::db::agent_dispatch::get(conn, "j-new")?
                        .unwrap()
                        .status,
                    crate::db::agent_dispatch::DispatchStatus::Pending,
                );
                assert!(crate::db::agent_dispatch::has_active_for_discussion(
                    conn, disc
                )?);
                Ok(())
            })
            .await
            .unwrap();
    }

    /// A `paused` session is NOT a live responder, so send_message must
    /// NOT skip. We can't drive make_agent_stream (it launches a real CLI)
    /// in a unit test, so we assert the decision input directly: with only
    /// a paused session, count_live_participants is 0 → the guard is not
    /// taken. (The skip-path behaviour itself is covered above.)
    #[tokio::test]
    async fn send_message_does_not_skip_when_only_paused_agent() {
        let disc = "d-paused-1";
        let state = make_state_with_disc(disc).await;
        let pk = state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::create_session(
                    conn,
                    disc,
                    "Codex",
                    Some("sess-p"),
                    "peer",
                )
            })
            .await
            .unwrap();
        state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::set_session_status(conn, pk, "paused")
            })
            .await
            .unwrap();

        let live = state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::count_live_participants(conn, disc)
            })
            .await
            .unwrap();
        assert_eq!(
            live, 0,
            "paused agent is not a live responder → Kronn would still answer"
        );
    }

    #[tokio::test]
    async fn typed_target_identity_selects_exact_responder_class() {
        let disc = "d-targeted-policy";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::create_session(
                    conn,
                    disc,
                    "ClaudeCode",
                    Some("claude-live"),
                    "owner",
                )?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    disc,
                    "ClaudeCode",
                    Some("claude-live"),
                    "listening",
                    300,
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let untargeted = human_dispatch_route(&state, disc, None).await;
        let codex =
            human_dispatch_route(&state, disc, Some(&MessageTarget::agent(AgentType::Codex))).await;
        let claude_cli = human_dispatch_route(
            &state,
            disc,
            Some(&MessageTarget::cli(AgentType::ClaudeCode, 1)),
        )
        .await;

        assert_eq!(untargeted, DispatchRoute::NativePrincipal);
        assert_eq!(
            codex,
            DispatchRoute::TargetedNative(AgentType::Codex),
            "a punctual Codex target must not be swallowed by a joined Claude CLI"
        );
        assert_eq!(
            claude_cli,
            DispatchRoute::JoinedPeers,
            "an exact CLI identity owns only its explicitly addressed turn"
        );
    }

    #[tokio::test]
    async fn paced_waiting_peer_owns_turn_until_its_poll_deadline_expires() {
        let disc = "d-paced-responder";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::create_session(
                    conn,
                    disc,
                    "ClaudeCode",
                    Some("claude-paced"),
                    "peer",
                )?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    disc,
                    "ClaudeCode",
                    Some("claude-paced"),
                    "waiting",
                    -1,
                )?;
                crate::db::discussion_sessions::set_next_poll_at(
                    conn,
                    disc,
                    "ClaudeCode",
                    Some("claude-paced"),
                    chrono::Utc::now() + chrono::Duration::seconds(60),
                )
            })
            .await
            .unwrap();

        let target = MessageTarget::cli(AgentType::ClaudeCode, 1);
        let waiting_route = human_dispatch_route(&state, disc, Some(&target)).await;
        assert_eq!(
            waiting_route,
            DispatchRoute::JoinedPeers,
            "a human turn inside the server-paced gap must not spawn a duplicate native agent"
        );

        state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::set_next_poll_at(
                    conn,
                    disc,
                    "ClaudeCode",
                    Some("claude-paced"),
                    chrono::Utc::now() - chrono::Duration::seconds(121),
                )
            })
            .await
            .unwrap();
        let expired_route = human_dispatch_route(&state, disc, Some(&target)).await;
        assert_eq!(
            expired_route,
            DispatchRoute::JoinedPeers,
            "a durable CLI addressee remains the owner across pacing windows"
        );
    }

    #[tokio::test]
    async fn expired_peer_activity_falls_back_without_removing_sticky_membership() {
        let disc = "d-expired-responder";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::create_session(
                    conn,
                    disc,
                    "ClaudeCode",
                    Some("claude-expired"),
                    "peer",
                )?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    disc,
                    "ClaudeCode",
                    Some("claude-expired"),
                    "listening",
                    -100,
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let targeted_route = human_dispatch_route(
            &state,
            disc,
            Some(&MessageTarget::agent(AgentType::ClaudeCode)),
        )
        .await;
        let untargeted_route = human_dispatch_route(&state, disc, None).await;
        let sticky_members = state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::count_live_participants(conn, disc)
            })
            .await
            .unwrap();

        assert_eq!(
            targeted_route,
            DispatchRoute::TargetedNative(AgentType::ClaudeCode)
        );
        assert_eq!(untargeted_route, DispatchRoute::NativePrincipal);
        assert_eq!(
            sticky_members, 1,
            "dispatch freshness must not detach the participant from the room"
        );
    }

    /// A run that dies in make_agent_stream's
    /// preflight (here: Isolated disc whose worktree re-lock fails because
    /// the project path isn't a git repo) must NOT leave awaiting_agent=1,
    /// or the next boot reconcile appends a bogus interruption notice.
    /// The marker is set after every preflight early-return, so this path
    /// never touches it.
    #[tokio::test]
    async fn run_agent_preflight_failure_leaves_no_awaiting_marker() {
        let disc = "d-relock-fail";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "UPDATE discussions SET workspace_mode = 'Isolated',
                            workspace_path = NULL, worktree_branch = 'kronn/test-relock'
                     WHERE id = ?1",
                    rusqlite::params![disc],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let resp = run_agent(State(state.clone()), Path(disc.to_string()), None).await;
        let body = sse_body_to_string(resp).await;
        assert!(
            body.contains("error"),
            "re-lock preflight must fail, got: {body}"
        );

        let awaiting: i64 = state
            .db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = ?1",
                    rusqlite::params![disc],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(
            awaiting, 0,
            "failed preflight must not leave the disc marked as owed a run"
        );
    }

    /// A BATCH child is pre-marked awaiting_agent=1 at enqueue
    /// (create_batch_run) and its SSE stream has no consumer. A preflight
    /// failure must behave like the agent-start-failed arm: persist the
    /// error in the thread, clear the marker (no bogus boot notice) and
    /// bump the batch counters (no run stuck at n-1/N).
    #[tokio::test]
    async fn batch_child_preflight_failure_persists_error_and_settles_the_batch() {
        let disc = "d-batch-relock";
        let state = make_state_with_disc(disc).await;
        state
            .db
            .with_conn(move |conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
                     VALUES ('wf-r12', 'r12', '{}', '[]', ?1, ?1)",
                    rusqlite::params![now],
                )?;
                conn.execute(
                    "INSERT INTO workflow_runs (id, workflow_id, run_type, status, started_at, batch_total)
                     VALUES ('run-r12', 'wf-r12', 'batch', 'Running', ?1, 1)",
                    rusqlite::params![now],
                )?;
                conn.execute(
                    "UPDATE discussions SET workflow_run_id = 'run-r12', awaiting_agent = 1,
                            workspace_mode = 'Isolated', workspace_path = NULL,
                            worktree_branch = 'kronn/test-relock'
                     WHERE id = ?1",
                    rusqlite::params![disc],
                )?;
                crate::db::discussions::insert_message(
                    conn,
                    disc,
                    &DiscussionMessage {
                        recovered_partial: false,
                        session_tokens_at_message: None,
                        author_cli_ordinal: None,
                        id: "user-batch-relock".into(),
                        role: MessageRole::User,
                        channel: MessageChannel::Main,
                        content: "run batch child".into(),
                        timestamp: Utc::now(),
                        model: None,
                        lint_report: None,
                        agent_type: None,
                        tokens_used: 0,
                        auth_mode: None,
                        model_tier: None,
                        cost_usd: None,
                        author_pseudo: None,
                        author_avatar_email: None,
                        source_msg_id: None,
                        duration_ms: None,
                        target_agent: None,
        reply_to_message_id: None,
                    },
                )?;
                crate::db::agent_dispatch::enqueue_for_latest_user(
                    conn,
                    crate::db::agent_dispatch::NewLatestUserDispatch {
                        id: "job-batch-relock",
                        discussion_id: disc,
                        dedupe_key: "message:user-batch-relock",
                        agent_override: None,
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: Some("run-r12"),
                        group_concurrency_limit: None,
                    },
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let resp = crate::api::discussions::runtime::stream_dispatch_job(
            state.clone(),
            "job-batch-relock".to_string(),
            None,
        )
        .await
        .expect("durable batch child stream");
        let body = sse_body_to_string(resp).await;
        assert!(
            body.contains("error"),
            "re-lock preflight must fail, got: {body}"
        );

        let settled = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let failed = state
                    .db
                    .with_conn(|conn| {
                        Ok(conn.query_row(
                            "SELECT batch_failed FROM workflow_runs WHERE id = 'run-r12'",
                            [],
                            |row| row.get::<_, i64>(0),
                        )?)
                    })
                    .await
                    .unwrap();
                if failed == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(settled.is_ok(), "durable preflight settlement timed out");

        let (awaiting, batch_failed): (i64, i64) = state
            .db
            .with_conn(move |conn| {
                let awaiting = conn.query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = ?1",
                    rusqlite::params![disc],
                    |r| r.get(0),
                )?;
                let failed = conn.query_row(
                    "SELECT batch_failed FROM workflow_runs WHERE id = 'run-r12'",
                    [],
                    |r| r.get(0),
                )?;
                Ok((awaiting, failed))
            })
            .await
            .unwrap();
        assert_eq!(
            awaiting, 0,
            "enqueue-time marker must be cleared on preflight failure"
        );
        assert_eq!(
            batch_failed, 1,
            "the child must count as failed so the batch can finish"
        );

        let msgs = state
            .db
            .with_conn(move |conn| crate::db::discussions::list_messages(conn, disc))
            .await
            .unwrap();
        assert!(
            msgs.iter()
                .any(|m| m.role == MessageRole::System && m.content.starts_with("Erreur:")),
            "the preflight error must be persisted in the thread (fire-and-forget child)"
        );
    }
}
