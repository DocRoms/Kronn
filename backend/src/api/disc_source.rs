//! 0.8.4 (#294) — Cross-agent memory HTTP routes.
//!
//! 7 endpoints that let an external CLI agent (Claude Code, Cursor,
//! Codex, …) push its conversation history into Kronn so the SAME
//! discussion thread can be picked up by a DIFFERENT agent later.
//! Wired through `disc-introspection-mcp.py` so each route is also a
//! standard MCP tool reachable from any compatible agent runtime.
//!
//! Endpoints:
//!
//! - `POST /api/disc/create` — create a fresh disc, optionally bound
//!   to a source session.
//! - `POST /api/disc/append` — append messages, idempotent on
//!   `(disc_id, source_msg_id)`.
//! - `POST /api/disc/link` — bind an existing disc to a source session.
//! - `POST /api/disc/unlink` — release the binding.
//! - `GET  /api/disc/find_by_session` — lookup by
//!   (source_agent, source_session_id).
//! - `GET  /api/disc/search` — LIKE search across titles + content.
//! - `GET  /api/disc/load_other` — load N messages from a disc other
//!   than the current one.
//!
//! Each route returns the standard `ApiResponse<T>` envelope so the
//! MCP bridge can unwrap success/error uniformly.

use axum::{
    extract::{Query, State},
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

use super::discussions::messaging::canonical_targets;
use super::discussions::routing::{route_human_turn, route_joined_peer_turn, DispatchRoute};

/// Body of `POST /api/disc/create`. The triple `(source_agent,
/// source_session_id, project_id)` is enough to disambiguate: if a
/// disc already exists for the (agent, session) pair, we return its
/// id instead of creating a duplicate.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscCreateRequest {
    pub title: String,
    pub agent: AgentType,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    /// When set, the new disc is immediately bound to this
    /// (source_agent, source_session_id) pair.
    #[serde(default)]
    pub source_agent: Option<String>,
    #[serde(default)]
    pub source_session_id: Option<String>,
    /// Disable Kronn's native discussion runner. Multi-agent rooms created
    /// through `disc_create_room` set this so only explicitly joined peers
    /// answer live MCP appends.
    #[serde(default)]
    pub no_agent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscCreateResponse {
    pub disc_id: String,
    /// `true` when a fresh row was inserted; `false` when an existing
    /// disc was returned because (source_agent, source_session_id)
    /// already mapped.
    pub created: bool,
}

/// `POST /api/disc/create`
pub async fn disc_create(
    State(state): State<AppState>,
    Json(req): Json<DiscCreateRequest>,
) -> Json<ApiResponse<DiscCreateResponse>> {
    // Idempotency: if a binding for this (agent, session) is already
    // open, return its disc rather than creating a duplicate. This is
    // what makes `disc_create` safe to call on every CLI session
    // bootstrap.
    if let (Some(src_agent), Some(src_sess)) = (
        req.source_agent.as_deref(),
        req.source_session_id.as_deref(),
    ) {
        let src_agent = src_agent.to_string();
        let src_sess = src_sess.to_string();
        let lookup = state
            .db
            .with_conn(move |conn| {
                crate::db::disc_source::find_disc_by_source_session(conn, &src_agent, &src_sess)
            })
            .await;
        if let Ok(Some(disc_id)) = lookup {
            return Json(ApiResponse::ok(DiscCreateResponse {
                disc_id,
                created: false,
            }));
        }
    }

    let now = Utc::now();
    let language = req.language.unwrap_or_else(|| "en".to_string());
    let disc_id = Uuid::new_v4().to_string();
    let agent = req.agent.clone();
    let no_agent = req.no_agent;
    let disc = Discussion {
        awaiting_agent: false,
        id: disc_id.clone(),
        project_id: req.project_id.clone(),
        title: req.title.clone(),
        agent: agent.clone(),
        language,
        participants: vec![agent],
        messages: vec![],
        message_count: 0,
        non_system_message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: "Direct".to_string(),
        workspace_path: None,
        worktree_branch: None,
        tier: ModelTier::Reasoning,
        model: None,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: SummaryStrategy::default(),
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };

    let disc_for_insert = disc.clone();
    let inserted = state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::insert_discussion(conn, &disc_for_insert)?;
            if no_agent {
                crate::db::discussions::set_disc_no_agent(conn, &disc_for_insert.id, true)?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await;
    if let Err(e) = inserted {
        return Json(ApiResponse::err(format!("DB error inserting disc: {}", e)));
    }

    // Bind to source if requested. Failure to bind is fatal because
    // the caller is going to rely on `find_by_session` to find this
    // disc next time — silent skip would leave them orphaned.
    if let (Some(src_agent), Some(src_sess)) =
        (req.source_agent.clone(), req.source_session_id.clone())
    {
        let disc_for_bind = disc_id.clone();
        let bind_result = state
            .db
            .with_conn(move |conn| {
                crate::db::disc_source::bind_to_source(conn, &disc_for_bind, &src_agent, &src_sess)
            })
            .await;
        if let Err(e) = bind_result {
            return Json(ApiResponse::err(format!("DB error binding source: {}", e)));
        }
    }

    Json(ApiResponse::ok(DiscCreateResponse {
        disc_id,
        created: true,
    }))
}

/// One message in a `disc_append` payload. `source_msg_id` is REQUIRED
/// because it's how the dedup pass works — without it we'd duplicate
/// every message on every reconnect.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscAppendMessage {
    pub source_msg_id: String,
    pub role: MessageRole,
    pub content: String,
    #[serde(default)]
    pub agent_type: Option<AgentType>,
    /// Authoritative responder identities for this live peer turn. This is the
    /// same typed model as human messages, so `@codex` (native) and one exact
    /// joined Codex CLI session are never conflated.
    #[serde(default)]
    pub targets: Vec<MessageTarget>,
    /// Explicit one-shot responder requested by a structured `@agent`
    /// mention. Compatibility projection for older bridges; new callers use
    /// `targets`.
    #[serde(default)]
    pub target_agent: Option<AgentType>,
    /// Durable id of an existing message in this same discussion. Message ids
    /// are opaque (federated peers may use non-UUID ids).
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscAppendRequest {
    pub disc_id: String,
    pub messages: Vec<DiscAppendMessage>,
    /// Calling bridge session. New bridges always send this so heartbeat and
    /// activity cleanup cannot affect a sibling of the same agent type.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Compact lint feedback echoed to the POSTING agent (tool result), so it can
/// self-correct unverifiable `[src:]` citations in its next message. The full
/// report rides the stored message (UI badge), same as streaming replies.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AppendLintSummary {
    pub fabricated_count: u32,
    pub unsourced_count: u32,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscAppendResponse {
    pub appended: u32,
    pub skipped_as_duplicates: u32,
    /// When true, the disc has been edited inside Kronn since the
    /// last import — the caller should warn the user before pushing
    /// MORE messages (they might be applying stale state on top).
    pub diverged: bool,
    /// Present only for a live single Agent append whose lint had a signal.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub lint: Option<AppendLintSummary>,
    /// `sort_order` of the LAST appended message (stab-1). This is a write
    /// receipt, not a read cursor: another message may have landed between
    /// the caller's last read and this append. `None` when nothing was appended.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_sort_order: Option<i64>,
}

fn is_live_peer_turn(single_agent_append: bool, session_id: Option<&str>, appended: u32) -> bool {
    single_agent_append && session_id.is_some() && appended == 1
}

/// `POST /api/disc/append`
pub async fn disc_append(
    State(state): State<AppState>,
    Json(req): Json<DiscAppendRequest>,
) -> Json<ApiResponse<DiscAppendResponse>> {
    let did = req.disc_id.clone();
    let exists = state
        .db
        .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &did))
        .await;
    let disc = match exists {
        Ok(Some(d)) => d,
        Ok(None) => return Json(ApiResponse::err("Discussion not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    // Validate every reply before inserting anything so a malformed bulk call
    // cannot partially append. The relation always targets an already durable
    // local message; bulk transcript remapping belongs to portable import.
    let reply_targets = req
        .messages
        .iter()
        .filter_map(|message| message.reply_to_message_id.clone())
        .collect::<Vec<_>>();
    if !reply_targets.is_empty() {
        let did = req.disc_id.clone();
        let missing = state
            .db
            .with_conn(move |conn| {
                let mut statement = conn.prepare(
                    "SELECT EXISTS(
                         SELECT 1 FROM messages
                         WHERE id = ?1 AND discussion_id = ?2
                     )",
                )?;
                for target in reply_targets {
                    let exists = statement
                        .query_row(rusqlite::params![target, did], |row| row.get::<_, bool>(0))?;
                    if !exists {
                        return Ok(Some(target));
                    }
                }
                Ok(None)
            })
            .await;
        match missing {
            Ok(Some(target)) => {
                return Json(ApiResponse::err(format!(
                    "Reply target `{target}` not found in this discussion"
                )));
            }
            Ok(None) => {}
            Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
        }
    }

    // 0.8.4 (#294) — `diverged_at` lives on the table but NOT on the
    // `Discussion` struct (see migration 054 + the model comment).
    // Read the column directly so we can warn the caller their import
    // is landing on a user-edited disc.
    let did_div = req.disc_id.clone();
    let diverged = state
        .db
        .with_conn(move |conn| crate::db::disc_source::get_diverged_at(conn, &did_div))
        .await
        .ok()
        .flatten()
        .is_some();
    // Lint-on-append (contract 2026-07-13): ONLY a live single Agent append
    // is linted — bulk imports, User/System messages and project-less discs
    // are exempt — and the insert is NEVER blocked. The full report rides the
    // stored message (UI badge); the summary rides the response (tool result)
    // so the posting agent can self-correct.
    let live_agent_append =
        req.messages.len() == 1 && matches!(req.messages[0].role, MessageRole::Agent);
    // KT-116 — live peer turns now carry the same durable identities as human
    // turns. The compatibility `target_agent` field is projected to one native
    // target only when an older bridge did not send `targets`.
    let routing_candidate = live_agent_append && req.session_id.is_some();
    let mut requested_targets = if routing_candidate && !req.messages[0].targets.is_empty() {
        match canonical_targets(&state, &req.disc_id, req.messages[0].targets.clone(), false).await
        {
            Ok(targets) => targets,
            Err(error) => return Json(ApiResponse::err(error)),
        }
    } else {
        Vec::new()
    };
    let legacy_requested_target = if routing_candidate && requested_targets.is_empty() {
        req.messages[0]
            .target_agent
            .clone()
            .filter(|target| req.messages[0].agent_type.as_ref() != Some(target))
    } else {
        None
    };
    // KT-127 — bind this live MCP append to the exact durable CLI session that
    // authored it. Provider identity alone cannot distinguish two Codex CLIs
    // joined to the same room. A missing/mismatched session fails closed: bulk
    // imports and unverifiable callers never acquire local CLI provenance.
    let author_cli_session_id = if routing_candidate {
        match (req.messages[0].agent_type.as_ref(), req.session_id.as_ref()) {
            (Some(agent_type), Some(session_id)) => {
                let did = req.disc_id.clone();
                let agent = format!("{agent_type:?}");
                let session = session_id.clone();
                match state
                    .db
                    .with_read_conn(move |conn| {
                        Ok(crate::db::discussion_sessions::find_active_session(
                            conn, &agent, &session,
                        )?
                        .filter(|row| row.disc_id == did)
                        .map(|row| row.id))
                    })
                    .await
                {
                    Ok(session) => session,
                    Err(error) => {
                        return Json(ApiResponse::err(format!(
                            "DB error resolving CLI author: {error}"
                        )))
                    }
                }
            }
            _ => None,
        }
    } else {
        None
    };
    // An explicit typed target or legacy one-shot target always wins. With no
    // explicit responder, replying to a CLI-authored message targets that
    // exact session — never the provider's native agent or a sibling CLI.
    if routing_candidate && requested_targets.is_empty() && legacy_requested_target.is_none() {
        if let Some(reply_to_message_id) = req.messages[0].reply_to_message_id.clone() {
            let did = req.disc_id.clone();
            requested_targets = match state
                .db
                .with_read_conn(move |conn| {
                    Ok(crate::db::discussions::message_cli_author_target(
                        conn,
                        &did,
                        &reply_to_message_id,
                    )?
                    .into_iter()
                    .collect())
                })
                .await
            {
                Ok(targets) => targets,
                Err(error) => {
                    return Json(ApiResponse::err(format!(
                        "DB error resolving reply target: {error}"
                    )))
                }
            };
        }
    }
    // Resolve the whole presence snapshot in one DB turn, then feed the pure
    // shared routing policy. On lookup failure we fail closed as a no-agent
    // room: this live MCP caller is already a proven peer, so duplicate native
    // responders are worse than leaving the turn to peers that have the
    // message. This is deliberately asymmetric with the human HTTP path, where
    // no proven peer owner means falling back to the durable local runner.
    let (no_agent_room, native_principal_is_eligible, legacy_target_is_eligible) =
        if routing_candidate {
            let did = req.disc_id.clone();
            let native_agent = format!("{:?}", disc.agent);
            let legacy_target = legacy_requested_target
                .as_ref()
                .map(|target| format!("{target:?}"));
            state
                .db
                .with_conn(move |conn| {
                    let no_agent = crate::db::discussions::disc_is_no_agent(conn, &did)?;
                    let native_eligible =
                        crate::db::discussion_sessions::count_eligible_responders_for_agent(
                            conn,
                            &did,
                            &native_agent,
                        )? > 0;
                    let target_eligible = match legacy_target {
                        Some(target) => {
                            crate::db::discussion_sessions::count_eligible_responders_for_agent(
                                conn, &did, &target,
                            )? > 0
                        }
                        None => false,
                    };
                    Ok((no_agent, native_eligible, target_eligible))
                })
                .await
                .unwrap_or((true, false, true))
        } else {
            (true, false, false)
        };
    let dispatch_routes = if requested_targets.is_empty() {
        vec![route_joined_peer_turn(
            routing_candidate,
            no_agent_room,
            legacy_requested_target.as_ref(),
            legacy_target_is_eligible,
            native_principal_is_eligible,
        )]
    } else {
        requested_targets
            .iter()
            .map(|target| route_human_turn(no_agent_room, Some(target)))
            .collect::<Vec<_>>()
    };
    let mut dispatch_agents = Vec::<Option<AgentType>>::new();
    for route in &dispatch_routes {
        let agent = match route {
            DispatchRoute::NativePrincipal => Some(None),
            DispatchRoute::TargetedNative(agent) => Some(Some(agent.clone())),
            DispatchRoute::NoNativeResponder | DispatchRoute::JoinedPeers => None,
        };
        if let Some(agent) = agent {
            if !dispatch_agents.contains(&agent) {
                dispatch_agents.push(agent);
            }
        }
    }
    let mut live_lint_report: Option<crate::core::anti_halluc::LintReport> = None;
    let mut lint_summary: Option<AppendLintSummary> = None;
    if live_agent_append && crate::core::anti_halluc::current_mode().is_active() {
        if let Some(pid) = disc.project_id.clone() {
            let roots = state
                .db
                .with_conn(move |conn| {
                    let p = crate::db::projects::get_project(conn, &pid)?;
                    Ok(p.map(|p| {
                        let linked = p
                            .linked_repos
                            .iter()
                            .map(|lr| lr.location.clone())
                            .filter(|loc| {
                                !loc.starts_with("http://") && !loc.starts_with("https://")
                            })
                            .collect::<Vec<_>>();
                        (p.path, linked)
                    }))
                })
                .await
                .ok()
                .flatten();
            if let Some((project_path, linked)) = roots.filter(|(p, _)| !p.is_empty()) {
                live_lint_report = crate::core::anti_halluc::finalize_lint_report(
                    &req.messages[0].content,
                    None,
                    &project_path,
                    &linked,
                );
                // Echo a summary only when something actually FAILED — a
                // report with soft signals but zero failures would pair a
                // scary note with 0/0 counts (caught by live dogfooding).
                if let Some(ref r) = live_lint_report {
                    if r.fabricated_count > 0 || r.unsourced_count > 0 {
                        lint_summary = Some(AppendLintSummary {
                            fabricated_count: r.fabricated_count,
                            unsourced_count: r.unsourced_count,
                            note: "Some citations in your message could not be verified against the discussion's project tree — re-check the [src:] paths/lines and correct in your next message if needed.".into(),
                        });
                    }
                }
            }
        }
    }

    let mut appended = 0u32;
    let mut skipped = 0u32;
    let mut last_sort_order: Option<i64> = None;
    // Freshly-inserted messages, federated to peers after the loop IF this is a
    // single-message (live-turn) append on a shared disc — see the F3 gate below.
    let mut inserted_msgs: Vec<DiscussionMessage> = Vec::new();

    let did_for_loop = req.disc_id.clone();
    for incoming in req.messages.iter() {
        let did_check = did_for_loop.clone();
        let src_id_check = incoming.source_msg_id.clone();
        let already = state
            .db
            .with_conn(move |conn| {
                crate::db::disc_source::message_exists_for_source_id(
                    conn,
                    &did_check,
                    &src_id_check,
                )
            })
            .await
            .unwrap_or(false);
        if already {
            skipped += 1;
            continue;
        }

        let msg = DiscussionMessage {
            model: None,
            // Only a live single Agent append carries a report (loop runs once).
            lint_report: live_lint_report.take(),
            id: Uuid::new_v4().to_string(),
            role: incoming.role.clone(),
            content: incoming.content.clone(),
            agent_type: incoming.agent_type.clone(),
            timestamp: Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo: None,
            author_avatar_email: None,
            source_msg_id: Some(incoming.source_msg_id.clone()),
            // 0.8.5 — imported messages don't carry an authoritative
            // wall-clock duration (the source CLI may not have tracked
            // it). Always NULL on import; metrics aggregator excludes
            // NULLs from the AVG so this doesn't skew per-version data.
            duration_ms: None,
            // Compatibility projection for readers that predate
            // `message_targets`. The authoritative identity (including an
            // exact CLI session) is persisted below.
            target_agent: requested_targets
                .first()
                .map(|target| target.agent_type.clone())
                .or_else(|| legacy_requested_target.clone()),
            reply_to_message_id: incoming.reply_to_message_id.clone(),
        };
        let did_insert = did_for_loop.clone();
        let msg_clone = msg.clone();
        let typed_targets = requested_targets.clone();
        let dispatch_jobs = dispatch_agents
            .iter()
            .cloned()
            .map(|agent| (Uuid::new_v4().to_string(), agent))
            .collect::<Vec<_>>();
        let insert_result = state
            .db
            .with_conn(move |conn| {
                let dispatches = dispatch_jobs
                    .iter()
                    .map(|(job_id, agent)| crate::db::discussions::UserDispatchSpec {
                        job_id,
                        agent_override: agent.as_ref(),
                    })
                    .collect::<Vec<_>>();
                if let Some(author_cli_session_id) = author_cli_session_id {
                    crate::db::discussions::insert_cli_message_with_targets_and_dispatches(
                        conn,
                        &did_insert,
                        &msg_clone,
                        &typed_targets,
                        &dispatches,
                        author_cli_session_id,
                    )
                } else if !typed_targets.is_empty() || !dispatches.is_empty() {
                    crate::db::discussions::insert_message_with_targets_and_dispatches(
                        conn,
                        &did_insert,
                        &msg_clone,
                        &typed_targets,
                        &dispatches,
                    )
                } else {
                    crate::db::discussions::insert_message(conn, &did_insert, &msg_clone)
                }
            })
            .await;
        match insert_result {
            Ok(sort_order) => last_sort_order = Some(sort_order),
            Err(e) => {
                return Json(ApiResponse::err(format!(
                    "DB error appending message: {}",
                    e
                )))
            }
        }
        inserted_msgs.push(msg);
        appended += 1;
    }

    // Federate to peers via the shared helper (carries role + agent_type so an
    // agent reply lands as Agent, not User). F3: ONLY for a single-message
    // append — the live agent-turn case. Bulk transcript imports
    // (messages.len() > 1) are historical catch-up, not live chat: replaying N
    // frames would re-announce old turns AND can overflow the broadcast bus,
    // silently truncating the peer's copy.
    if req.messages.len() == 1 {
        if let Some(m) = inserted_msgs.first() {
            crate::api::federation::federate_message(&state, &req.disc_id, m).await;
        }
    }

    // Liveness heartbeat (migration 064). Posting is proof the agent is
    // alive — bump last_seen for each distinct agent_type that appended,
    // so `count_live_participants` (the double-responder guard) keeps
    // counting it as a live responder. Best-effort; a failure here must
    // not fail the append.
    if appended > 0 {
        let heartbeat_session_id = req.session_id.clone();
        let mut seen_agents = std::collections::HashSet::new();
        for incoming in req.messages.iter() {
            if let Some(at) = incoming.agent_type.clone() {
                let agent_type = format!("{at:?}");
                if seen_agents.insert(agent_type.clone()) {
                    let Some(session_id) = heartbeat_session_id.clone() else {
                        continue;
                    };
                    let did_touch = req.disc_id.clone();
                    if let Err(e) = state
                        .db
                        .with_conn(move |conn| {
                            crate::db::discussion_sessions::touch_session(
                                conn,
                                &did_touch,
                                &agent_type,
                                &session_id,
                            )?;
                            // 0.8.12 PR B — the agent just replied: the
                            // listening/reading placeholder vanishes the
                            // instant its message lands.
                            crate::db::discussion_sessions::clear_session_activity(
                                conn,
                                &did_touch,
                                &agent_type,
                                Some(&session_id),
                            )?;
                            // 0.9.2-G — a landed append IS proof the write path
                            // works: write-liveness becomes `ok`.
                            crate::db::discussion_sessions::mark_write_ok(
                                conn,
                                &did_touch,
                                &agent_type,
                                Some(&session_id),
                                chrono::Utc::now(),
                            )
                        })
                        .await
                    {
                        tracing::warn!(
                            "disc_append: failed to bump heartbeat / clear activity / write-state: {e}"
                        );
                    }
                }
            }
        }
    }

    // Room routing is intentionally asymmetric:
    // - a HUMAN message skips Kronn's native runner while joined MCP peers
    //   are live (messaging.rs), so the user does not get two answers;
    // - a joined PEER's reply wakes the native principal only when no live
    //   session of that principal's agent type already owns the room. A live
    //   Claude peer can wake an absent Codex principal, while a live Codex MCP
    //   session must not spawn a second Codex process for its own append.
    //
    // A session id distinguishes a live joined peer from historical imports.
    // Bulk appends, duplicates and no-agent rooms never trigger execution.
    let appended_live_turn =
        is_live_peer_turn(live_agent_append, req.session_id.as_deref(), appended);
    if appended_live_turn && !dispatch_agents.is_empty() {
        state.agent_dispatch_notify.notify_one();
        tracing::info!(
            "disc_append: live peer turn on {} queued {} durable native target(s)",
            req.disc_id,
            dispatch_agents.len()
        );
    }

    Json(ApiResponse::ok(DiscAppendResponse {
        appended,
        skipped_as_duplicates: skipped,
        last_sort_order,
        diverged,
        lint: lint_summary,
    }))
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscLinkRequest {
    pub disc_id: String,
    pub source_agent: String,
    pub source_session_id: String,
    /// Reassign a session that is already owned by another discussion.
    /// Defaults to false so the UI/MCP cannot steal a live thread silently.
    #[serde(default)]
    pub force_reassign: bool,
}

/// `POST /api/disc/link`
pub async fn disc_link(
    State(state): State<AppState>,
    Json(req): Json<DiscLinkRequest>,
) -> Json<ApiResponse<bool>> {
    let source_agent = req.source_agent.trim().to_string();
    let source_session_id = req.source_session_id.trim().to_string();
    if source_agent.is_empty() || source_session_id.is_empty() {
        return Json(ApiResponse::err(
            "source_agent and source_session_id are required",
        ));
    }
    if source_agent.chars().count() > 80 || source_session_id.chars().count() > 512 {
        return Json(ApiResponse::err(
            "source_agent or source_session_id exceeds the supported length",
        ));
    }

    let result = state
        .db
        .with_conn(move |conn| {
            let existing = crate::db::disc_source::find_disc_by_source_session(
                conn,
                &source_agent,
                &source_session_id,
            )?;
            if existing.as_deref().is_some_and(|id| id != req.disc_id)
                && !req.force_reassign
            {
                anyhow::bail!(
                    "session already linked to discussion {} — pass force_reassign=true to transfer it",
                    existing.expect("checked as Some")
                );
            }
            crate::db::disc_source::bind_to_source(
                conn,
                &req.disc_id,
                &source_agent,
                &source_session_id,
            )
        })
        .await;
    match result {
        Ok(_) => Json(ApiResponse::ok(true)),
        Err(e) => Json(ApiResponse::err(format!("DB error linking: {}", e))),
    }
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscSessionStatusQuery {
    pub source_agent: String,
    pub source_session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscSessionStatusResponse {
    pub binding_version: i64,
    pub bound_disc_id: Option<String>,
    /// Discussion currently carrying a non-left live peer with this identity.
    pub connected_disc_id: Option<String>,
    /// Native session state when connected (`active` / `paused`), otherwise null.
    pub connection_status: Option<String>,
}

/// `GET /api/disc/session-status?source_agent=…&source_session_id=…`
///
/// Source ownership and live peer presence are deliberately separate: an
/// imported Claude/Codex session may remain a valid resume key while its CLI is
/// currently offline.
pub async fn disc_session_status(
    State(state): State<AppState>,
    Query(query): Query<DiscSessionStatusQuery>,
) -> Json<ApiResponse<DiscSessionStatusResponse>> {
    let source_agent = query.source_agent.trim().to_string();
    let source_session_id = query.source_session_id.trim().to_string();
    if source_agent.is_empty() || source_session_id.is_empty() {
        return Json(ApiResponse::err(
            "source_agent and source_session_id are required",
        ));
    }
    let result = state
        .db
        .with_read_conn(move |conn| {
            let bound_disc_id = crate::db::disc_source::find_disc_by_source_session(
                conn,
                &source_agent,
                &source_session_id,
            )?;
            let connected = crate::db::discussion_sessions::find_active_session(
                conn,
                &source_agent,
                &source_session_id,
            )?;
            Ok::<_, anyhow::Error>(DiscSessionStatusResponse {
                binding_version: crate::db::disc_source::SOURCE_BINDING_VERSION,
                bound_disc_id,
                connected_disc_id: connected.as_ref().map(|session| session.disc_id.clone()),
                connection_status: connected.map(|session| session.status),
            })
        })
        .await;
    match result {
        Ok(status) => Json(ApiResponse::ok(status)),
        Err(error) => Json(ApiResponse::err(format!(
            "DB error resolving session status: {error}"
        ))),
    }
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscUnlinkRequest {
    pub disc_id: String,
    /// KT-85 — release only THIS session's binding. Omitting both fields
    /// releases every binding of the discussion, which a shared room must never
    /// do implicitly: it is reserved for an explicit human "detach this thread".
    #[serde(default)]
    pub source_agent: Option<String>,
    #[serde(default)]
    pub source_session_id: Option<String>,
}

/// `POST /api/disc/unlink`
pub async fn disc_unlink(
    State(state): State<AppState>,
    Json(req): Json<DiscUnlinkRequest>,
) -> Json<ApiResponse<bool>> {
    let result = state
        .db
        .with_conn(move |conn| {
            // Half a pair is a caller bug, not an instruction to detach the whole
            // room: falling through to the global unlink would evict every peer.
            let only = match (
                req.source_agent.as_deref(),
                req.source_session_id.as_deref(),
            ) {
                (Some(agent), Some(session)) => Some((agent, session)),
                (None, None) => None,
                _ => anyhow::bail!("source_agent and source_session_id must be provided together"),
            };
            crate::db::disc_source::unbind_from_source(conn, &req.disc_id, only)
        })
        .await;
    match result {
        Ok(closed) => Json(ApiResponse::ok(closed)),
        Err(e) => Json(ApiResponse::err(format!("DB error unlinking: {}", e))),
    }
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscFindBySessionQuery {
    pub source_agent: String,
    pub source_session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscFindBySessionResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disc_id: Option<String>,
}

/// `GET /api/disc/find_by_session?source_agent=…&source_session_id=…`
pub async fn disc_find_by_session(
    State(state): State<AppState>,
    Query(q): Query<DiscFindBySessionQuery>,
) -> Json<ApiResponse<DiscFindBySessionResponse>> {
    let result = state
        .db
        .with_conn(move |conn| {
            crate::db::disc_source::find_disc_by_source_session(
                conn,
                &q.source_agent,
                &q.source_session_id,
            )
        })
        .await;
    match result {
        Ok(disc_id) => Json(ApiResponse::ok(DiscFindBySessionResponse { disc_id })),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscSearchQuery {
    pub q: String,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `GET /api/disc/search?q=…&limit=…`
pub async fn disc_search(
    State(state): State<AppState>,
    Query(q): Query<DiscSearchQuery>,
) -> Json<ApiResponse<Vec<crate::db::disc_source::DiscSearchHit>>> {
    if q.q.trim().is_empty() {
        return Json(ApiResponse::err("query string `q` must not be empty"));
    }
    let limit = q.limit.unwrap_or(20);
    let result = state
        .db
        .with_conn(move |conn| crate::db::disc_source::search_discussions(conn, &q.q, limit))
        .await;
    match result {
        Ok(hits) => Json(ApiResponse::ok(hits)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// KT-65 — filters for the message-level search. Every field is optional and
/// they combine with AND, so the caller narrows instead of paging blindly.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct MessageSearchQuery {
    pub q: String,
    #[serde(default)]
    pub discussion_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    /// Agent type ("Codex") or federated human pseudo ("Romu - mac").
    #[serde(default)]
    pub author: Option<String>,
    /// Inclusive RFC3339 bounds.
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
}

/// `GET /api/disc/search/messages?q=…&discussion_id=…&author=…&since=…`
///
/// Unlike `/api/disc/search` (which answers "which rooms mention X"), this
/// returns the matching MESSAGES so the UI can open the exact turn. The query is
/// bounded server-side — limit clamped, offset capped — so a large history can't
/// turn one keystroke into a full scan streamed to the client.
pub async fn message_search(
    State(state): State<AppState>,
    Query(q): Query<MessageSearchQuery>,
) -> Json<ApiResponse<Vec<crate::db::disc_source::MessageSearchHit>>> {
    if q.q.trim().is_empty() {
        return Json(ApiResponse::err("query string `q` must not be empty"));
    }
    let limit = q.limit.unwrap_or(20);
    let offset = q.offset.unwrap_or(0);
    let result = state
        .db
        .with_conn(move |conn| {
            let filters = crate::db::disc_source::MessageSearchFilters {
                discussion_id: q.discussion_id.as_deref(),
                project_id: q.project_id.as_deref(),
                author: q.author.as_deref(),
                since: q.since.as_deref(),
                until: q.until.as_deref(),
            };
            crate::db::disc_source::search_messages(conn, &q.q, &filters, limit, offset)
        })
        .await;
    match result {
        Ok(hits) => Json(ApiResponse::ok(hits)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscLoadOtherQuery {
    pub disc_id: String,
    #[serde(default)]
    pub from: Option<u32>,
    #[serde(default)]
    pub to: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscLoadOtherMessage {
    pub idx: u32,
    pub role: MessageRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<AgentType>,
    pub timestamp: String,
    /// Files attached to this message (0.8.8). Mirrors `disc_get_message` so a
    /// cross-disc reader can discover an image's `disk_path` and open it with
    /// its file tools — without this, an agent browsing ANOTHER disc only sees
    /// the text and is blind to the attached images. Empty for most messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<crate::api::disc_introspection::MessageAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscLoadOtherResponse {
    pub disc_id: String,
    pub title: String,
    pub total_messages: u32,
    pub from_idx: u32,
    pub to_idx: u32,
    pub messages: Vec<DiscLoadOtherMessage>,
}

/// `GET /api/disc/sources`
///
/// 0.8.4 (#294) — batch endpoint that returns every currently-bound
/// disc with its source binding. The frontend sidebar calls this
/// once per mount to decorate disc rows with a "bound to X" badge +
/// drive the source-filter dropdown. Returns `[]` when no disc has a
/// binding (the common case on fresh installs).
///
/// A binding is NOT an import (KT-74): a portable bundle's provenance lives in
/// `discussion_imports` and is served by `GET /api/disc/imports`.
pub async fn list_source_bindings(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<crate::db::disc_source::DiscSourceBinding>>> {
    let result = state
        .db
        .with_conn(crate::db::disc_source::list_all_source_bindings)
        .await;
    match result {
        Ok(bindings) => Json(ApiResponse::ok(bindings)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscSourceDetail {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<crate::db::disc_source::DiscSourceBinding>,
    pub history: Vec<crate::db::disc_source::DiscSourceHistoryEntry>,
}

/// `GET /api/discussions/{id}/source`
///
/// Returns the current binding (if any) + the full append-only
/// history chain for tooltip rendering ("first owned by ClaudeCode
/// sess A, then Cursor sess B"). Empty `history: []` for discs that
/// have never been imported.
pub async fn disc_source_detail(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<ApiResponse<DiscSourceDetail>> {
    // KT-85 — read the discussion's CURRENT binding. Scanning the full list and
    // taking the first hit returned the OLDEST binding once a room could hold
    // several, so the panel named a session the user had long since replaced.
    let id_for_bindings = id.clone();
    let current = state
        .db
        .with_conn(move |conn| {
            crate::db::disc_source::current_source_binding(conn, &id_for_bindings)
        })
        .await
        .unwrap_or_default();

    let id_for_hist = id.clone();
    let history = state
        .db
        .with_conn(move |conn| crate::db::disc_source::list_source_history(conn, &id_for_hist))
        .await
        .unwrap_or_default();
    Json(ApiResponse::ok(DiscSourceDetail { current, history }))
}

/// `GET /api/disc/load_other?disc_id=…&from=…&to=…`
///
/// Defaults: `from=0`, `to=total` (full disc). Clamped to the actual
/// length so a curious caller can't OOM us with a huge range.
pub async fn disc_load_other(
    State(state): State<AppState>,
    Query(q): Query<DiscLoadOtherQuery>,
) -> Json<ApiResponse<DiscLoadOtherResponse>> {
    let did = q.disc_id.clone();
    let result = state
        .db
        .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &did))
        .await;
    let disc = match result {
        Ok(Some(d)) => d,
        Ok(None) => return Json(ApiResponse::err("Discussion not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let non_system: Vec<&DiscussionMessage> = disc
        .messages
        .iter()
        .filter(|m| !matches!(m.role, MessageRole::System))
        .collect();
    let total = non_system.len() as u32;
    let from = q.from.unwrap_or(0).min(total);
    let to = q.to.unwrap_or(total).min(total);
    let (from, to) = if from > to { (to, from) } else { (from, to) };

    // Group the disc's attachments by message id in one read, so each returned
    // message can carry the files pinned to it (0.8.8). list_context_files is
    // a single indexed query — cheaper than one query per message.
    let did_files = q.disc_id.clone();
    let files = state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::list_context_files(conn, &did_files)
                .map_err(|e| anyhow::anyhow!(e))
        })
        .await
        .unwrap_or_default();
    let mut by_msg: std::collections::HashMap<
        String,
        Vec<crate::api::disc_introspection::MessageAttachment>,
    > = std::collections::HashMap::new();
    for f in files {
        if let Some(mid) = f.message_id.clone() {
            by_msg.entry(mid).or_default().push(
                crate::api::disc_introspection::MessageAttachment {
                    id: f.id,
                    filename: f.filename,
                    mime_type: f.mime_type,
                    disk_path: f.disk_path,
                },
            );
        }
    }

    let msgs = non_system[(from as usize)..(to as usize)]
        .iter()
        .enumerate()
        .map(|(rel, m)| DiscLoadOtherMessage {
            idx: from + rel as u32,
            role: m.role.clone(),
            content: m.content.clone(),
            agent_type: m.agent_type.clone(),
            timestamp: m.timestamp.to_rfc3339(),
            attachments: by_msg.get(&m.id).cloned().unwrap_or_default(),
        })
        .collect();

    Json(ApiResponse::ok(DiscLoadOtherResponse {
        disc_id: q.disc_id,
        title: disc.title,
        total_messages: total,
        from_idx: from,
        to_idx: to,
        messages: msgs,
    }))
}

#[cfg(test)]
mod tests {
    //! Route-level unit tests live in `backend/tests/api_tests.rs` —
    //! the in-memory DB integration there is what exercises the full
    //! HTTP→DB→response loop. This block only pins shape-level
    //! invariants (no I/O).

    use super::*;
    use serial_test::serial;

    /// In-memory state with a project rooted at a real tempdir + one disc.
    async fn lint_state(bind_project: bool) -> (crate::AppState, tempfile::TempDir) {
        use std::sync::Arc;
        use tokio::sync::RwLock;
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/real.rs"), "fn real() {}\n").unwrap();
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let path = tmp.path().to_string_lossy().to_string();
        let bind = bind_project;
        db.with_conn(move |conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p-lint', 'LintProj', ?1, ?2, ?2)",
                rusqlite::params![path, now],
            )?;
            conn.execute(
                "INSERT INTO discussions (id, project_id, title, agent, language, participants_json,
                 created_at, updated_at, message_count, workspace_mode, no_agent)
                 VALUES ('d-lint', ?1, 'T', 'ClaudeCode', 'fr', '[]', datetime('now'), datetime('now'), 0, 'Direct', 1)",
                rusqlite::params![if bind { Some("p-lint") } else { None }],
            )?;
            Ok(())
        }).await.unwrap();
        let cfg = Arc::new(RwLock::new(crate::core::config::default_config()));
        (
            crate::AppState::new_defaults(cfg, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS),
            tmp,
        )
    }

    fn agent_msg(id: &str, content: &str) -> DiscAppendMessage {
        DiscAppendMessage {
            source_msg_id: id.into(),
            role: MessageRole::Agent,
            content: content.into(),
            agent_type: Some(AgentType::Codex),
            targets: Vec::new(),
            target_agent: None,
            reply_to_message_id: None,
        }
    }

    async fn append_as(
        state: &crate::AppState,
        msgs: Vec<DiscAppendMessage>,
        session_id: Option<&str>,
    ) -> DiscAppendResponse {
        let resp = disc_append(
            axum::extract::State(state.clone()),
            Json(DiscAppendRequest {
                disc_id: "d-lint".into(),
                messages: msgs,
                session_id: session_id.map(str::to_owned),
            }),
        )
        .await;
        resp.0.data.expect("append succeeds")
    }

    async fn append(state: &crate::AppState, msgs: Vec<DiscAppendMessage>) -> DiscAppendResponse {
        append_as(state, msgs, None).await
    }

    #[tokio::test]
    async fn append_persists_an_opaque_same_discussion_reply_target() {
        let (state, _tmp) = lint_state(false).await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO messages
                     (id, discussion_id, role, content, timestamp, sort_order)
                     VALUES ('wsl-original', 'd-lint', 'User', 'Question', datetime('now'), 1)",
                    [],
                )?;
                conn.execute(
                    "UPDATE discussions
                     SET next_message_seq = 2, message_count = 1
                     WHERE id = 'd-lint'",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let mut reply = agent_msg("reply-source", "Precise answer");
        reply.reply_to_message_id = Some("wsl-original".into());
        let response = append(&state, vec![reply]).await;
        assert_eq!(response.appended, 1);

        let messages = state
            .db
            .with_conn(|conn| crate::db::discussions::list_messages(conn, "d-lint"))
            .await
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[1].reply_to_message_id.as_deref(),
            Some("wsl-original")
        );
    }

    #[tokio::test]
    async fn append_rejects_a_reply_target_outside_the_discussion() {
        let (state, _tmp) = lint_state(false).await;
        let response = disc_append(
            axum::extract::State(state.clone()),
            Json(DiscAppendRequest {
                disc_id: "d-lint".into(),
                messages: vec![DiscAppendMessage {
                    reply_to_message_id: Some("missing-message".into()),
                    ..agent_msg("reply-source", "Precise answer")
                }],
                session_id: None,
            }),
        )
        .await;
        let error = response.0.error.expect("invalid target rejected");
        assert!(error.contains("not found in this discussion"));

        let messages = state
            .db
            .with_conn(|conn| crate::db::discussions::list_messages(conn, "d-lint"))
            .await
            .unwrap();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    #[serial] // global anti-halluc mode cell
    async fn append_clears_the_activity_placeholder() {
        // 0.8.12 PR B (Copilot review): the REAL disc_append path must
        // clear only the posting session's placeholder and heartbeat. A
        // sibling Codex process in the same room must remain untouched.
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::discussion_sessions::join_disc_session(conn, "d-lint", "Codex", "s-x")
                    .map(|_| ())?;
                crate::db::discussion_sessions::join_disc_session(conn, "d-lint", "Codex", "s-y")
                    .map(|_| ())?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("s-x"),
                    "reading",
                    300,
                )?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("s-y"),
                    "reading",
                    300,
                )?;
                conn.execute(
                    "UPDATE discussion_sessions SET last_seen = '2000-01-01T00:00:00Z'\
                     WHERE disc_id = 'd-lint' AND agent_type = 'Codex'",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        append_as(
            &state,
            vec![agent_msg("s-act", "voilà ma réponse")],
            Some("s-x"),
        )
        .await;

        let sessions = state
            .db
            .with_conn(|conn| crate::db::discussion_sessions::list_sessions(conn, "d-lint", false))
            .await
            .unwrap();
        let poster = sessions
            .iter()
            .find(|s| s.session_id.as_deref() == Some("s-x"))
            .unwrap();
        let sibling = sessions
            .iter()
            .find(|s| s.session_id.as_deref() == Some("s-y"))
            .unwrap();
        assert!(
            poster.activity.is_none(),
            "disc_append must clear the poster's activity"
        );
        assert_ne!(poster.last_seen.as_deref(), Some("2000-01-01T00:00:00Z"));
        assert_eq!(sibling.activity.as_deref(), Some("reading"));
        assert_eq!(sibling.last_seen.as_deref(), Some("2000-01-01T00:00:00Z"));
        assert!(
            state.cancel_registry.lock().unwrap().is_empty(),
            "a no-agent room persists the peer message without starting a native runner"
        );
    }

    #[tokio::test]
    #[serial]
    async fn live_peer_append_commits_a_durable_native_dispatch() {
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE discussions SET no_agent = 0 WHERE id = 'd-lint'",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        append_as(
            &state,
            vec![agent_msg("s-durable", "réponse du pair")],
            Some("joined-session"),
        )
        .await;

        let (job, awaiting) = state
            .db
            .with_conn(|conn| {
                let job = crate::db::agent_dispatch::find_active_for_discussion(conn, "d-lint")?;
                let awaiting = conn.query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = 'd-lint'",
                    [],
                    |row| row.get::<_, bool>(0),
                )?;
                Ok((job, awaiting))
            })
            .await
            .unwrap();
        let job = job.expect("peer turn must own a durable native response");
        assert_eq!(
            job.status,
            crate::db::agent_dispatch::DispatchStatus::Pending
        );
        assert!(awaiting);
    }

    #[tokio::test]
    #[serial]
    async fn live_native_agent_session_prevents_a_duplicate_dispatch() {
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE discussions
                     SET no_agent = 0, agent = 'Codex'
                     WHERE id = 'd-lint'",
                    [],
                )?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("joined-session"),
                    "peer",
                )?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("joined-session"),
                    "reading",
                    300,
                )?;
                Ok(())
            })
            .await
            .unwrap();

        append_as(
            &state,
            vec![agent_msg(
                "same-agent-turn",
                "réponse du Codex déjà connecté",
            )],
            Some("joined-session"),
        )
        .await;

        let (job, awaiting) = state
            .db
            .with_conn(|conn| {
                let job = crate::db::agent_dispatch::find_active_for_discussion(conn, "d-lint")?;
                let awaiting = conn.query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = 'd-lint'",
                    [],
                    |row| row.get::<_, bool>(0),
                )?;
                Ok((job, awaiting))
            })
            .await
            .unwrap();
        assert!(
            job.is_none(),
            "a live session of the native agent type already owns the reply"
        );
        assert!(!awaiting);
    }

    #[tokio::test]
    #[serial]
    async fn live_peer_mention_dispatches_targeted_agent_bypassing_native_gate() {
        // 0.9.2-G: a joined Codex peer mentions @ollama. Even though the native
        // principal (Codex) is live — which suppresses the NATIVE dispatch — the
        // structured target must still queue a durable one-shot for Ollama, or
        // the mentioned agent never answers (the peer→peer targeting gap).
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE discussions
                     SET no_agent = 0, agent = 'Codex'
                     WHERE id = 'd-lint'",
                    [],
                )?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("joined-session"),
                    "peer",
                )?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("joined-session"),
                    "reading",
                    300,
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let mut msg = agent_msg("mention-ollama", "@ollama peux-tu confirmer ?");
        msg.target_agent = Some(AgentType::Ollama);
        append_as(&state, vec![msg], Some("joined-session")).await;

        let (job, awaiting) = state
            .db
            .with_conn(|conn| {
                let job = crate::db::agent_dispatch::find_active_for_discussion(conn, "d-lint")?;
                let awaiting = conn.query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = 'd-lint'",
                    [],
                    |row| row.get::<_, bool>(0),
                )?;
                Ok((job, awaiting))
            })
            .await
            .unwrap();
        let job = job.expect("a structured mention must own a durable targeted response");
        assert_eq!(
            job.status,
            crate::db::agent_dispatch::DispatchStatus::Pending
        );
        assert_eq!(
            job.agent_override,
            Some(AgentType::Ollama),
            "the durable job must target the mentioned agent, not the native principal"
        );
        assert!(awaiting);
    }

    #[tokio::test]
    #[serial]
    async fn typed_cli_target_is_durable_and_never_spawns_a_native_agent() {
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        let cli_session_id = state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE discussions
                     SET no_agent = 0, agent = 'ClaudeCode'
                     WHERE id = 'd-lint'",
                    [],
                )?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Vibe",
                    Some("vibe-author"),
                    "peer",
                )?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("codex-exact"),
                    "peer",
                )
            })
            .await
            .unwrap();

        let mut msg = agent_msg("typed-exact-cli", "@codex-cli confirme");
        msg.agent_type = Some(AgentType::Vibe);
        msg.targets = vec![MessageTarget::cli(AgentType::Codex, cli_session_id)];
        append_as(&state, vec![msg], Some("vibe-author")).await;

        let (job, targets) = state
            .db
            .with_conn(move |conn| {
                let job = crate::db::agent_dispatch::find_active_for_discussion(conn, "d-lint")?;
                let message_id = conn.query_row(
                    "SELECT id FROM messages WHERE source_msg_id = 'typed-exact-cli'",
                    [],
                    |row| row.get::<_, String>(0),
                )?;
                let targets = crate::db::discussions::list_message_targets(conn, &message_id)?;
                Ok((job, targets))
            })
            .await
            .unwrap();
        assert!(job.is_none(), "an exact CLI owns its turn through polling");
        assert_eq!(
            targets,
            vec![MessageTarget::cli(AgentType::Codex, cli_session_id)]
        );
    }

    #[tokio::test]
    #[serial]
    async fn reply_to_routes_to_the_exact_cli_author_across_session_resume() {
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        let (author_session, author_resume, responder_session) = state
            .db
            .with_conn(|conn| {
                let author = crate::db::discussion_sessions::join_disc_session_resumable(
                    conn,
                    "d-lint",
                    "Codex",
                    "codex-author-old",
                )?;
                let responder = crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Vibe",
                    Some("vibe-responder"),
                    "peer",
                )?;
                Ok((author.session_pk, author.resume_token, responder))
            })
            .await
            .unwrap();

        append_as(
            &state,
            vec![agent_msg("exact-author", "message from exact Codex CLI")],
            Some("codex-author-old"),
        )
        .await;

        let original_message_id =
            state
                .db
                .with_conn(move |conn| {
                    let message_id = conn.query_row(
                        "SELECT id FROM messages WHERE source_msg_id = 'exact-author'",
                        [],
                        |row| row.get::<_, String>(0),
                    )?;
                    assert_eq!(
                        crate::db::discussions::message_cli_author_target(
                            conn,
                            "d-lint",
                            &message_id,
                        )?,
                        Some(MessageTarget::cli(AgentType::Codex, author_session))
                    );
                    let resumed = crate::db::discussion_sessions::resume_disc_session(
                        conn,
                        "Codex",
                        &author_resume,
                        "codex-author-reloaded",
                        None,
                    )?;
                    assert_eq!(
                        resumed.session_pk, author_session,
                        "a bridge reload must preserve the durable exact author identity"
                    );
                    Ok(message_id)
                })
                .await
                .unwrap();

        let mut reply = agent_msg("exact-reply", "reply from Vibe");
        reply.agent_type = Some(AgentType::Vibe);
        reply.reply_to_message_id = Some(original_message_id);
        append_as(&state, vec![reply], Some("vibe-responder")).await;

        let (targets, reply_author, job) = state
            .db
            .with_conn(move |conn| {
                let reply_message_id = conn.query_row(
                    "SELECT id FROM messages WHERE source_msg_id = 'exact-reply'",
                    [],
                    |row| row.get::<_, String>(0),
                )?;
                Ok((
                    crate::db::discussions::list_message_targets(conn, &reply_message_id)?,
                    crate::db::discussions::message_cli_author_target(
                        conn,
                        "d-lint",
                        &reply_message_id,
                    )?,
                    crate::db::agent_dispatch::find_active_for_discussion(conn, "d-lint")?,
                ))
            })
            .await
            .unwrap();

        assert_eq!(
            targets,
            vec![MessageTarget::cli(AgentType::Codex, author_session)]
        );
        assert_eq!(
            reply_author,
            Some(MessageTarget::cli(AgentType::Vibe, responder_session))
        );
        assert!(
            job.is_none(),
            "replying to a CLI must never wake a same-provider native agent"
        );
    }

    #[tokio::test]
    #[serial]
    async fn explicit_target_overrides_the_reply_to_cli_author() {
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        let explicit_session = state
            .db
            .with_conn(|conn| {
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("codex-original"),
                    "peer",
                )?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Vibe",
                    Some("vibe-responder"),
                    "peer",
                )?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("codex-explicit"),
                    "peer",
                )
            })
            .await
            .unwrap();

        append_as(
            &state,
            vec![agent_msg("override-author", "original Codex CLI")],
            Some("codex-original"),
        )
        .await;
        let original_message_id = state
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT id FROM messages WHERE source_msg_id = 'override-author'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .map_err(anyhow::Error::from)
            })
            .await
            .unwrap();

        let mut reply = agent_msg("override-reply", "explicitly for another CLI");
        reply.agent_type = Some(AgentType::Vibe);
        reply.reply_to_message_id = Some(original_message_id);
        reply.targets = vec![MessageTarget::cli(AgentType::Codex, explicit_session)];
        append_as(&state, vec![reply], Some("vibe-responder")).await;

        let targets = state
            .db
            .with_conn(move |conn| {
                let reply_message_id = conn.query_row(
                    "SELECT id FROM messages WHERE source_msg_id = 'override-reply'",
                    [],
                    |row| row.get::<_, String>(0),
                )?;
                crate::db::discussions::list_message_targets(conn, &reply_message_id)
            })
            .await
            .unwrap();
        assert_eq!(
            targets,
            vec![MessageTarget::cli(AgentType::Codex, explicit_session)]
        );
    }

    #[tokio::test]
    #[serial]
    async fn reply_to_a_native_message_keeps_legacy_untargeted_routing() {
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Vibe",
                    Some("vibe-native-reply"),
                    "peer",
                )?;
                conn.execute(
                    "INSERT INTO messages (
                         id, discussion_id, role, content, agent_type,
                         timestamp, sort_order
                     ) VALUES (
                         'native-message', 'd-lint', 'Agent', 'native answer',
                         'Codex', datetime('now'), 1
                     )",
                    [],
                )?;
                conn.execute(
                    "UPDATE discussions
                     SET next_message_seq = 2, message_count = 1
                     WHERE id = 'd-lint'",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let mut reply = agent_msg("native-message-reply", "Vibe follows up");
        reply.agent_type = Some(AgentType::Vibe);
        reply.reply_to_message_id = Some("native-message".into());
        append_as(&state, vec![reply], Some("vibe-native-reply")).await;

        let targets = state
            .db
            .with_conn(|conn| {
                let reply_message_id = conn.query_row(
                    "SELECT id FROM messages
                     WHERE source_msg_id = 'native-message-reply'",
                    [],
                    |row| row.get::<_, String>(0),
                )?;
                crate::db::discussions::list_message_targets(conn, &reply_message_id)
            })
            .await
            .unwrap();
        assert!(
            targets.is_empty(),
            "a native author has no exact local CLI identity to infer"
        );
    }

    #[tokio::test]
    #[serial]
    async fn bulk_import_reply_to_never_infers_a_local_cli_target() {
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        state
            .db
            .with_conn(|conn| {
                let cli_session_id = crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("codex-local-author"),
                    "peer",
                )?;
                conn.execute(
                    "INSERT INTO messages (
                         id, discussion_id, role, content, agent_type,
                         timestamp, sort_order
                     ) VALUES (
                         'local-cli-message', 'd-lint', 'Agent', 'local answer',
                         'Codex', datetime('now'), 1
                     )",
                    [],
                )?;
                crate::db::discussions::set_message_cli_author(
                    conn,
                    "local-cli-message",
                    cli_session_id,
                )?;
                conn.execute(
                    "UPDATE discussions
                     SET next_message_seq = 2, message_count = 1
                     WHERE id = 'd-lint'",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let mut first_imported = agent_msg("bulk-reply", "historical reply");
        first_imported.reply_to_message_id = Some("local-cli-message".into());
        append(
            &state,
            vec![
                first_imported,
                agent_msg("bulk-follow-up", "historical follow-up"),
            ],
        )
        .await;

        let imported = state
            .db
            .with_conn(|conn| {
                let mut statement = conn.prepare(
                    "SELECT id, target_agent
                     FROM messages
                     WHERE source_msg_id IN ('bulk-reply', 'bulk-follow-up')
                     ORDER BY sort_order",
                )?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let rows = rows
                    .into_iter()
                    .map(|(message_id, target_agent)| {
                        let targets =
                            crate::db::discussions::list_message_targets(conn, &message_id)?;
                        Ok((target_agent, targets))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await
            .unwrap();

        assert_eq!(imported.len(), 2);
        assert!(imported
            .iter()
            .all(|(target_agent, targets)| target_agent.is_none() && targets.is_empty()));
    }

    #[tokio::test]
    #[serial]
    async fn typed_native_target_never_wakes_a_same_provider_cli_by_coincidence() {
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE discussions
                     SET no_agent = 0, agent = 'ClaudeCode'
                     WHERE id = 'd-lint'",
                    [],
                )?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Vibe",
                    Some("vibe-author"),
                    "peer",
                )?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("codex-cli"),
                    "peer",
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let mut msg = agent_msg("typed-native-codex", "@codex confirme");
        msg.agent_type = Some(AgentType::Vibe);
        msg.targets = vec![MessageTarget::agent(AgentType::Codex)];
        append_as(&state, vec![msg], Some("vibe-author")).await;

        let job = state
            .db
            .with_conn(|conn| crate::db::agent_dispatch::find_active_for_discussion(conn, "d-lint"))
            .await
            .unwrap()
            .expect("the typed native identity must own a durable job");
        assert_eq!(job.agent_override, Some(AgentType::Codex));
    }

    #[tokio::test]
    #[serial]
    async fn live_peer_mention_does_not_spawn_duplicate_native_target() {
        // Regression (live room 0.9.2): Vibe mentioned @codex while a joined
        // Codex peer was actively watching. The targeted path bypassed every
        // liveness gate and spawned a second native Codex, producing two near-
        // identical replies. A live target owns the mention; no job is needed.
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE discussions
                     SET no_agent = 0, agent = 'ClaudeCode'
                     WHERE id = 'd-lint'",
                    [],
                )?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Vibe",
                    Some("vibe-session"),
                    "peer",
                )?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    "d-lint",
                    "Vibe",
                    Some("vibe-session"),
                    "reading",
                    300,
                )?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("codex-session"),
                    "peer",
                )?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("codex-session"),
                    "listening",
                    300,
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let mut msg = agent_msg("vibe-mentions-live-codex", "@codex peux-tu confirmer ?");
        msg.agent_type = Some(AgentType::Vibe);
        msg.target_agent = Some(AgentType::Codex);
        append_as(&state, vec![msg], Some("vibe-session")).await;

        let (job, awaiting) = state
            .db
            .with_conn(|conn| {
                let job = crate::db::agent_dispatch::find_active_for_discussion(conn, "d-lint")?;
                let awaiting = conn.query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = 'd-lint'",
                    [],
                    |row| row.get::<_, bool>(0),
                )?;
                Ok((job, awaiting))
            })
            .await
            .unwrap();
        assert!(
            job.is_none(),
            "the joined Codex peer owns @codex; a native Codex would duplicate it"
        );
        assert!(
            !awaiting,
            "no native response is owed while the peer is live"
        );

        // KT-58 — no dispatch job, but the addressee must still be recorded:
        // this is the common case in a populated room, and it is exactly when a
        // reader needs to see WHO is expected to answer.
        let stored = state
            .db
            .with_conn(|conn| crate::db::discussions::list_messages(conn, "d-lint"))
            .await
            .unwrap();
        let mention = stored
            .iter()
            .find(|m| m.source_msg_id.as_deref() == Some("vibe-mentions-live-codex"))
            .expect("the mention must be persisted");
        assert_eq!(
            mention.target_agent,
            Some(AgentType::Codex),
            "the addressee must survive even when a live peer owns the reply"
        );
    }

    #[tokio::test]
    #[serial]
    async fn expired_target_peer_falls_back_to_a_durable_targeted_dispatch() {
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE discussions
                     SET no_agent = 0, agent = 'ClaudeCode'
                     WHERE id = 'd-lint'",
                    [],
                )?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Vibe",
                    Some("vibe-session"),
                    "peer",
                )?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    "d-lint",
                    "Vibe",
                    Some("vibe-session"),
                    "reading",
                    300,
                )?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("stale-codex-session"),
                    "peer",
                )?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("stale-codex-session"),
                    "listening",
                    -100,
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let mut msg = agent_msg("vibe-mentions-stale-codex", "@codex peux-tu confirmer ?");
        msg.agent_type = Some(AgentType::Vibe);
        msg.target_agent = Some(AgentType::Codex);
        append_as(&state, vec![msg], Some("vibe-session")).await;

        let (job, sticky_target_count) = state
            .db
            .with_conn(|conn| {
                let job = crate::db::agent_dispatch::find_active_for_discussion(conn, "d-lint")?;
                let sticky_target_count =
                    crate::db::discussion_sessions::count_live_participants_for_agent(
                        conn, "d-lint", "Codex",
                    )?;
                Ok((job, sticky_target_count))
            })
            .await
            .unwrap();
        let job = job.expect("an expired target peer must not swallow the turn");
        assert_eq!(job.agent_override, Some(AgentType::Codex));
        assert_eq!(
            sticky_target_count, 1,
            "routing freshness must not detach the stale participant"
        );
    }

    #[tokio::test]
    #[serial]
    async fn no_agent_room_never_spawns_an_absent_mentioned_agent() {
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE discussions
                     SET no_agent = 1, agent = 'ClaudeCode'
                     WHERE id = 'd-lint'",
                    [],
                )?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Vibe",
                    Some("vibe-session"),
                    "peer",
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let mut msg = agent_msg("vibe-mentions-absent-codex", "@codex peux-tu confirmer ?");
        msg.agent_type = Some(AgentType::Vibe);
        msg.target_agent = Some(AgentType::Codex);
        append_as(&state, vec![msg], Some("vibe-session")).await;

        let (job, awaiting, stored_target) = state
            .db
            .with_conn(|conn| {
                let job = crate::db::agent_dispatch::find_active_for_discussion(conn, "d-lint")?;
                let awaiting = conn.query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = 'd-lint'",
                    [],
                    |row| row.get::<_, bool>(0),
                )?;
                let stored_target = crate::db::discussions::list_messages(conn, "d-lint")?
                    .into_iter()
                    .find(|message| {
                        message.source_msg_id.as_deref() == Some("vibe-mentions-absent-codex")
                    })
                    .and_then(|message| message.target_agent);
                Ok((job, awaiting, stored_target))
            })
            .await
            .unwrap();
        assert!(job.is_none(), "no-agent rooms never start native runners");
        assert!(!awaiting);
        assert_eq!(
            stored_target,
            Some(AgentType::Codex),
            "the intended addressee remains visible even without a dispatch"
        );
    }

    #[tokio::test]
    #[serial]
    async fn self_mention_does_not_dispatch() {
        // Anti-loop: an agent mentioning its own type is a no-op for targeting.
        // With the native principal (Codex) live, neither the targeted nor the
        // native path fires — so a self-mention never spawns a responder.
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE discussions
                     SET no_agent = 0, agent = 'Codex'
                     WHERE id = 'd-lint'",
                    [],
                )?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("joined-session"),
                    "peer",
                )?;
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    "d-lint",
                    "Codex",
                    Some("joined-session"),
                    "reading",
                    300,
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let mut msg = agent_msg("self-mention", "@codex je me réponds ?");
        msg.target_agent = Some(AgentType::Codex);
        append_as(&state, vec![msg], Some("joined-session")).await;

        let job = state
            .db
            .with_conn(|conn| crate::db::agent_dispatch::find_active_for_discussion(conn, "d-lint"))
            .await
            .unwrap();
        assert!(
            job.is_none(),
            "a self-mention must not create a dispatch, and the live native session suppresses the native path"
        );
    }

    #[tokio::test]
    #[serial] // global anti-halluc mode cell
    async fn append_returns_the_real_sort_order_of_the_last_message() {
        // stab-1 — agents estimated their position (+1 per post) because the
        // response carried no sort_order; concurrent posters made the
        // estimate drift and long-polls silently skipped messages.
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;

        let first = append(&state, vec![agent_msg("s1", "un")]).await;
        let a = first.last_sort_order.expect("appended → position present");

        let second = append(
            &state,
            vec![agent_msg("s2", "deux"), agent_msg("s3", "trois")],
        )
        .await;
        let b = second
            .last_sort_order
            .expect("batch → position of the LAST message");
        assert_eq!(b, a + 2, "two more rows after the first");

        // Pure duplicate: nothing appended → no position (the caller keeps
        // its previous marker).
        let dup = append(&state, vec![agent_msg("s3", "trois")]).await;
        assert_eq!(dup.skipped_as_duplicates, 1);
        assert!(dup.last_sort_order.is_none());
    }

    #[tokio::test]
    #[serial] // global anti-halluc mode cell
    async fn live_agent_append_with_fabricated_source_carries_lint() {
        crate::core::anti_halluc::set_mode("warn");
        let (state, _tmp) = lint_state(true).await;
        let out = append(
            &state,
            vec![agent_msg(
                "m1",
                "Confirmed the bug. [src: file: src/does-not-exist.rs:42]",
            )],
        )
        .await;
        assert_eq!(out.appended, 1, "insert is NEVER blocked");
        let lint = out
            .lint
            .expect("fabricated citation must produce a summary");
        assert!(lint.fabricated_count >= 1, "{lint:?}");
        // The stored message carries the full report (UI badge).
        let msg = state
            .db
            .with_conn(|conn| crate::db::discussions::list_messages(conn, "d-lint"))
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert!(msg.lint_report.is_some());
        crate::core::anti_halluc::set_mode("off");
    }

    #[tokio::test]
    #[serial]
    async fn live_agent_append_with_valid_source_has_no_fabricated() {
        crate::core::anti_halluc::set_mode("warn");
        let (state, _tmp) = lint_state(true).await;
        let out = append(
            &state,
            vec![agent_msg("m1", "Verified. [src: file: src/real.rs:1]")],
        )
        .await;
        assert_eq!(out.appended, 1);
        if let Some(l) = out.lint {
            assert_eq!(
                l.fabricated_count, 0,
                "valid citation must not read as fabricated: {l:?}"
            );
        }
        crate::core::anti_halluc::set_mode("off");
    }

    #[tokio::test]
    #[serial]
    async fn bulk_import_and_user_messages_are_never_linted() {
        crate::core::anti_halluc::set_mode("warn");
        let (state, _tmp) = lint_state(true).await;
        // Bulk (2 messages) with a fabricated citation → no lint.
        let out = append(
            &state,
            vec![
                agent_msg("b1", "one [src: file: src/ghost.rs:1]"),
                agent_msg("b2", "two"),
            ],
        )
        .await;
        assert!(out.lint.is_none(), "bulk import must not lint");
        // Single USER message with a fabricated citation → no lint.
        let user = DiscAppendMessage {
            source_msg_id: "u1".into(),
            role: MessageRole::User,
            content: "look at [src: file: src/ghost.rs:1]".into(),
            agent_type: None,
            targets: Vec::new(),
            target_agent: None,
            reply_to_message_id: None,
        };
        let out = append(&state, vec![user]).await;
        assert!(out.lint.is_none(), "user messages must not lint");
        crate::core::anti_halluc::set_mode("off");
    }

    #[tokio::test]
    #[serial]
    async fn projectless_disc_and_off_mode_skip_lint() {
        // Project-less disc: no roots → no lint, no false fabricated.
        crate::core::anti_halluc::set_mode("warn");
        let (state, _tmp) = lint_state(false).await;
        let out = append(
            &state,
            vec![agent_msg("m1", "x [src: file: src/ghost.rs:1]")],
        )
        .await;
        assert!(out.lint.is_none(), "no project → no lint");
        // Mode off: bound project but lint disabled.
        crate::core::anti_halluc::set_mode("off");
        let (state2, _tmp2) = lint_state(true).await;
        let out = append(
            &state2,
            vec![agent_msg("m1", "x [src: file: src/ghost.rs:1]")],
        )
        .await;
        assert!(out.lint.is_none(), "mode off → no lint");
    }

    #[test]
    fn disc_create_request_deserializes_with_optional_source_binding() {
        // Without source binding — pure local create.
        let minimal: DiscCreateRequest = serde_json::from_str(
            r#"{
            "title": "test",
            "agent": "ClaudeCode"
        }"#,
        )
        .expect("minimal create body must parse");
        assert_eq!(minimal.title, "test");
        assert!(minimal.source_agent.is_none());
        assert!(!minimal.no_agent);

        // With source binding — CLI-initiated import.
        let bound: DiscCreateRequest = serde_json::from_str(
            r#"{
            "title": "imported",
            "agent": "ClaudeCode",
            "source_agent": "ClaudeCode",
            "source_session_id": "abc-123"
        }"#,
        )
        .expect("bound create body must parse");
        assert_eq!(bound.source_agent.as_deref(), Some("ClaudeCode"));
        assert_eq!(bound.source_session_id.as_deref(), Some("abc-123"));
    }

    #[tokio::test]
    async fn disc_create_can_persist_a_no_agent_room() {
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let config = Arc::new(RwLock::new(crate::core::config::default_config()));
        let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);

        let response = disc_create(
            State(state.clone()),
            Json(DiscCreateRequest {
                title: "Peer-only room".into(),
                agent: AgentType::Codex,
                language: Some("fr".into()),
                project_id: None,
                source_agent: None,
                source_session_id: None,
                no_agent: true,
            }),
        )
        .await;
        let created = response.0.data.expect("room creation succeeds");
        let disc_id = created.disc_id;

        let is_no_agent = state
            .db
            .with_conn(move |conn| crate::db::discussions::disc_is_no_agent(conn, &disc_id))
            .await
            .unwrap();
        assert!(
            is_no_agent,
            "a peer-only room must never wake the persisted placeholder agent"
        );
    }

    #[test]
    fn disc_append_requires_source_msg_id_per_entry() {
        // The dedup pass depends on `source_msg_id` being present —
        // missing it is a programmer error, not a runtime fallback.
        // serde_json refuses to deserialize without it.
        let bad = serde_json::from_str::<DiscAppendMessage>(
            r#"{
            "role": "User",
            "content": "no id"
        }"#,
        );
        assert!(
            bad.is_err(),
            "missing source_msg_id must fail deser (dedup invariant)"
        );
    }

    #[test]
    fn disc_append_target_agent_is_optional_and_typed() {
        let without_target = serde_json::from_str::<DiscAppendMessage>(
            r#"{
                "source_msg_id": "source-1",
                "role": "Agent",
                "content": "plain peer message",
                "agent_type": "Codex"
            }"#,
        )
        .unwrap();
        assert_eq!(without_target.target_agent, None);
        assert!(without_target.targets.is_empty());

        let targeted = serde_json::from_str::<DiscAppendMessage>(
            r#"{
                "source_msg_id": "source-2",
                "role": "Agent",
                "content": "@ollama continue",
                "agent_type": "ClaudeCode",
                "target_agent": "Ollama"
            }"#,
        )
        .unwrap();
        assert_eq!(targeted.target_agent, Some(AgentType::Ollama));

        let typed = serde_json::from_str::<DiscAppendMessage>(
            r#"{
                "source_msg_id": "source-3",
                "role": "Agent",
                "content": "@codex-cli réponds",
                "agent_type": "ClaudeCode",
                "targets": [{
                    "kind": "cli",
                    "agent_type": "Codex",
                    "cli_session_id": 42
                }]
            }"#,
        )
        .unwrap();
        assert_eq!(
            typed.targets,
            vec![MessageTarget::cli(AgentType::Codex, 42)]
        );
    }

    #[test]
    fn only_a_fresh_live_peer_turn_wakes_the_native_agent() {
        assert!(is_live_peer_turn(true, Some("joined-session"), 1));
        assert!(
            !is_live_peer_turn(true, None, 1),
            "historical imports have no live session"
        );
        assert!(
            !is_live_peer_turn(false, Some("joined-session"), 1),
            "User/System appends do not wake the native principal"
        );
        assert!(
            !is_live_peer_turn(true, Some("joined-session"), 0),
            "a duplicate append must not retrigger a reply"
        );
        assert!(
            !is_live_peer_turn(true, Some("joined-session"), 2),
            "bulk transcript imports are not live turns"
        );
    }
}
