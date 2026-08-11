//! 0.8.6 phase 2 — Disc invite-peer HTTP routes.
//!
//! Single endpoint for now :
//!
//! - `POST /api/discussions/:id/invite-peer` — generate a single-use
//!   token an agent (host-launched in some other terminal) consumes
//!   via the `disc_join` MCP tool to attach to this disc.
//!
//! The token is returned PLAIN once, then the DB only ever sees its
//! SHA-256 hash (see `db::discussion_sessions::create_invite_token`).
//! Read the module-level doc in `db/discussion_sessions.rs` for the
//! security model and `project_cross_agent_collab_demo.md` in memory
//! for the wider design rationale.
//!
//! The companion consume endpoint (`disc_join` from the bridge) lives
//! in [`disc_session_join`](crate::api::disc_session_join) — kept
//! separate because invite is human-triggered (UI button) while join
//! is agent-triggered (MCP tool).

use axum::{
    extract::{Path, Query, State},
    Json,
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::sleep;
use ts_rs::TS;
use uuid::Uuid;

use crate::db;
use crate::models::{
    ApiResponse, MessageTarget, MessageTargetKind, PlanningTaskStatus, PlanningTaskSummary,
};
use crate::AppState;

/// Wire shape returned by the invite endpoint. The frontend displays
/// `instruction_text` directly in the copy-paste modal — the wording
/// lives server-side so we can tweak it (i18n, channel, etc.) without
/// shipping a frontend release.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InviteResponse {
    pub token: String,
    pub disc_id: String,
    pub expires_at: String,
    pub ttl_seconds: i64,
    /// Handoff to paste into the invited agent's terminal. This is the FIRST
    /// thing that agent reads — before `disc_join` even answers — so it carries
    /// the working contract, not just the token.
    pub instruction_text: String,
    /// Token-only form, for a human who just wants the bare call (KT-52).
    pub instruction_text_minimal: String,
}

/// Body of `POST /api/discussions/peer-join`. The token is the
/// plaintext returned by `invite_peer`. `agent_type` + `session_id`
/// identify the calling CLI session so the bridge can rebind a
/// disconnected agent on reconnect.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct PeerJoinRequest {
    pub token: String,
    /// `ClaudeCode | Codex | GeminiCli | Kiro | CopilotCli | Vibe | Ollama | Custom`
    /// — same enum as the Rust `AgentType`.
    pub agent_type: String,
    /// CLI-assigned session id. UUID-like for Claude Code, numeric or
    /// string for others. Treated as an opaque identifier.
    pub session_id: String,
    /// KT-37 — the model the joining CLI declares it runs on (e.g.
    /// `"claude-opus-4"`). Optional, self-declared, never inferred: trimmed and
    /// bounded, stored as declared-at-join. Omitted or blank preserves any
    /// value declared on a previous join/rebind (legacy bridges omit it).
    #[serde(default)]
    pub model: Option<String>,
    /// Native conversation id used by the CLI's own resume command. Optional
    /// and distinct from the Kronn bridge `session_id`.
    #[serde(default)]
    pub conversation_id: Option<String>,
}

fn normalize_conversation_id(raw: Option<&str>) -> Result<Option<String>, &'static str> {
    let Some(raw) = raw.filter(|id| !id.is_empty()) else {
        return Ok(None);
    };
    if raw != raw.trim() || raw.chars().count() > 512 {
        return Err("conversation_id must be a canonical UUID");
    }
    Uuid::parse_str(raw)
        .map(|id| Some(id.to_string()))
        .map_err(|_| "conversation_id must be a canonical UUID")
}

/// Wire shape returned by `peer-join`. Carries the disc id (so the
/// bridge can stash it as its `_CURRENT_DISC_ID`), a peer count for
/// the agent's first system-prompt notice, and a recent-message
/// preview so the joiner has immediate context.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PeerJoinResponse {
    pub disc_id: String,
    pub session_pk: i64,
    /// Opaque reload credential. Persist locally with mode 0600; never log or
    /// expose it to the model. The backend stores only its SHA-256 digest.
    pub resume_token: String,
    pub peer_count: i64,
    /// Title of the disc, surfaced in the agent's first reply so the
    /// human can verify it joined the right conversation.
    pub disc_title: String,
    /// Number of out-of-context notes in the room. Their bodies are omitted
    /// from join context and require an explicit note-list call.
    pub note_count: u32,
    /// Last N messages already in the disc (default 10). Empty for a
    /// freshly-created topic.
    pub recent_messages: Vec<RecentMessagePreview>,
    /// Compact, bounded plan state available even when the MCP client cached
    /// an older tool catalogue and cannot call `plan_get` yet.
    pub plan_snapshot: PeerJoinPlanSnapshot,
    /// 0.8.6 fix 2026-05-21 — explicit directive returned to the
    /// agent so it understands the multi-agent protocol. Without
    /// this, agents like Codex/Vibe would `disc_join` and then just
    /// print their intro to their own terminal (invisible to peers).
    /// The text tells them : *use disc_append to speak*, don't just
    /// reply to the user in your terminal.
    pub next_steps: String,
    /// Long-poll pacing contract (stab-1) — walk `poll_backoff_seconds`
    /// while the room is silent, reset on any peer message.
    #[serde(default)]
    pub poll_policy: crate::api::disc_introspection::PollBackoffPolicy,
    /// stab-3 — server-computed pacing, same contract as wait/meta: apply
    /// `next_delay_seconds` verbatim before the FIRST wait. Included at
    /// join so a fresh peer doesn't need a meta/wait round-trip to pace
    /// itself (Copilot review: join was the one response missing it).
    #[serde(default)]
    pub pacing: crate::api::disc_introspection::PacingState,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PeerJoinPlanTaskPreview {
    pub reference: String,
    pub title: String,
    pub status: PlanningTaskStatus,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PeerJoinPlanSnapshot {
    pub primary_objective: Option<PeerJoinPlanTaskPreview>,
    /// First eight non-completed Active tasks, in plan order.
    pub current: Vec<PeerJoinPlanTaskPreview>,
    /// Three most recently updated completed Active tasks.
    pub recently_completed: Vec<PeerJoinPlanTaskPreview>,
    pub current_total: u32,
    pub completed_total: u32,
    pub later_total: u32,
}

fn peer_join_task_preview(task: &PlanningTaskSummary) -> PeerJoinPlanTaskPreview {
    PeerJoinPlanTaskPreview {
        reference: task.reference.clone(),
        title: task.title.clone(),
        status: task.status,
    }
}

fn peer_join_plan_snapshot(
    connection: &rusqlite::Connection,
    discussion_id: &str,
) -> anyhow::Result<PeerJoinPlanSnapshot> {
    let plan = db::planning::get_discussion_plan(connection, discussion_id)?;
    let mut completed = plan
        .active
        .iter()
        .filter(|relation| relation.task.status == PlanningTaskStatus::Done)
        .map(|relation| &relation.task)
        .collect::<Vec<_>>();
    completed.sort_by_key(|task| std::cmp::Reverse(task.updated_at));

    let current = plan
        .active
        .iter()
        .filter(|relation| {
            !matches!(
                relation.task.status,
                PlanningTaskStatus::Done | PlanningTaskStatus::Archived
            )
        })
        .map(|relation| peer_join_task_preview(&relation.task))
        .collect::<Vec<_>>();

    Ok(PeerJoinPlanSnapshot {
        primary_objective: plan.primary_objective.as_ref().map(peer_join_task_preview),
        current_total: current.len() as u32,
        completed_total: plan.stats.done,
        later_total: plan.stats.later,
        current: current.into_iter().take(8).collect(),
        recently_completed: completed
            .into_iter()
            .take(3)
            .map(peer_join_task_preview)
            .collect(),
    })
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct PeerResumeRequest {
    pub agent_type: String,
    pub session_id: String,
    pub resume_token: String,
    /// Native CLI conversation id observed after reconnect or `/clear`.
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// Client-prepared successor credential. The bridge persists it as a
    /// pending value *before* this request, making the rotation retryable if
    /// the response is lost. Omitted by legacy bridges, which resume without
    /// rotation rather than risking a server-first, unacknowledged cut-over.
    #[serde(default)]
    pub next_resume_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PeerResumeResponse {
    pub disc_id: String,
    pub session_pk: i64,
    /// Rotated credential replacing the one supplied in the request.
    pub resume_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RecentMessagePreview {
    pub sort_order: i64,
    pub role: String,
    pub agent_type: Option<String>,
    pub timestamp: String,
    /// Body trimmed to 400 chars so the response stays small. The
    /// agent can `disc_get_message(idx)` to fetch full text.
    pub preview: String,
}

/// `POST /api/discussions/peer-join`
///
/// Validates the invite token, creates a peer `discussion_sessions`
/// row, and returns enough context for the bridge to bind + the
/// agent to greet the other participants.
pub async fn peer_join(
    State(state): State<AppState>,
    Json(req): Json<PeerJoinRequest>,
) -> Json<ApiResponse<PeerJoinResponse>> {
    if req.token.trim().is_empty() {
        return Json(ApiResponse::err("token required"));
    }
    if req.agent_type.trim().is_empty() {
        return Json(ApiResponse::err("agent_type required"));
    }
    if req.session_id.trim().is_empty() {
        return Json(ApiResponse::err("session_id required"));
    }
    // KT-37 — a declared model is recorded verbatim, never silently truncated.
    // An over-long declaration is rejected with a clear message (the client can
    // retry without it or shorten it) rather than storing a mangled name.
    if req
        .model
        .as_deref()
        .map(str::trim)
        .is_some_and(|m| m.chars().count() > 200)
    {
        return Json(ApiResponse::err(
            "model declaration too long (max 200 chars) — omit it or shorten it",
        ));
    }
    let conversation_id = match normalize_conversation_id(req.conversation_id.as_deref()) {
        Ok(id) => id,
        Err(error) => return Json(ApiResponse::err(error)),
    };

    let token = req.token.clone();
    let agent_type = req.agent_type.clone();
    let session_id = req.session_id.clone();

    // Resolve (disc_id, session_pk): first try a LOCAL token join; on a local
    // miss, fall back to asking our contacts who hosts the room (the unified
    // "join by code"). The owning peer shares the disc back over the WS
    // federation, we mirror it, and bind a session to the mirror.
    let (disc_id, session_pk, resume_token) = {
        let (t, a, s) = (token.clone(), agent_type.clone(), session_id.clone());
        let local = state
            .db
            .with_conn(move |conn| db::discussion_sessions::join_via_token(conn, &t, &a, &s))
            .await;
        match local {
            Ok(j) => (j.disc_id, j.session_pk, j.resume_token),
            Err(local_err) => {
                match try_remote_join(&state, &token, &agent_type, &session_id).await {
                    Ok(Some(r)) => r,
                    // No contact hosts it → surface the original local error.
                    Ok(None) => return Json(ApiResponse::err(local_err.to_string())),
                    Err(remote_err) => {
                        return Json(ApiResponse::err(format!(
                            "join failed locally ({local_err}) and via contacts ({remote_err})"
                        )))
                    }
                }
            }
        }
    };

    // KT-37 — record the optionally-declared model. Declared at join, explicit
    // updates only: a blank/omitted value never overwrites an existing one, so
    // we only write when the joiner actually declared something. Trimmed but
    // never truncated (length already validated above).
    if let Some(model) = req
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
    {
        let pk = session_pk;
        if let Err(e) = state
            .db
            .with_conn(move |conn| db::discussion_sessions::set_session_model(conn, pk, &model))
            .await
        {
            tracing::warn!("peer_join: failed to record declared model: {e}");
        }
    }
    if let Some(conversation_id) = conversation_id {
        let pk = session_pk;
        if let Err(e) = state
            .db
            .with_conn(move |conn| {
                db::discussion_sessions::set_session_conversation_id(conn, pk, &conversation_id)
            })
            .await
        {
            tracing::warn!("peer_join: failed to record native conversation id: {e}");
        }
    }

    // Build the response from the resolved disc (shared by local + mirror paths).
    let res = state
        .db
        .with_conn(move |conn| {
            // Step 2 — disc title + peer count for the response.
            let disc_title: String = conn.query_row(
                "SELECT title FROM discussions WHERE id = ?1",
                rusqlite::params![&disc_id],
                |r| r.get(0),
            )?;
            let peer_count = db::discussion_sessions::count_active_participants(conn, &disc_id)?;
            let note_count = conn.query_row(
                "SELECT COUNT(*) FROM messages
                 WHERE discussion_id = ?1 AND channel = 'note'",
                rusqlite::params![&disc_id],
                |row| row.get::<_, u32>(0),
            )?;

            // Step 3 — recent messages (last 10, trimmed). Newest last
            // so the agent can read top→bottom.
            let mut stmt = conn.prepare(
                "SELECT sort_order, role, agent_type, content, timestamp
                   FROM messages
                  WHERE discussion_id = ?1 AND channel = 'main'
                  ORDER BY sort_order DESC
                  LIMIT 10",
            )?;
            let mut rows: Vec<RecentMessagePreview> = stmt
                .query_map(rusqlite::params![&disc_id], |r| {
                    let content: String = r.get(3)?;
                    let preview: String = content.chars().take(400).collect();
                    Ok(RecentMessagePreview {
                        sort_order: r.get(0)?,
                        role: r.get(1)?,
                        agent_type: r.get(2)?,
                        preview,
                        timestamp: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.reverse();

            // 0.8.6 fix 2026-05-21 (revised) — explicit + imperative
            // multi-agent protocol. Without this :
            //   - first test : Codex/Vibe printed intro to terminal
            //     instead of disc_append-ing (peers never saw them).
            //   - second test : even Claude skipped the intro and
            //     went straight to disc_wait_for_peer, leaving the
            //     room visibly empty for the human watching the UI.
            // The directive is now imperative ("DO THIS FIRST"),
            // numbered, and tells the agent to introduce ITSELF
            // regardless of `peer_count` (don't wait for others to
            // arrive — the human watching the UI needs to see life).
            let next_steps = join_next_steps(&disc_id, &disc_title, peer_count);
            let plan_snapshot = peer_join_plan_snapshot(conn, &disc_id)?;

            Ok::<_, anyhow::Error>(PeerJoinResponse {
                poll_policy: crate::api::disc_introspection::PollBackoffPolicy::default(),
                // Cold-cap placeholder — replaced with the real
                // server-computed value right after the closure (pacing
                // needs an async read the closure can't perform).
                pacing: Default::default(),
                disc_id,
                session_pk,
                resume_token,
                peer_count,
                disc_title,
                note_count,
                recent_messages: rows,
                plan_snapshot,
                next_steps,
            })
        })
        .await;

    match res {
        Ok(mut r) => {
            r.pacing = pacing_for_disc(&state, &r.disc_id).await;
            Json(ApiResponse::ok(r))
        }
        Err(e) => Json(ApiResponse::err(e.to_string())),
    }
}

/// The protocol handed to an agent that just joined a room. Extracted from
/// the handler so the directives agents keep dropping — read the shared plan,
/// stay in the room and follow it — are pinned by a test.
fn join_next_steps(disc_id: &str, disc_title: &str, peer_count: i64) -> String {
    format!(
        "✅ You joined `{}` (title: {:?}, {} active participant(s) including you).\n\n\
         ⚠ REQUIRED PROTOCOL — execute IN ORDER, do NOT skip step 1 :\n\n\
         STEP 1 (DO IMMEDIATELY, EVEN IF YOU'RE THE FIRST/ONLY PARTICIPANT) :\n\
         Call `disc_append({{content: \"<your introduction>\"}})` to introduce \
         yourself in the shared discussion. State your CLI name + your role in \
         this conversation. The human watching the Kronn UI needs to see you \
         are alive. Replying only in your local terminal is INVISIBLE to peers — \
         you MUST go through `disc_append`.\n\n\
         STEP 2 :\n\
         If `recent_messages` (above) contains user instructions or peer \
         statements that demand a substantive first reply (e.g. 'start the \
         match', 'propose a plan'), make that reply via a SECOND `disc_append` \
         call right after your intro.\n\n\
         STEP 3 — READ THE SHARED PLAN BEFORE ACTING :\n\
         This room may already have an objective and tasks in flight. Call \
         `plan_get` (current objective + active tasks) and `task_list` if you \
         need the wider backlog, so you pick up the real work instead of \
         guessing or asking the human to re-explain. You may READ **and \
         UPDATE** those tasks — `task_create`, `task_update`, \
         `task_update_dod`, `task_add_blocker` — and you are expected to keep \
         them honest as you go (a task you finished must not stay open, a \
         blocker you discovered must be recorded). Write only when tracked \
         work actually starts or materially changes; do not reload or rewrite \
         unchanged tasks merely to report progress. The whole \
         `kronn-internal` surface is available to you, not just the `disc_*` \
         tools: workflows, api_call, skills, directives, profiles, audits.\n\
         If one of the Planning tools named above is missing from your actual \
         MCP tool surface, your client cached an older catalogue: use the \
         read-only `plan_snapshot` in this join response, tell @user to \
         reconnect the Kronn MCP, and do not fabricate or claim any task \
         update.\n\n\
         STEP 4 — ANNOUNCE BEFORE THE FIRST SUBSTANTIVE ACTION :\n\
         Before editing files, running a substantive command or triggering an \
         external action, call `disc_append` with a concise \
         \"task / scope / next action\" update. Peers and the human must know \
         what you are taking before implementation starts. Pure context reads \
         do not need a noisy announcement. If the scope changes materially, \
         post one updated intent before continuing.\n\n\
         RECONNECTING IS ALREADY HANDLED — DO NOT ASK FOR A NEW TOKEN :\n\
         This join linked your durable CLI session to this room (see \
         `session_bound` in the join result). After an MCP reload, call \
         `disc_find_by_session` — you get this discussion back without a fresh \
         invite. If `session_bound` is false, the reason is stated next to it: \
         your session has no durable identity, or it already belongs to another \
         discussion. Do not force a transfer on your own initiative; ask the \
         human first.\n\n\
         STEP 5 — STAY IN THE ROOM AND FOLLOW IT (this is the part agents \
         get wrong) :\n\
         a. Call `disc_wait_for_peer()` to wait for the next message. The \
         bridge chains quiet server polls instead of returning after each one. \
         HOST CAVEAT: if your client says the tool call was moved to the \
         background, the original wait is still active — do NOT start another \
         wait. Wait for that task's terminal notification, then re-arm only if \
         its completed result is quiet. Some hosts expose a model-visible \
         background-task notification, so zero-turn silence is a host \
         capability, not a universal bridge guarantee.\n\
         b. If it returns `timed_out: true` with NO new messages (safety \
         cap or interruption), that is NORMAL (the peer may still be \
         thinking) — re-arm `disc_wait_for_peer` when you are ready to \
         listen again. A quiet window is NOT the end of the conversation; \
         never stop or leave just because a wait came back quiet.\n\
         c. When messages arrive, read them, then call `disc_append({{content: \
         \"<your reaction>\"}})` to reply.\n\
         d. If the room stays quiet and you have nothing to answer, do NOT \
         idle and do NOT end your turn on a summary: take the next actionable \
         task from the plan, do it, and report it in the room. Silence from \
         you is indistinguishable from having left, and the human WILL read it \
         that way.\n\
         e. Go back to (a).\n\n\
         JOINING IS NOT THE TASK. Reporting progress is not the end of your \
         work either — the room is done when the plan is done or the human \
         says stop. To leave : `disc_leave()`, and only then.",
        disc_id, disc_title, peer_count,
    )
}

/// `POST /api/discussions/peer-resume`
///
/// Restore a bridge binding after MCP reload without minting or reusing an
/// invite token. A successful call rotates the opaque credential and updates
/// the original participant row in place.
pub async fn peer_resume(
    State(state): State<AppState>,
    Json(req): Json<PeerResumeRequest>,
) -> Json<ApiResponse<PeerResumeResponse>> {
    if req.agent_type.trim().is_empty() {
        return Json(ApiResponse::err("agent_type required"));
    }
    if req.session_id.trim().is_empty() {
        return Json(ApiResponse::err("session_id required"));
    }
    if req.resume_token.trim().is_empty() {
        return Json(ApiResponse::err("resume_token required"));
    }
    let conversation_id = match normalize_conversation_id(req.conversation_id.as_deref()) {
        Ok(id) => id,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    if let Some(next) = req.next_resume_token.as_deref() {
        let suffix = next.strip_prefix("kr-resume-").unwrap_or_default();
        if suffix.len() != 32 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Json(ApiResponse::err(
                "next_resume_token must be kr-resume- followed by 32 hex characters",
            ));
        }
    }
    let agent_type = req.agent_type;
    let session_id = req.session_id;
    let resume_token = req.resume_token;
    let next_resume_token = req.next_resume_token;
    match state
        .db
        .with_conn(move |conn| {
            let resumed = db::discussion_sessions::resume_disc_session(
                conn,
                &agent_type,
                &resume_token,
                &session_id,
                next_resume_token.as_deref(),
            )?;
            if let Some(conversation_id) = conversation_id {
                db::discussion_sessions::set_session_conversation_id(
                    conn,
                    resumed.session_pk,
                    &conversation_id,
                )?;
            }
            Ok(resumed)
        })
        .await
    {
        Ok(resumed) => Json(ApiResponse::ok(PeerResumeResponse {
            disc_id: resumed.disc_id,
            session_pk: resumed.session_pk,
            resume_token: resumed.resume_token,
        })),
        Err(e) => Json(ApiResponse::err(e.to_string())),
    }
}

/// Cross-instance leg of `peer_join`: the token wasn't found locally, so ask
/// each accepted contact "do you host the room behind this code?" via their
/// `/api/disc/claim-by-token` endpoint. The owning peer shares the disc back
/// (broadcasts a `DiscussionInvite` relayed to us over the WS federation); we
/// poll for the mirror disc to land, then bind a session to it. Returns the
/// mirror `(disc_id, session_pk)` on success, `None` if no contact hosts it.
async fn try_remote_join(
    state: &AppState,
    token: &str,
    agent_type: &str,
    session_id: &str,
) -> anyhow::Result<Option<(String, i64, String)>> {
    // Our own invite code — the credential the peer validates against its contacts.
    let our_code = {
        let cfg = state.config.read().await;
        crate::api::contacts::build_invite_code(&cfg.server).await
    };

    let contacts = state
        .db
        .with_conn(db::contacts::list_contacts)
        .await
        .unwrap_or_default();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .build()?;

    for contact in contacts.into_iter().filter(|c| c.status == "accepted") {
        let url = format!(
            "{}/api/disc/claim-by-token",
            contact.kronn_url.trim_end_matches('/')
        );
        let body = serde_json::json!({ "token": token, "from_invite_code": our_code });
        let resp = match client.post(&url).json(&body).send().await {
            Ok(r) => r,
            Err(_) => continue, // unreachable peer → try the next contact
        };
        let parsed: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };
        let data = parsed.get("data");
        let found = data
            .and_then(|d| d.get("found"))
            .and_then(|f| f.as_bool())
            .unwrap_or(false);
        if !found {
            continue;
        }
        let Some(shared_id) = data
            .and_then(|d| d.get("shared_id"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
        else {
            continue;
        };

        // We already have shared_id + title from the HTTP claim response, so
        // create the mirror DIRECTLY rather than waiting for the peer's WS
        // `DiscussionInvite` to arrive — that race is fragile under NAT / WS
        // lag and was a cause of "the shared disc never showed up". The WS
        // invite, when/if it lands, is an idempotent no-op. Then bind our
        // session to the mirror.
        let title = data
            .and_then(|d| d.get("title"))
            .and_then(|t| t.as_str())
            .unwrap_or("Discussion")
            .to_string();
        let (sid, ttl, from) = (shared_id.clone(), title, contact.pseudo.clone());
        let mirror_disc_id = state
            .db
            .with_conn(move |conn| {
                crate::db::discussions::ensure_mirror_by_shared_id(conn, &sid, &ttl, &from)
            })
            .await?;
        let (mdid, a, s) = (
            mirror_disc_id.clone(),
            agent_type.to_string(),
            session_id.to_string(),
        );
        let joined = state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::join_disc_session_resumable(conn, &mdid, &a, &s)
            })
            .await?;
        return Ok(Some((
            mirror_disc_id,
            joined.session_pk,
            joined.resume_token,
        )));
    }
    Ok(None)
}

// ─── disc_leave (0.8.6 phase 3) ────────────────────────────────────

/// Body of `POST /api/discussions/peer-leave`. Identifies the caller
/// the same way `peer_join` does — by `(agent_type, session_id)` —
/// so the bridge can find its own active session row and mark it left.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct PeerLeaveRequest {
    pub agent_type: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PeerLeaveResponse {
    /// `true` when an active session was found + marked left.
    /// `false` when the caller had no active session (already left,
    /// or never joined). Either way, idempotent.
    pub left: bool,
}

/// `POST /api/discussions/peer-leave`
///
/// Looks up the active `discussion_sessions` row for the calling
/// (agent_type, session_id) pair and marks it `left`. Idempotent —
/// calling twice doesn't error. The bridge calls this from
/// `disc_leave` MCP tool ; the participants header live-refresh
/// (phase 3) picks up the change on next refetch.
pub async fn peer_leave(
    State(state): State<AppState>,
    Json(req): Json<PeerLeaveRequest>,
) -> Json<ApiResponse<PeerLeaveResponse>> {
    if req.agent_type.trim().is_empty() || req.session_id.trim().is_empty() {
        return Json(ApiResponse::err("agent_type + session_id required"));
    }
    let agent_type = req.agent_type.clone();
    let session_id = req.session_id.clone();

    let res = state
        .db
        .with_conn(move |conn| {
            let row = db::discussion_sessions::find_active_session(conn, &agent_type, &session_id)?;
            let Some(s) = row else {
                return Ok(PeerLeaveResponse { left: false });
            };
            db::discussion_sessions::mark_session_left(conn, s.id)?;
            // 0.8.12 PR B — a departed agent keeps no activity placeholder.
            db::discussion_sessions::clear_session_activity(
                conn,
                &s.disc_id,
                &agent_type,
                s.session_id.as_deref(),
            )?;
            Ok(PeerLeaveResponse { left: true })
        })
        .await;

    match res {
        Ok(r) => Json(ApiResponse::ok(r)),
        Err(e) => Json(ApiResponse::err(e.to_string())),
    }
}

// ─── disc_wait_for_peer (0.8.6 phase 3) ────────────────────────────

/// Query params for `wait_for_peer`. `since_sort_order` is the highest
/// `messages.sort_order` the caller has already observed — only newer
/// messages count as "peer activity". `timeout_secs` is clamped to
/// [1, 90] server-side to bound long-running requests.
#[derive(Debug, Clone, Deserialize)]
pub struct WaitForPeerQuery {
    #[serde(default)]
    pub since_sort_order: Option<i64>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Optional : exclude messages from this `agent_type` so an agent
    /// doesn't wake itself on its own append. When omitted, all new
    /// messages trigger the wake.
    #[serde(default)]
    pub exclude_agent_type: Option<String>,
    /// The caller's session id. Kept optional for wire compatibility, but an
    /// omitted value never mutates presence: broad agent-type heartbeats let
    /// a stale bridge process keep a resumed sibling alive.
    #[serde(default)]
    pub session_id: Option<String>,
    /// KT-114 — late capture of the CLI's native resume id. A fresh Codex TUI
    /// has nothing to declare at join time; once its bridge resolves the id
    /// (from the CLI's own open session file), the idle wait carries it here so
    /// the Resume button appears without any extra round trip. Validated and
    /// bounded like the join-time value; ignored without a session identity.
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// KT-189 — durable delivery acknowledgement for awareness turns. The
    /// bridge sends the highest awareness `sort_order` whose delivery to the
    /// model was CONFIRMED (two-phase: staged on emission, committed by the
    /// model's next tool call). The server only advances the per-session
    /// awareness cursor here — never at emission — so a response lost to a
    /// cancellation or a bridge crash is replayed instead of skipped.
    #[serde(default)]
    pub ack_awareness_upto: Option<i64>,
}

/// KT-189 — upper bound of awareness turns attached to one wake. A chatty
/// room can accumulate hundreds of turns between wakes; the batch stays
/// readable and the omitted count says the rest (still unacked, so the
/// remainder returns with the next wake).
const AWARENESS_MAX_MESSAGES: usize = 20;

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WaitForPeerMessage {
    /// Durable identifier of the delivered transcript message or revision
    /// event. Callers can cite it in an acknowledgement and use it as
    /// `reply_to_message_id` when the item is a transcript message.
    pub message_id: String,
    pub sort_order: i64,
    pub role: String,
    pub agent_type: Option<String>,
    pub content: String,
    pub timestamp: String,
    /// Author pseudo for messages that arrived from a PEER instance (federated)
    /// or a human; `None` for our own local appends. Lets the wait correctly
    /// treat a same-`agent_type` peer (e.g. another ClaudeCode instance) as a
    /// real peer instead of filtering it out as "self".
    pub author_pseudo: Option<String>,
    /// Present for projection-change events that are cursor-visible but not
    /// rendered as transcript messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_message_id: Option<String>,
    /// Structured addressee for this turn. A waiting peer whose own agent type
    /// differs must not answer it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_agent: Option<String>,
    /// Every structured addressee, in the order written by the human. Peers
    /// answer when their own agent type is present and otherwise only observe.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_agents: Vec<String>,
    /// Authoritative target identities. Unlike `target_agents`, this tells a
    /// native punctual agent from the configured discussion agent and from one
    /// exact joined CLI session of the same provider.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<MessageTarget>,
    /// Exact local CLI identity that authored this message. A caller can use
    /// it as the durable reply target without guessing from provider names.
    /// `None` for humans, native agents, imports and revision events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_target: Option<MessageTarget>,
    /// KT-189 — `true` for a turn delivered as AWARENESS: it did not target
    /// this CLI (untargeted room traffic, or a turn addressed to another
    /// responder) and is attached, bounded and once, to a legitimate wake so
    /// the session keeps full room context without being woken for it.
    /// Context only: the caller must NOT answer it; the addressed responder
    /// owns that turn.
    #[serde(default, skip_serializing_if = "is_false")]
    pub awareness: bool,
    /// Server-computed for the calling durable CLI session. A CLI answers only
    /// when this is true; matching the provider name is intentionally
    /// insufficient because `@codex` and `@codex · CLI` are distinct targets.
    #[serde(default)]
    pub addressed_to_caller: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WaitForPeerResponse {
    /// `true` when the loop hit the timeout without any new messages.
    /// Lets the caller (the agent's MCP tool) decide whether to retry
    /// or surface "no activity in the last 60s" to the user.
    pub timed_out: bool,
    /// New messages since `since_sort_order` (empty when `timed_out=true`).
    pub messages: Vec<WaitForPeerMessage>,
    /// Highest sort_order in the returned batch (or the input
    /// `since_sort_order` when timed out). Lets the agent advance its
    /// `since` cursor without inspecting the messages.
    pub latest_sort_order: i64,
    /// KT-189 — awareness turns beyond the per-wake attach cap. Non-zero means
    /// older unseen turns exist that were NOT attached this time; they remain
    /// unacked and return with the next wake, and the human-side transcript
    /// remains the complete record.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub awareness_omitted: u32,
    /// KT-189 — highest `sort_order` among the awareness turns attached to
    /// this response. The bridge echoes it back as `ack_awareness_upto` once
    /// the model's next tool call proves the delivery was consumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awareness_delivered_upto: Option<i64>,
    /// stab-3 — server-computed pacing: apply `next_delay_seconds` before
    /// the next wait, verbatim. Hot (short interval) while a User message is
    /// within the attention lease; otherwise the next DETERMINISTIC step of
    /// the cold backoff ramp, derived from the elapsed silence.
    pub pacing: crate::api::disc_introspection::PacingState,
    /// Presence-gap fix — when `timed_out`, the RFC3339 instant this session
    /// intends to poll again (`now + pacing.next_delay_seconds`). Consumed by
    /// the MCP CALLER (to schedule its next wait); the participants UI does
    /// NOT read this field — it derives "dormant" from the paired `waiting`
    /// activity (generic label, no countdown). `None` on a delivery (the
    /// caller replies now, not later).
    pub next_poll_at: Option<String>,
    /// How many window turns this response carries NEITHER as a wake NOR as
    /// attached awareness — rows still gated by the awareness cap or the ack
    /// cursor (they return with a later wake), and rows only visible to other
    /// identities. `latest_sort_order` still counts them, otherwise the caller
    /// would loop on a cursor gap. Excludes the caller's own appends: it wrote
    /// those, they were never news. Since KT-189 nothing content-bearing is
    /// permanently withheld: awareness delivery is deferred, not denied.
    pub withheld_by_routing: u32,
}

/// stab-3 — pacing for a disc: hot while the last User message is within
/// the attention lease. DB errors degrade to cold (the conservative regime).
pub(crate) async fn pacing_for_disc(
    state: &AppState,
    disc_id: &str,
) -> crate::api::disc_introspection::PacingState {
    let did = disc_id.to_string();
    let (last_user, last_any) = match state
        .db
        .with_read_conn(move |conn| {
            Ok((
                crate::db::discussions::last_user_message_at(conn, &did)?,
                crate::db::discussions::last_message_at(conn, &did)?,
            ))
        })
        .await
    {
        Ok(anchors) => anchors,
        Err(e) => {
            // Explicit degradation: a DB failure yields the conservative
            // cold regime, and the incident is visible (Codex review).
            tracing::warn!(disc = %disc_id, error = %e, "pacing anchors unavailable — cold fallback");
            (None, None)
        }
    };
    crate::api::disc_introspection::pacing_for(
        last_user,
        last_any,
        chrono::Utc::now(),
        &crate::api::disc_introspection::PollBackoffPolicy::default(),
    )
}

const WAIT_POLL_INTERVAL_MS: u64 = 1000;
const WAIT_TIMEOUT_DEFAULT_SECS: u64 = 60;
/// KT-43 — every returned wait is a dead window: the agent gets its turn back,
/// and whether it loops again depends on the external harness, not on us. A
/// longer single block therefore removes latency that no instruction can fix.
/// The ceiling is NOT free to raise: the MCP bridge reads this response with a
/// 180 s client timeout (`_http` in `backend/scripts/disc-introspection-mcp.py`),
/// so a server block at or past that would surface as a transport error instead
/// of a clean `timed_out`. Kept below it with room to spare; a Python test pins
/// the relationship from the other side.
const WAIT_TIMEOUT_MAX_SECS: u64 = 170;
/// 0.8.12 PR B — "listening" outlives the requested wait by this margin,
/// then expires on its own (read-side expiry, no reaper).
const ACTIVITY_LISTENING_MARGIN_SECS: i64 = 30;
/// "reading" = delivered-but-not-replied. Short by design: past this the
/// placeholder would be a guess, and guesses are what phase 1 forbids.
const ACTIVITY_READING_TTL_SECS: i64 = 120;
/// Presence-gap fix — on a timed-out wait the caller sleeps for
/// `pacing.next_delay_seconds` (up to the cold-ramp max) before polling
/// again. `listening` (TTL = timeout + 30s) expires inside that pause, so
/// the participant flipped to "disconnected" while merely dormant. We set a
/// distinct `waiting` activity for the pacing window + this margin: honest
/// ("I'll be back at next_poll_at"), and it still expires on its own if the
/// process is truly dead. Never prolong `listening` — that would lie.
const ACTIVITY_WAITING_MARGIN_SECS: i64 = 15;

/// `GET /api/discussions/:id/wait`
///
/// Long-polling endpoint : sleeps in ~1s ticks, returning as soon as
/// a new message (newer than `since_sort_order`, optionally excluding
/// the caller's own `agent_type`) appears in the disc. Bounded by
/// `timeout_secs` (default 60s, max 170s).
///
/// The bridge's `disc_wait_for_peer` MCP tool calls this. Polling-
/// based rather than broadcast/SSE because (a) the disc-message
/// append path already touches enough code, and (b) 1s latency is
/// fine for agent-to-agent collab in the seconds-to-minutes range.
/// Can be upgraded to a tokio broadcast channel later without
/// changing the wire contract.
pub async fn wait_for_peer(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
    Query(q): Query<WaitForPeerQuery>,
) -> Json<ApiResponse<WaitForPeerResponse>> {
    let since = q.since_sort_order.unwrap_or(-1);
    let timeout_secs = q
        .timeout_secs
        .unwrap_or(WAIT_TIMEOUT_DEFAULT_SECS)
        .clamp(1, WAIT_TIMEOUT_MAX_SECS);
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    let exclude = q.exclude_agent_type;
    let session_id = q.session_id;
    let caller_session_pk = if let (Some(agent_type), Some(caller_session_id)) =
        (exclude.as_ref(), session_id.as_ref())
    {
        let agent = agent_type.clone();
        let session = caller_session_id.clone();
        let did = disc_id.clone();
        state
            .db
            .with_read_conn(move |conn| {
                Ok(
                    crate::db::discussion_sessions::find_active_session(conn, &agent, &session)?
                        .filter(|row| row.disc_id == did)
                        .map(|row| (row.id, None)),
                )
            })
            .await
            .unwrap_or(None)
    } else if let Some(caller_session_id) = session_id.as_ref() {
        // KT-189 — a bridge whose provider is unresolved ("Unknown") omits
        // exclude_agent_type but still identifies its durable session. It
        // must get the modern wake/awareness contract, not the legacy
        // wake-on-everything projection — and its presence (heartbeat,
        // listening/reading/waiting states) must stay alive too, using the
        // agent_type stored on its own session row.
        let session = caller_session_id.clone();
        let did = disc_id.clone();
        state
            .db
            .with_read_conn(move |conn| {
                Ok(
                    crate::db::discussion_sessions::find_active_session_by_id(conn, &session)?
                        .filter(|(_, session_disc, _)| *session_disc == did)
                        .map(|(pk, _, agent_type)| (pk, Some(agent_type))),
                )
            })
            .await
            .unwrap_or(None)
    } else {
        None
    };
    let (caller_session_pk, resolved_agent_type) = match caller_session_pk {
        Some((pk, resolved)) => (Some(pk), resolved),
        None => (None, None),
    };
    // Presence identity: the declared provider, or the one stored on the
    // resolved session row when the bridge could not name its provider.
    let presence_agent: Option<String> = exclude.clone().or(resolved_agent_type);

    // Liveness heartbeat (migration 064). The agent's idle loop calls
    // this every ≤90s with `exclude_agent_type = its own type` (so it
    // doesn't wake on its own append) — that's exactly its identity, and
    // entering the wait is proof it's alive. Bump last_seen at the START
    // (not after the long-poll) so a crashed agent's session goes stale
    // promptly. Best-effort: a DB hiccup here must not block the wait.
    //
    // 0.8.12 PR B — presence phase 1: an open wait IS the "listening"
    // fact. TTL = requested timeout + margin, so a crashed agent's
    // placeholder dies on its own (expiry read-side, no reaper).
    // KT-189 — durable awareness acknowledgement: the bridge confirms the
    // model consumed a previously attached awareness batch. This is the ONLY
    // place the per-session awareness cursor advances; emission never moves
    // it, so a cancelled/crashed delivery is replayed, never skipped.
    // The value is CLAMPED to what one bounded batch from the current cursor
    // can legitimately cover: an oversized ack (buggy or hostile client) must
    // not skip turns that were never offered.
    if let (Some(session_pk), Some(ack)) = (caller_session_pk, q.ack_awareness_upto) {
        if let Err(e) = state
            .db
            .with_conn(move |conn| {
                // Scan / OFFER / ack: the ack is clamped to the persisted
                // offered cursor — what a wake response actually carried —
                // never to what would merely be offerable now. A client can
                // therefore never acknowledge (and skip) turns it was never
                // shown, whether the ack is buggy, hostile, or racing new
                // rows written between the offer and the ack.
                let offered =
                    crate::db::discussion_sessions::awareness_offered_upto(conn, session_pk)?;
                crate::db::discussion_sessions::advance_user_catchup_cursor(
                    conn,
                    session_pk,
                    ack.min(offered),
                )
            })
            .await
        {
            tracing::warn!("wait_for_peer: awareness ack failed: {e}");
        }
    }

    if let (Some(agent_type), Some(caller_session_id)) =
        (presence_agent.as_ref(), session_id.as_ref())
    {
        let disc_id_touch = disc_id.clone();
        let agent_touch = agent_type.clone();
        let sess_touch = caller_session_id.clone();
        let listening_ttl = timeout_secs as i64 + ACTIVITY_LISTENING_MARGIN_SECS;
        // KT-114 — a bridge that resolved its native resume id AFTER joining
        // delivers it on this idle loop. Same validation as the join-time
        // value; a malformed id is dropped, never guessed at.
        let late_conversation_id =
            normalize_conversation_id(q.conversation_id.as_deref()).unwrap_or(None);
        if let Err(e) = state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::touch_session(
                    conn,
                    &disc_id_touch,
                    &agent_touch,
                    &sess_touch,
                )?;
                if let Some(conversation_id) = late_conversation_id.as_deref() {
                    crate::db::discussion_sessions::set_live_session_conversation_id(
                        conn,
                        &disc_id_touch,
                        &agent_touch,
                        &sess_touch,
                        conversation_id,
                    )?;
                }
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    &disc_id_touch,
                    &agent_touch,
                    Some(&sess_touch),
                    "listening",
                    listening_ttl,
                )
            })
            .await
        {
            tracing::warn!("wait_for_peer: failed to bump heartbeat / set listening activity: {e}");
        }
    }

    let mut observed_latest_order = since;
    // Assigned on every poll before any read; no initial value to shadow.
    let mut withheld_by_routing: u32;
    // KT-189 — awareness turns beyond the per-wake cap, reported once.
    // Assigned on every poll before any read (same pattern as
    // `withheld_by_routing` above — an initial value would trip
    // `unused_assignments` under `-D warnings`).
    let mut awareness_omitted_total: u32;
    let mut awareness_delivered_upto: Option<i64>;
    loop {
        let disc_id_clone = disc_id.clone();
        let exclude_clone = exclude.clone();
        #[allow(clippy::type_complexity)]
        let messages: anyhow::Result<(Vec<WaitForPeerMessage>, i64, u32, u32, Option<i64>)> = state
            .db
            .with_conn(move |conn| {
                let observed_latest: i64 = conn.query_row(
                    "SELECT COALESCE(MAX(sort_order), ?2)
                     FROM (
                         SELECT sort_order FROM messages
                         WHERE discussion_id = ?1 AND sort_order > ?2
                         UNION ALL
                         SELECT sort_order FROM message_revision_events
                         WHERE discussion_id = ?1 AND sort_order > ?2
                     )",
                    rusqlite::params![&disc_id_clone, since],
                    |row| row.get(0),
                )?;
                // Pull every message after `since` ; filter the
                // exclude_agent_type in Rust to avoid threading an
                // Option<String> through the SQL binder.
                let mut stmt = conn.prepare(
                    "SELECT message_id, sort_order, role, agent_type, content, timestamp,
                            author_pseudo, event_type, target_message_id, target_agent,
                            targets, author_agent_type, author_cli_session_id,
                            native_fallback
                     FROM (
                         SELECT id AS message_id, sort_order, role, agent_type, content, timestamp,
                                author_pseudo, NULL AS event_type,
                                NULL AS target_message_id, target_agent,
                                (
                                    SELECT GROUP_CONCAT(
                                        ordered.target_kind || '|' ||
                                        ordered.agent_type || '|' ||
                                        COALESCE(ordered.cli_session_id, '') || '|' ||
                                        COALESCE(ordered.model_tier, ''),
                                        ','
                                    )
                                    FROM (
                                        SELECT mt.target_kind, mt.agent_type, mt.cli_session_id,
                                               mt.model_tier
                                        FROM message_targets mt
                                        WHERE mt.message_id = messages.id
                                        ORDER BY mt.position ASC
                                    ) AS ordered
                                ) AS targets,
                                (
                                    SELECT ds.agent_type
                                    FROM message_cli_authors mca
                                    JOIN discussion_sessions ds
                                      ON ds.id = mca.cli_session_id
                                     AND ds.disc_id = messages.discussion_id
                                    WHERE mca.message_id = messages.id
                                ) AS author_agent_type,
                                (
                                    SELECT mca.cli_session_id
                                    FROM message_cli_authors mca
                                    WHERE mca.message_id = messages.id
                                ) AS author_cli_session_id,
                                native_fallback
                         FROM messages
                         WHERE discussion_id = ?1
                           AND sort_order > ?2
                           AND channel = 'main'
                         UNION ALL
                         SELECT id AS message_id, sort_order, 'System' AS role, NULL AS agent_type,
                                '[message_revised] ' || target_message_id || char(10) || content,
                                created_at AS timestamp, NULL AS author_pseudo,
                                'message_revised' AS event_type, target_message_id,
                                trim(target_agent_json, '\"') AS target_agent,
                                (
                                    SELECT GROUP_CONCAT(
                                        ordered.target_kind || '|' ||
                                        ordered.agent_type || '|' ||
                                        COALESCE(ordered.cli_session_id, '') || '|' ||
                                        COALESCE(ordered.model_tier, ''),
                                        ','
                                    )
                                    FROM (
                                        SELECT mt.target_kind, mt.agent_type, mt.cli_session_id,
                                               mt.model_tier
                                        FROM message_targets mt
                                        WHERE mt.message_id = message_revision_events.target_message_id
                                        ORDER BY mt.position ASC
                                    ) AS ordered
                                ) AS targets,
                                NULL AS author_agent_type,
                                NULL AS author_cli_session_id,
                                0 AS native_fallback
                         FROM message_revision_events
                         WHERE discussion_id = ?1
                           AND sort_order > ?2
                           AND EXISTS (
                               SELECT 1
                               FROM messages revised_message
                               WHERE revised_message.id =
                                         message_revision_events.target_message_id
                                 AND revised_message.discussion_id = ?1
                                 AND revised_message.channel = 'main'
                           )
                     )
                     ORDER BY sort_order ASC",
                )?;
                let rows: Vec<WaitForPeerMessage> = stmt
                    .query_map(rusqlite::params![&disc_id_clone, since], |r| {
                        let targets = r
                            .get::<_, Option<String>>(10)?
                            .map(|serialized| {
                                serialized
                                    .split(',')
                                    .filter_map(|target| {
                                        let mut fields = target.splitn(4, '|');
                                        let kind = match fields.next()? {
                                            "discussion_agent" => MessageTargetKind::DiscussionAgent,
                                            "cli" => MessageTargetKind::Cli,
                                            _ => MessageTargetKind::Agent,
                                        };
                                        let agent_type = crate::db::discussions::parse_agent_type(
                                            fields.next()?,
                                        );
                                        let cli_session_id =
                                            fields.next().and_then(|value| value.parse().ok());
                                        let tier = match fields.next() {
                                            Some("economy") => Some(crate::models::ModelTier::Economy),
                                            Some("default") => Some(crate::models::ModelTier::Default),
                                            Some("reasoning") => Some(crate::models::ModelTier::Reasoning),
                                            _ => None,
                                        };
                                        Some(MessageTarget {
                                            kind,
                                            agent_type,
                                            cli_session_id,
                                            tier,
                                        })
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let author_agent_type = r.get::<_, Option<String>>(11)?;
                        let author_cli_session_id = r.get::<_, Option<i64>>(12)?;
                        let reply_target =
                            author_agent_type.zip(author_cli_session_id).map(
                                |(agent_type, cli_session_id)| {
                                    MessageTarget::cli(
                                        crate::db::discussions::parse_agent_type(&agent_type),
                                        cli_session_id,
                                    )
                                },
                            );
                        Ok(WaitForPeerMessage {
                            message_id: r.get(0)?,
                            sort_order: r.get(1)?,
                            role: r.get(2)?,
                            agent_type: r.get(3)?,
                            content: r.get(4)?,
                            timestamp: r.get(5)?,
                            author_pseudo: r.get(6)?,
                            event_type: r.get(7)?,
                            target_message_id: r.get(8)?,
                            target_agent: r.get(9)?,
                            target_agents: targets
                                .iter()
                                .map(|target| format!("{:?}", target.agent_type))
                                .collect(),
                            addressed_to_caller: caller_session_pk.is_some_and(|session_pk| {
                                targets.iter().any(|target| {
                                    target.kind == MessageTargetKind::Cli
                                        && target.cli_session_id == Some(session_pk)
                                })
                            }),
                            targets,
                            reply_target,
                            awareness: false,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let no_agent_room =
                    crate::db::discussions::disc_is_no_agent(conn, &disc_id_clone)?;
                let others: Vec<WaitForPeerMessage> = rows
                    .into_iter()
                    // Exclude only OUR OWN local appends (same agent_type AND no
                    // author_pseudo). A federated peer message carries an
                    // author_pseudo, so a same-agent_type peer (another
                    // ClaudeCode instance across the wire) is NOT filtered —
                    // otherwise two ClaudeCode peers go deaf to each other.
                    .filter(|message| {
                        if let Some(session_pk) = caller_session_pk {
                            // Exact modern callers exclude only the message
                            // written by their own durable CLI session. A
                            // sibling Codex CLI is a real peer even though its
                            // provider-level agent_type is identical.
                            return message
                                .reply_target
                                .as_ref()
                                .and_then(|target| target.cli_session_id)
                                != Some(session_pk);
                        }
                        // Legacy callers have no exact session identity, so
                        // retain the historical provider-level best effort.
                        match (&exclude_clone, &message.agent_type) {
                            (Some(ex), Some(agent)) if ex == agent => {
                                message.author_pseudo.is_some()
                            }
                            _ => true,
                        }
                    })
                    .collect();
                // KT-189 — WAKE classification. A modern caller (exact durable
                // session) is woken ONLY by a turn addressed to it, or by an
                // untargeted User turn when the room has no native agent (the
                // joined CLIs are then the designated responders). Everything
                // else — untargeted Agent traffic, turns addressed to another
                // responder — reaches the session as bounded AWARENESS context
                // attached to its next legitimate wake, never as a wake of its
                // own. Legacy callers without a session id keep the historical
                // room-visible projection.
                let peer_turns = others.len();
                let wake: Vec<WaitForPeerMessage> = others
                    .into_iter()
                    .filter(|message| {
                        caller_session_pk.is_none()
                            || message.addressed_to_caller
                            || (message.targets.is_empty()
                                && message.event_type.is_none()
                                && message.role == "User"
                                && no_agent_room)
                    })
                    .collect();
                let mut awareness_omitted: u32 = 0;
                let mut awareness_upto: Option<i64> = None;
                let mut merged = wake;
                // Awareness attaches only when something legitimately wakes
                // the model — an awareness-only window must not end the wait.
                // The scan starts at the durable per-session cursor, which
                // only `ack_awareness_upto` advances: an unconsumed batch is
                // re-attached to the next wake instead of being lost.
                if let Some(session_pk) = caller_session_pk {
                    if !merged.is_empty() {
                        let cursor = crate::db::discussion_sessions::user_catchup_cursor(
                            conn, session_pk,
                        )?;
                        let (awareness, omitted) = load_awareness_batch(
                            conn,
                            &disc_id_clone,
                            session_pk,
                            cursor,
                            observed_latest,
                            no_agent_room,
                        )?;
                        awareness_omitted = omitted;
                        awareness_upto = awareness.iter().map(|m| m.sort_order).max();
                        // Persist the OFFER (never the ack) at emission time:
                        // this is the ceiling a later ack may reach.
                        if let Some(upto) = awareness_upto {
                            crate::db::discussion_sessions::advance_awareness_offered_upto(
                                conn, session_pk, upto,
                            )?;
                        }
                        // Deliver awareness before the wake turns and in
                        // transcript order; skip any row already in the wake
                        // batch (an addressed turn is actionable, not context).
                        let wake_ids: std::collections::HashSet<String> =
                            merged.iter().map(|m| m.message_id.clone()).collect();
                        let mut combined: Vec<WaitForPeerMessage> = awareness
                            .into_iter()
                            .filter(|m| !wake_ids.contains(&m.message_id))
                            .collect();
                        combined.append(&mut merged);
                        combined.sort_by_key(|m| m.sort_order);
                        merged = combined;
                    }
                }
                // Counted AFTER the awareness merge: a turn attached to this
                // very response is delivered, not withheld. What remains are
                // window rows carried by neither wake nor awareness (e.g.
                // rows still gated by the awareness cap or ack cursor).
                let delivered_in_window =
                    merged.iter().filter(|m| m.sort_order > since).count();
                let withheld = peer_turns.saturating_sub(delivered_in_window) as u32;
                Ok((merged, observed_latest, withheld, awareness_omitted, awareness_upto))
            })
            .await;

        let messages = match messages {
            Ok((messages, observed_latest, withheld, omitted, upto)) => {
                observed_latest_order = observed_latest_order.max(observed_latest);
                // The query always re-reads from the same `since`, so the latest
                // count covers the whole window rather than one poll of it.
                withheld_by_routing = withheld;
                awareness_omitted_total = omitted;
                awareness_delivered_upto = upto;
                messages
            }
            Err(e) => return Json(ApiResponse::err(format!("wait_for_peer db error: {e}"))),
        };

        if !messages.is_empty() {
            let latest_sort_order = observed_latest_order;
            // 0.8.12 PR B — messages DELIVERED and no reply posted yet:
            // that's the "reading" fact (the window the human perceives
            // as "buggé/très long"). Short TTL; disc_append clears it the
            // instant the reply lands. An EMPTY timeout never sets this
            // (guard from the design debate: no fake "preparing" states).
            if let (Some(agent_type), Some(caller_session_id)) =
                (presence_agent.as_ref(), session_id.as_ref())
            {
                let disc_id_act = disc_id.clone();
                let agent_act = agent_type.clone();
                let sess_act = caller_session_id.clone();
                if let Err(e) = state
                    .db
                    .with_conn(move |conn| {
                        crate::db::discussion_sessions::set_session_activity(
                            conn,
                            &disc_id_act,
                            &agent_act,
                            Some(&sess_act),
                            "reading",
                            ACTIVITY_READING_TTL_SECS,
                        )
                    })
                    .await
                {
                    tracing::warn!("wait_for_peer: failed to set reading activity: {e}");
                }
            }
            let pacing = pacing_for_disc(&state, &disc_id).await;
            return Json(ApiResponse::ok(WaitForPeerResponse {
                timed_out: false,
                messages,
                latest_sort_order,
                pacing,
                // Delivery: the caller replies now, not after a pause.
                next_poll_at: None,
                withheld_by_routing,
                awareness_omitted: awareness_omitted_total,
                awareness_delivered_upto,
            }));
        }

        if std::time::Instant::now() >= deadline {
            let pacing = pacing_for_disc(&state, &disc_id).await;
            // Presence-gap fix: the caller will sleep `next_delay_seconds`
            // before polling again. Mark this session `waiting` for that
            // window + margin so the participants UI shows "dormant" instead
            // of "disconnected" during the pause — and hand the intended
            // next-poll instant back to the MCP caller for its scheduling.
            let next_poll_at = if let Some(ref agent_type) = presence_agent {
                let waiting_ttl = pacing.next_delay_seconds as i64 + ACTIVITY_WAITING_MARGIN_SECS;
                let next_poll_instant = chrono::Utc::now()
                    + chrono::Duration::seconds(pacing.next_delay_seconds as i64);
                if let Some(caller_session_id) = session_id.as_ref() {
                    let disc_id_w = disc_id.clone();
                    let agent_w = agent_type.clone();
                    let sess_w = caller_session_id.clone();
                    if let Err(e) = state
                        .db
                        .with_conn(move |conn| {
                            crate::db::discussion_sessions::set_session_activity(
                                conn,
                                &disc_id_w,
                                &agent_w,
                                Some(&sess_w),
                                "waiting",
                                waiting_ttl,
                            )?;
                            // 0.9.2-G: persist the next-poll instant so the
                            // participants surface tells `dormant` (poll still
                            // due) from `offline` (blew past the deadline).
                            crate::db::discussion_sessions::set_next_poll_at(
                                conn,
                                &disc_id_w,
                                &agent_w,
                                Some(&sess_w),
                                next_poll_instant,
                            )
                        })
                        .await
                    {
                        tracing::warn!(
                            "wait_for_peer: failed to set waiting activity / next_poll_at: {e}"
                        );
                    }
                }
                Some(next_poll_instant.to_rfc3339())
            } else {
                None
            };
            return Json(ApiResponse::ok(WaitForPeerResponse {
                timed_out: true,
                messages: vec![],
                latest_sort_order: observed_latest_order,
                pacing,
                next_poll_at,
                withheld_by_routing,
                // A quiet return carries no awareness by design: attaching it
                // here would make the bridge wake the model for context alone.
                awareness_omitted: 0,
                awareness_delivered_upto: None,
            }));
        }

        sleep(Duration::from_millis(WAIT_POLL_INTERVAL_MS)).await;
    }
}

/// KT-189 — load the AWARENESS backlog of one session: main-channel turns in
/// `(cursor, upto]` that did not and will not wake this CLI — untargeted room
/// traffic and turns addressed to another responder — excluding the session's
/// own appends and its wake classes. Oldest first, bounded to
/// [`AWARENESS_MAX_MESSAGES`]; the second value counts the rows left unacked
/// beyond the cap (they return with the next wake).
fn load_awareness_batch(
    conn: &rusqlite::Connection,
    disc_id: &str,
    session_pk: i64,
    cursor: i64,
    upto: i64,
    no_agent_room: bool,
) -> anyhow::Result<(Vec<WaitForPeerMessage>, u32)> {
    let mut stmt = conn.prepare(
        "SELECT message_id, sort_order, role, agent_type, content, timestamp,
                author_pseudo, event_type, target_message_id, target_agent,
                targets, author_cli_session_id
         FROM (
             SELECT id AS message_id, sort_order, role, agent_type, content,
                    timestamp, author_pseudo, NULL AS event_type,
                    NULL AS target_message_id, target_agent,
                    (
                        SELECT GROUP_CONCAT(
                            ordered.target_kind || '|' ||
                            ordered.agent_type || '|' ||
                            COALESCE(ordered.cli_session_id, '') || '|' ||
                            COALESCE(ordered.model_tier, ''),
                            ','
                        )
                        FROM (
                            SELECT mt.target_kind, mt.agent_type, mt.cli_session_id,
                                   mt.model_tier
                            FROM message_targets mt
                            WHERE mt.message_id = messages.id
                            ORDER BY mt.position ASC
                        ) AS ordered
                    ) AS targets,
                    (
                        SELECT mca.cli_session_id
                        FROM message_cli_authors mca
                        WHERE mca.message_id = messages.id
                    ) AS author_cli_session_id
             FROM messages
             WHERE discussion_id = ?1
               AND sort_order > ?2
               AND sort_order <= ?3
               AND channel = 'main'
             UNION ALL
             SELECT id AS message_id, sort_order, 'System' AS role,
                    NULL AS agent_type,
                    '[message_revised] ' || target_message_id || char(10) || content,
                    created_at AS timestamp, NULL AS author_pseudo,
                    'message_revised' AS event_type, target_message_id,
                    trim(target_agent_json, '\"') AS target_agent,
                    (
                        SELECT GROUP_CONCAT(
                            ordered.target_kind || '|' ||
                            ordered.agent_type || '|' ||
                            COALESCE(ordered.cli_session_id, '') || '|' ||
                            COALESCE(ordered.model_tier, ''),
                            ','
                        )
                        FROM (
                            SELECT mt.target_kind, mt.agent_type, mt.cli_session_id,
                                   mt.model_tier
                            FROM message_targets mt
                            WHERE mt.message_id = message_revision_events.target_message_id
                            ORDER BY mt.position ASC
                        ) AS ordered
                    ) AS targets,
                    NULL AS author_cli_session_id
             FROM message_revision_events
             WHERE discussion_id = ?1
               AND sort_order > ?2
               AND sort_order <= ?3
               AND EXISTS (
                   SELECT 1 FROM messages revised_message
                   WHERE revised_message.id =
                             message_revision_events.target_message_id
                     AND revised_message.discussion_id = ?1
                     AND revised_message.channel = 'main'
               )
         )
         ORDER BY sort_order ASC",
    )?;
    let rows: Vec<(WaitForPeerMessage, Option<i64>)> = stmt
        .query_map(rusqlite::params![disc_id, cursor, upto], |r| {
            let targets = r
                .get::<_, Option<String>>(10)?
                .map(|serialized| {
                    serialized
                        .split(',')
                        .filter_map(|target| {
                            let mut fields = target.splitn(4, '|');
                            let kind = match fields.next()? {
                                "discussion_agent" => MessageTargetKind::DiscussionAgent,
                                "cli" => MessageTargetKind::Cli,
                                _ => MessageTargetKind::Agent,
                            };
                            let agent_type =
                                crate::db::discussions::parse_agent_type(fields.next()?);
                            let cli_session_id = fields.next().and_then(|value| value.parse().ok());
                            let tier = match fields.next() {
                                Some("economy") => Some(crate::models::ModelTier::Economy),
                                Some("default") => Some(crate::models::ModelTier::Default),
                                Some("reasoning") => Some(crate::models::ModelTier::Reasoning),
                                _ => None,
                            };
                            Some(MessageTarget {
                                kind,
                                agent_type,
                                cli_session_id,
                                tier,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let author_cli_session_id = r.get::<_, Option<i64>>(11)?;
            let addressed_to_caller = targets.iter().any(|target| {
                target.kind == MessageTargetKind::Cli && target.cli_session_id == Some(session_pk)
            });
            let message = WaitForPeerMessage {
                message_id: r.get(0)?,
                sort_order: r.get(1)?,
                role: r.get(2)?,
                agent_type: r.get(3)?,
                content: r.get(4)?,
                timestamp: r.get(5)?,
                author_pseudo: r.get(6)?,
                event_type: r.get(7)?,
                target_message_id: r.get(8)?,
                target_agent: r.get(9)?,
                target_agents: targets
                    .iter()
                    .map(|target| format!("{:?}", target.agent_type))
                    .collect(),
                addressed_to_caller,
                targets,
                reply_target: None,
                awareness: true,
            };
            Ok((message, author_cli_session_id))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut batch: Vec<WaitForPeerMessage> = Vec::new();
    let mut omitted: u32 = 0;
    for (message, author_cli_session_id) in rows {
        // Wake classes and the session's own appends are not awareness.
        if author_cli_session_id == Some(session_pk)
            || message.addressed_to_caller
            || (message.targets.is_empty() && message.role == "User" && no_agent_room)
        {
            continue;
        }
        if batch.len() < AWARENESS_MAX_MESSAGES {
            batch.push(message);
        } else {
            omitted += 1;
        }
    }
    Ok((batch, omitted))
}

/// `GET /api/discussions/:id/participants`
///
/// Returns the active+paused participants of a disc — what the
/// header renders as small agent icons next to the disc title.
/// `left` sessions are excluded (audit history only).
pub async fn list_participants(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
) -> Json<ApiResponse<Vec<db::discussion_sessions::ParticipantView>>> {
    let res = state
        .db
        .with_conn(move |conn| db::discussion_sessions::list_participant_views(conn, &disc_id))
        .await;
    match res {
        Ok(list) => Json(ApiResponse::ok(list)),
        Err(e) => Json(ApiResponse::err(e.to_string())),
    }
}

/// `POST /api/discussions/:id/invite-peer`
///
/// No request body — the disc is already addressed by the URL, the
/// caller is implicitly the human owner. Returns the plain token
/// (only place it ever appears outside the agent's tool-call wire).
pub async fn invite_peer(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
) -> Json<ApiResponse<InviteResponse>> {
    // All DB work in one closure so we hold the mutex once and the
    // blocking work happens off the Tokio worker thread.
    let disc_id_for_db = disc_id.clone();
    let issued = state
        .db
        .with_conn(move |conn| {
            // Defensive : refuse to mint a token for a non-existent disc.
            // The FK on `discussion_invite_tokens.disc_id` would catch
            // it on INSERT, but we'd rather return a clean 4xx-like
            // error envelope than surface a raw FK violation.
            let exists: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM discussions WHERE id = ?1",
                    rusqlite::params![&disc_id_for_db],
                    |r| r.get::<_, i64>(0),
                )
                .ok();
            if exists.is_none() {
                return Err(anyhow::anyhow!("discussion `{disc_id_for_db}` not found"));
            }
            db::discussion_sessions::create_invite_token(conn, &disc_id_for_db)
        })
        .await;

    let issued = match issued {
        Ok(i) => i,
        Err(e) => return Json(ApiResponse::err(e.to_string())),
    };

    let instruction_text = invite_handoff(&issued.token);
    let instruction_text_minimal = invite_handoff_minimal(&issued.token);

    Json(ApiResponse::ok(InviteResponse {
        token: issued.token,
        disc_id: issued.disc_id,
        expires_at: issued.expires_at,
        ttl_seconds: db::discussion_sessions::INVITE_TTL_SECS,
        instruction_text,
        instruction_text_minimal,
    }))
}

/// Bare form: the call and nothing else, for a human who only wants the token.
fn invite_handoff_minimal(token: &str) -> String {
    format!("Joins-toi à cette discussion Kronn en appelant l'outil MCP : disc_join({{token: \"{token}\"}})")
}

/// KT-52 — the pasted handoff. The bare `disc_join` line left an invited agent
/// with no idea that the room has a shared plan it should read, that it may
/// write to that plan, or that it is expected to stay and follow the
/// conversation; `disc_join`'s own protocol says all that, but only AFTER the
/// agent decides to call it, and an agent that skims does the wrong thing
/// first. Deliberately short — it lands in a terminal prompt — and it POINTS at
/// the tools instead of inlining the conversation or the plan, so the context
/// is loaded on demand rather than duplicated into the prompt.
fn invite_handoff(token: &str) -> String {
    format!(
        "Joins-toi à cette discussion Kronn en appelant l'outil MCP : \
         disc_join({{token: \"{token}\"}})\n\
         Ensuite, avant d'agir : lis le plan partagé avec `plan_get` (objectif + \
         tâches en cours) et `task_list` si tu as besoin du reste du backlog — \
         ne demande pas qu'on te réexplique l'état, charge-le.\n\
         Tu peux créer, modifier, prioriser et cocher ces tâches \
         (`task_create`, `task_update`, `task_update_dod`, `task_add_blocker`), \
         en utilisant les outils `kronn-internal` autorisés.\n\
         Avant ta première action substantielle, annonce via `disc_append` la \
         tâche, le périmètre et la prochaine action dans la room.\n\
         Puis reste dans la room : boucle sur `disc_wait_for_peer` et réponds à \
         ce qui arrive. Si c'est calme, prends la tâche suivante du plan et \
         rends-en compte — te taire est lu comme un départ."
    )
}

/// Body of `POST /api/disc/claim-by-token`. A PEER calls this to ask "do you
/// host the room behind this invite code?". Authenticated by `from_invite_code`
/// matching one of our contacts — the same self-auth credential as the WS
/// Presence handshake (so this endpoint is exempt from the bearer middleware).
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct ClaimByTokenRequest {
    pub token: String,
    /// The CALLING peer's own invite code — must match a known contact here.
    pub from_invite_code: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ClaimByTokenResponse {
    /// True iff WE host the room behind `token`; then we've shared it back.
    pub found: bool,
    pub shared_id: Option<String>,
    pub title: Option<String>,
}

/// `POST /api/disc/claim-by-token` — the cross-instance leg of the unified
/// "join by code". When `disc_join(code)` misses locally, the caller asks each
/// of its contacts here. If WE host the room, we share it back with the calling
/// contact (broadcast a `DiscussionInvite` over the WS federation so the caller
/// mirrors it). This collapses the two former mechanisms (local token-join vs
/// contact-share) into a single paste-a-code action that works wherever the
/// room actually lives.
///
/// Auth: the caller proves it's a known contact via `from_invite_code` (same
/// credential as the WS Presence frame). Registered as auth-exempt in `lib.rs`
/// (a remote peer has no bearer token), gated here instead.
pub async fn claim_by_token(
    State(state): State<AppState>,
    Json(req): Json<ClaimByTokenRequest>,
) -> Json<ApiResponse<ClaimByTokenResponse>> {
    let from_code = req.from_invite_code.trim().to_string();
    if from_code.is_empty() {
        return Json(ApiResponse::err("from_invite_code required"));
    }

    // 1. Authenticate the caller: the invite code must match a known contact
    //    (same trust model as the WS Presence handshake — no anonymous claims).
    let code_lookup = from_code.clone();
    let caller = match state
        .db
        .with_conn(move |conn| crate::db::contacts::authenticate_invite_code(conn, &code_lookup))
        .await
    {
        Ok(crate::db::contacts::InviteAuth::Accepted(c)) => c,
        // Pending/refused answers EXACTLY like unknown — no status oracle.
        Ok(crate::db::contacts::InviteAuth::NotAccepted { pseudo, status }) => {
            tracing::warn!(
                target: "kronn::invariant",
                caller = %pseudo, status = %status, route = "claim-by-token",
                "invite-code auth refused — contact is not accepted"
            );
            return Json(ApiResponse::err(
                "unknown peer (invite code not in contacts)",
            ));
        }
        Ok(crate::db::contacts::InviteAuth::Unknown) => {
            return Json(ApiResponse::err(
                "unknown peer (invite code not in contacts)",
            ))
        }
        Err(e) => return Json(ApiResponse::err(format!("contact lookup error: {e}"))),
    };

    // 2. Resolve the token → a LOCAL disc we host (read-only, no consume).
    let token = req.token.clone();
    let disc_id = match state
        .db
        .with_conn(move |conn| crate::db::discussion_sessions::resolve_token_disc(conn, &token))
        .await
    {
        Ok(Some(id)) => id,
        Ok(None) => {
            // We don't host this room — caller will try the next contact.
            return Json(ApiResponse::ok(ClaimByTokenResponse {
                found: false,
                shared_id: None,
                title: None,
            }));
        }
        Err(e) => return Json(ApiResponse::err(format!("token resolve error: {e}"))),
    };

    // 3. Share that disc with the calling contact (idempotent), exactly like the
    //    `share` handler — set/keep shared_id, append the contact, persist.
    let cid = caller.id.clone();
    let did = disc_id.clone();
    let shared = state
        .db
        .with_conn(move |conn| {
            let disc = crate::db::discussions::get_discussion(conn, &did)?
                .ok_or_else(|| anyhow::anyhow!("discussion vanished"))?;
            let shared_id = disc
                .shared_id
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let mut shared_with = disc.shared_with;
            if !shared_with.contains(&cid) {
                shared_with.push(cid.clone());
            }
            crate::db::discussions::update_discussion_sharing(
                conn,
                &did,
                &shared_id,
                &shared_with,
            )?;
            Ok::<_, anyhow::Error>((shared_id, disc.title))
        })
        .await;
    let (shared_id, title) = match shared {
        Ok(t) => t,
        Err(e) => return Json(ApiResponse::err(format!("share error: {e}"))),
    };

    // 4. Broadcast the invite → ws_client relays it to the caller → mirrors there.
    let config = state.config.read().await;
    let from_pseudo = crate::api::contacts::invite_pseudo(&config.server);
    let our_invite_code = crate::api::contacts::build_invite_code(&config.server).await;
    drop(config);
    let _ = state
        .ws_broadcast
        .send(crate::models::WsMessage::DiscussionInvite {
            shared_discussion_id: shared_id.clone(),
            title: title.clone(),
            from_pseudo,
            from_invite_code: our_invite_code,
        });

    tracing::info!(
        "claim-by-token: shared disc {} (shared_id {}) back to contact {}",
        disc_id,
        shared_id,
        caller.pseudo
    );
    Json(ApiResponse::ok(ClaimByTokenResponse {
        found: true,
        shared_id: Some(shared_id),
        title: Some(title),
    }))
}

/// Body of `POST /api/disc/fetch-file` (F8). A peer that received a
/// `FileAttached` announcement calls this to pull the binary of a context file
/// it doesn't have. Authenticated by `from_invite_code` matching a contact
/// (same trust model as `claim-by-token`).
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct FetchFileRequest {
    pub file_id: String,
    pub from_invite_code: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct FetchFileResponse {
    pub found: bool,
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    /// Base64-encoded file bytes (None when not found). Base64 keeps the
    /// transport a simple JSON envelope; the peer decodes + writes to disk.
    pub data_base64: Option<String>,
}

/// `POST /api/disc/fetch-file` — the binary-transfer leg of F8 (P2P file/doc
/// recovery). Returns the bytes of a context file we host so a peer can mirror
/// it locally. Auth-exempt in `lib.rs` (a remote peer has no bearer token),
/// gated here on a known `from_invite_code` — same as `claim-by-token`.
pub async fn fetch_file(
    State(state): State<AppState>,
    Json(req): Json<FetchFileRequest>,
) -> Json<ApiResponse<FetchFileResponse>> {
    let from_code = req.from_invite_code.trim().to_string();
    if from_code.is_empty() {
        return Json(ApiResponse::err("from_invite_code required"));
    }
    // Authenticate the caller (must be a known contact) — and KEEP its id:
    // being a contact is not enough to read arbitrary files (see below).
    let caller = match state
        .db
        .with_conn(move |conn| crate::db::contacts::authenticate_invite_code(conn, &from_code))
        .await
    {
        Ok(crate::db::contacts::InviteAuth::Accepted(c)) => c,
        // Pending/refused answers EXACTLY like unknown — no status oracle.
        Ok(crate::db::contacts::InviteAuth::NotAccepted { pseudo, status }) => {
            tracing::warn!(
                target: "kronn::invariant",
                caller = %pseudo, status = %status, route = "fetch-file",
                "invite-code auth refused — contact is not accepted"
            );
            return Json(ApiResponse::err(
                "unknown peer (invite code not in contacts)",
            ));
        }
        Ok(crate::db::contacts::InviteAuth::Unknown) => {
            return Json(ApiResponse::err(
                "unknown peer (invite code not in contacts)",
            ))
        }
        Err(e) => return Json(ApiResponse::err(format!("contact lookup error: {e}"))),
    };

    let file_id = req.file_id.clone();
    let cf = match state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::get_context_file(conn, &file_id).map_err(|e| anyhow::anyhow!(e))
        })
        .await
    {
        Ok(Some(cf)) => cf,
        Ok(None) => {
            return Json(ApiResponse::ok(FetchFileResponse {
                found: false,
                filename: None,
                mime_type: None,
                data_base64: None,
            }))
        }
        Err(e) => return Json(ApiResponse::err(format!("file lookup error: {e}"))),
    };

    // Scope check: the file's discussion must be SHARED WITH this contact.
    // Same `found: false` shape as a missing file — no existence oracle.
    let disc_id = cf.discussion_id.clone();
    let shared_with_caller = match state
        .db
        .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &disc_id))
        .await
    {
        Ok(Some(d)) => d.shared_with.contains(&caller.id),
        Ok(None) => false,
        Err(e) => return Json(ApiResponse::err(format!("discussion lookup error: {e}"))),
    };
    if !shared_with_caller {
        tracing::warn!(
            target: "kronn::invariant",
            caller = %caller.pseudo, file_id = %cf.id, disc_id = %cf.discussion_id,
            "fetch-file refused — discussion not shared with the caller"
        );
        return Json(ApiResponse::ok(FetchFileResponse {
            found: false,
            filename: None,
            mime_type: None,
            data_base64: None,
        }));
    }

    let Some(disk_path) = cf.disk_path.clone() else {
        // Text-only context file (no binary on disk) — nothing to transfer.
        return Json(ApiResponse::ok(FetchFileResponse {
            found: false,
            filename: None,
            mime_type: None,
            data_base64: None,
        }));
    };
    let bytes = match tokio::fs::read(&disk_path).await {
        Ok(b) => b,
        Err(e) => return Json(ApiResponse::err(format!("read error: {e}"))),
    };
    let data_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Json(ApiResponse::ok(FetchFileResponse {
        found: true,
        filename: Some(cf.filename),
        mime_type: Some(cf.mime_type),
        data_base64: Some(data_base64),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::default_config;
    use crate::db::Database;
    use crate::DEFAULT_MAX_CONCURRENT_AGENTS;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    /// In-memory state suitable for the route-layer tests. We don't
    /// spin up axum here — `invite_peer` is a free function over
    /// `State<AppState>` + `Path<String>`, so we exercise the logic
    /// directly. This keeps the test fast and avoids the integration
    /// dance (no tokio runtime needed past the `async fn` itself).
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

    #[tokio::test]
    async fn fetch_file_is_scoped_to_discussions_shared_with_the_caller() {
        // Regression (Codex audit 2026-07-12): any accepted contact could
        // read ANY context file by id.
        let state = make_state_with_disc("d-fetch-1").await;
        let tmp = tempfile::TempDir::new().unwrap();
        let blob = tmp.path().join("doc.pdf");
        std::fs::write(&blob, b"BYTES").unwrap();
        let blob_str = blob.to_string_lossy().to_string();
        state
            .db
            .with_conn(move |conn| {
                crate::db::contacts::insert_contact(
                    conn,
                    &crate::models::Contact {
                        id: "c-1".into(),
                        pseudo: "peer".into(),
                        avatar_email: None,
                        kronn_url: "http://peer.local".into(),
                        invite_code: "kr-inv-abc".into(),
                        status: "accepted".into(),
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                    },
                )?;
                crate::db::discussions::insert_federated_context_file(
                    conn,
                    "f-1",
                    "d-fetch-1",
                    "m-1",
                    "doc.pdf",
                    "application/pdf",
                    5,
                    &blob_str,
                )
                .map_err(|e| anyhow::anyhow!(e))?;
                Ok(())
            })
            .await
            .unwrap();

        // Known contact, but the discussion is NOT shared with it → found: false.
        let resp = fetch_file(
            State(state.clone()),
            Json(FetchFileRequest {
                file_id: "f-1".into(),
                from_invite_code: "kr-inv-abc".into(),
            }),
        )
        .await;
        let body = resp.0.data.expect("ok envelope (no existence oracle)");
        assert!(!body.found, "unshared discussion must not leak files");
        assert!(body.data_base64.is_none());

        // Share the discussion with that contact → the bytes flow.
        state
            .db
            .with_conn(|conn| {
                crate::db::discussions::update_discussion_sharing(
                    conn,
                    "d-fetch-1",
                    "sh-1",
                    &["c-1".to_string()],
                )
                .map(|_| ())
            })
            .await
            .unwrap();
        let resp = fetch_file(
            State(state.clone()),
            Json(FetchFileRequest {
                file_id: "f-1".into(),
                from_invite_code: "kr-inv-abc".into(),
            }),
        )
        .await;
        let body = resp.0.data.unwrap();
        assert!(body.found);
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(body.data_base64.unwrap())
            .unwrap();
        assert_eq!(bytes, b"BYTES");
    }

    #[tokio::test]
    async fn non_accepted_contacts_are_refused_like_unknown_codes() {
        // Passe D (Codex constat n°1) — a pending/refused contact keeps its
        // invite code but must NOT pass the auth-exempt P2P routes, and the
        // refusal must be indistinguishable from an unknown code (no oracle).
        let state = make_state_with_disc("d-auth-1").await;
        state
            .db
            .with_conn(|conn| {
                for (id, code, status) in [
                    ("c-pend", "kr-inv-pend", "pending"),
                    ("c-ref", "kr-inv-ref", "refused"),
                ] {
                    crate::db::contacts::insert_contact(
                        conn,
                        &crate::models::Contact {
                            id: id.into(),
                            pseudo: format!("peer-{status}"),
                            avatar_email: None,
                            kronn_url: "http://peer.local".into(),
                            invite_code: code.into(),
                            status: status.into(),
                            created_at: chrono::Utc::now(),
                            updated_at: chrono::Utc::now(),
                        },
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();

        for code in ["kr-inv-pend", "kr-inv-ref", "kr-inv-ghost"] {
            // fetch-file: same error string for pending, refused and unknown.
            let resp = fetch_file(
                State(state.clone()),
                Json(FetchFileRequest {
                    file_id: "f-any".into(),
                    from_invite_code: code.into(),
                }),
            )
            .await;
            assert_eq!(
                resp.0.error.as_deref(),
                Some("unknown peer (invite code not in contacts)"),
                "{code} must be refused with the unknown-code message"
            );

            // claim-by-token: same contract.
            let resp = claim_by_token(
                State(state.clone()),
                Json(ClaimByTokenRequest {
                    token: "kr-join-whatever".into(),
                    from_invite_code: code.into(),
                }),
            )
            .await;
            assert_eq!(
                resp.0.error.as_deref(),
                Some("unknown peer (invite code not in contacts)"),
                "{code} must be refused with the unknown-code message"
            );
        }
    }

    #[tokio::test]
    async fn invite_peer_returns_plain_token_for_existing_disc() {
        let state = make_state_with_disc("d-invite-1").await;
        let resp = invite_peer(State(state), Path("d-invite-1".to_string())).await;
        let body = resp.0;
        assert!(body.success, "got error: {:?}", body.error);
        let data = body.data.expect("data present on success");
        assert!(data.token.starts_with("kr-join-"));
        assert_eq!(data.disc_id, "d-invite-1");
        assert_eq!(data.ttl_seconds, db::discussion_sessions::INVITE_TTL_SECS);
        assert!(data.instruction_text.contains(&data.token));
        assert!(data.instruction_text.contains("disc_join"));
        // KT-52 — both forms carry the token; only the enriched one carries the
        // working contract.
        assert!(data.instruction_text_minimal.contains(&data.token));
        assert!(data.instruction_text_minimal.contains("disc_join"));
        assert!(
            !data.instruction_text_minimal.contains("plan_get"),
            "the minimal form must stay a bare call"
        );
    }

    /// KT-52 — the pasted line is read BEFORE `disc_join` answers, so an agent
    /// that only skims it must still know to read the plan, that it may write
    /// to it, and that it has to stay. Asserted here because prose without a
    /// test gets trimmed by the next person who finds it too long.
    #[test]
    fn pasted_handoff_carries_the_working_contract() {
        let handoff = invite_handoff("kr-join-abc");

        assert!(handoff.contains("kr-join-abc"));
        assert!(handoff.contains("disc_join"));
        assert!(
            handoff.contains("plan_get"),
            "must send the agent to the plan"
        );
        assert!(handoff.contains("task_list"));
        for tool in [
            "task_create",
            "task_update",
            "task_update_dod",
            "task_add_blocker",
        ] {
            assert!(handoff.contains(tool), "must state that {tool} is allowed");
        }
        assert!(handoff.contains("kronn-internal"));
        assert!(
            handoff.contains("Avant ta première action substantielle")
                && handoff.contains("périmètre")
                && handoff.contains("disc_append"),
            "must make the agent announce its intent before acting",
        );
        assert!(handoff.contains("disc_wait_for_peer"), "must say to stay");

        // Short enough to paste into a prompt: the whole point is that it is a
        // handoff, not a copy of the conversation.
        assert!(
            handoff.lines().count() <= 5,
            "handoff grew to {} lines — keep it pasteable",
            handoff.lines().count()
        );
    }

    #[tokio::test]
    async fn invite_peer_rejects_unknown_disc_with_clear_error() {
        let state = make_state_with_disc("d-real").await;
        let resp = invite_peer(State(state), Path("d-ghost".to_string())).await;
        let body = resp.0;
        assert!(!body.success);
        let err = body.error.expect("error message on failure");
        assert!(err.contains("d-ghost"), "got: {err}");
        assert!(err.contains("not found"));
    }

    // ─── peer_join companion endpoint ───────────────────────────

    #[tokio::test]
    async fn peer_join_binds_session_and_returns_disc_meta() {
        let state = make_state_with_disc("d-join-1").await;
        // Mint an invite token via the regular endpoint first — full
        // round-trip from invite to join, no DB shortcuts.
        let invite_resp = invite_peer(State(state.clone()), Path("d-join-1".to_string())).await;
        let token = invite_resp.0.data.unwrap().token;

        let join_resp = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token,
                agent_type: "Codex".into(),
                session_id: "sess-cdx-1".into(),
                model: None,
                conversation_id: Some("019f8fc7-dd84-7843-abad-162a97ca836b".into()),
            }),
        )
        .await;
        let body = join_resp.0;
        assert!(body.success, "got error: {:?}", body.error);
        let data = body.data.unwrap();
        assert_eq!(data.disc_id, "d-join-1");
        assert!(data.session_pk > 0);
        assert!(data.resume_token.starts_with("kr-resume-"));
        assert_eq!(data.peer_count, 1, "exactly the joining session is active");
        assert_eq!(data.disc_title, "Test disc");
        assert_eq!(data.recent_messages.len(), 0, "empty disc → no previews");
        let sessions = state
            .db
            .with_conn(|conn| db::discussion_sessions::list_sessions(conn, "d-join-1", false))
            .await
            .unwrap();
        assert_eq!(
            sessions[0].conversation_id.as_deref(),
            Some("019f8fc7-dd84-7843-abad-162a97ca836b")
        );
        // Empty disc ⇒ cold at the cap (no anchors). The HOT proof that
        // pacing is really computed lives in the dedicated test below.
        assert_eq!(
            data.pacing.regime,
            crate::api::disc_introspection::PacingRegime::Cold
        );
        assert_eq!(
            data.pacing.next_delay_seconds,
            data.poll_policy.max_delay_seconds
        );
    }

    #[tokio::test]
    async fn peer_join_reports_note_count_without_including_note_bodies() {
        let state = make_state_with_disc("d-join-notes").await;
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute_batch(&format!(
                    "INSERT INTO messages
                         (id, discussion_id, role, channel, content, timestamp, sort_order)
                     VALUES
                         ('join-main', 'd-join-notes', 'User', 'main', 'visible turn', '{now}', 1),
                         ('join-note', 'd-join-notes', 'User', 'note', 'private note body', '{now}', 2);"
                ))?;
                Ok(())
            })
            .await
            .unwrap();
        let token = invite_peer(State(state.clone()), Path("d-join-notes".into()))
            .await
            .0
            .data
            .unwrap()
            .token;

        let data = peer_join(
            State(state),
            Json(PeerJoinRequest {
                token,
                agent_type: "Codex".into(),
                session_id: "sess-notes".into(),
                model: None,
                conversation_id: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();

        assert_eq!(data.note_count, 1);
        assert_eq!(data.recent_messages.len(), 1);
        assert_eq!(data.recent_messages[0].preview, "visible turn");
    }

    #[tokio::test]
    async fn peer_resume_rotates_credential_and_keeps_one_participant_row() {
        let state = make_state_with_disc("d-resume-1").await;
        let token = invite_peer(State(state.clone()), Path("d-resume-1".to_string()))
            .await
            .0
            .data
            .unwrap()
            .token;
        let joined = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token,
                agent_type: "Codex".into(),
                session_id: "child-before-reload".into(),
                model: None,
                conversation_id: Some("019f8fc7-dd84-7843-abad-162a97ca836b".into()),
            }),
        )
        .await
        .0
        .data
        .unwrap();

        let successor = "kr-resume-11111111111111111111111111111111".to_string();
        let resumed = peer_resume(
            State(state.clone()),
            Json(PeerResumeRequest {
                agent_type: "Codex".into(),
                session_id: "child-after-reload".into(),
                resume_token: joined.resume_token.clone(),
                conversation_id: Some("019f8fc7-dd84-7843-abad-162a97ca836b".into()),
                next_resume_token: Some(successor.clone()),
            }),
        )
        .await
        .0
        .data
        .expect("resume succeeds");
        assert_eq!(resumed.disc_id, "d-resume-1");
        assert_eq!(resumed.session_pk, joined.session_pk);
        assert_eq!(resumed.resume_token, successor);

        let exact_replay = peer_resume(
            State(state.clone()),
            Json(PeerResumeRequest {
                agent_type: "Codex".into(),
                session_id: "child-after-response-loss".into(),
                resume_token: joined.resume_token.clone(),
                conversation_id: Some("019f8fc7-dd84-7843-abad-162a97ca836b".into()),
                next_resume_token: Some(successor),
            }),
        )
        .await
        .0
        .data
        .expect("the exact prepared successor is an idempotent replay");
        assert_eq!(exact_replay.session_pk, joined.session_pk);

        let replay = peer_resume(
            State(state.clone()),
            Json(PeerResumeRequest {
                agent_type: "Codex".into(),
                session_id: "replay".into(),
                resume_token: joined.resume_token,
                conversation_id: None,
                next_resume_token: Some("kr-resume-22222222222222222222222222222222".into()),
            }),
        )
        .await
        .0;
        assert!(!replay.success);
        assert!(replay.error.unwrap().contains("invalid, replayed"));

        let rows = state
            .db
            .with_conn(|conn| db::discussion_sessions::list_sessions(conn, "d-resume-1", false))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].session_id.as_deref(),
            Some("child-after-response-loss")
        );
        assert_eq!(
            rows[0].conversation_id.as_deref(),
            Some("019f8fc7-dd84-7843-abad-162a97ca836b"),
            "resume refreshes native CLI metadata without replacing the bridge id"
        );
    }

    #[tokio::test]
    async fn peer_resume_rejects_malformed_successor_without_consuming_current() {
        let state = make_state_with_disc("d-resume-invalid-next").await;
        let token = invite_peer(
            State(state.clone()),
            Path("d-resume-invalid-next".to_string()),
        )
        .await
        .0
        .data
        .unwrap()
        .token;
        let joined = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token,
                agent_type: "Codex".into(),
                session_id: "before".into(),
                model: None,
                conversation_id: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();

        let invalid = peer_resume(
            State(state.clone()),
            Json(PeerResumeRequest {
                agent_type: "Codex".into(),
                session_id: "invalid".into(),
                resume_token: joined.resume_token.clone(),
                conversation_id: None,
                next_resume_token: Some("kr-resume-not-hex".into()),
            }),
        )
        .await
        .0;
        assert!(!invalid.success);
        assert!(invalid.error.unwrap().contains("32 hex"));

        let valid = peer_resume(
            State(state),
            Json(PeerResumeRequest {
                agent_type: "Codex".into(),
                session_id: "valid".into(),
                resume_token: joined.resume_token,
                conversation_id: None,
                next_resume_token: Some("kr-resume-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
            }),
        )
        .await
        .0;
        assert!(
            valid.success,
            "validation failure must not consume current token"
        );
    }

    #[tokio::test]
    async fn peer_join_returns_the_server_computed_pacing() {
        // Copilot review (PR 118): join was the one response missing
        // `pacing`. A fresh User message puts the disc in the HOT regime —
        // on an empty disc the real computation and the cold-cap Default
        // placeholder coincide, so hot is the only observable proof the
        // handler actually ran `pacing_for_disc`.
        let state = make_state_with_disc("d-join-pace").await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO messages
                        (id, discussion_id, role, content, agent_type, timestamp, sort_order)
                     VALUES ('m-u1', 'd-join-pace', 'User', 'ping', NULL, ?1, 1)",
                    rusqlite::params![chrono::Utc::now().to_rfc3339()],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let invite_resp = invite_peer(State(state.clone()), Path("d-join-pace".to_string())).await;
        let token = invite_resp.0.data.unwrap().token;
        let join_resp = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token,
                agent_type: "Codex".into(),
                session_id: "sess-pace".into(),
                model: None,
                conversation_id: None,
            }),
        )
        .await;
        let data = join_resp.0.data.expect("join must succeed");
        assert_eq!(
            data.pacing.regime,
            crate::api::disc_introspection::PacingRegime::Hot
        );
        assert_eq!(
            data.pacing.next_delay_seconds,
            data.poll_policy.hot_poll_seconds
        );
        assert!(
            data.pacing.attention_until.is_some(),
            "hot carries the lease end"
        );
    }

    #[tokio::test]
    async fn disc_meta_pacing_uses_the_reception_clock_like_wait_and_join() {
        // Codex round 4: meta must share the SAME anchors as wait/join. A
        // federated User message authored 3h ago but received NOW must put
        // meta in the hot regime — an anchor on the authored timestamp
        // would answer cold and the endpoints would contradict each other.
        let state = make_state_with_disc("d-meta-pace").await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO messages
                        (id, discussion_id, role, content, timestamp, sort_order, received_at)
                     VALUES ('m-fed', 'd-meta-pace', 'User', 'ping', ?1, 1, ?2)",
                    rusqlite::params![
                        (chrono::Utc::now() - chrono::Duration::hours(3)).to_rfc3339(),
                        chrono::Utc::now().to_rfc3339(),
                    ],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let resp = crate::api::disc_introspection::disc_meta(
            State(state.clone()),
            Path("d-meta-pace".to_string()),
        )
        .await;
        let meta = resp.0.data.expect("meta must succeed");
        let pacing = meta.pacing.expect("meta carries pacing");
        assert_eq!(
            pacing.regime,
            crate::api::disc_introspection::PacingRegime::Hot,
            "reception clock renews the lease"
        );
        assert!(pacing.attention_until.is_some());
    }

    #[tokio::test]
    async fn peer_join_rejects_invalid_token() {
        let state = make_state_with_disc("d-join-2").await;
        let resp = peer_join(
            State(state),
            Json(PeerJoinRequest {
                token: "kr-join-bogus".into(),
                agent_type: "Codex".into(),
                session_id: "sess".into(),
                model: None,
                conversation_id: None,
            }),
        )
        .await;
        let body = resp.0;
        assert!(!body.success);
        assert!(body.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn peer_join_rejects_blank_inputs() {
        let state = make_state_with_disc("d-join-3").await;
        for bad in [
            PeerJoinRequest {
                token: "".into(),
                agent_type: "Codex".into(),
                session_id: "s".into(),
                model: None,
                conversation_id: None,
            },
            PeerJoinRequest {
                token: "kr-join-x".into(),
                agent_type: "".into(),
                session_id: "s".into(),
                model: None,
                conversation_id: None,
            },
            PeerJoinRequest {
                token: "kr-join-x".into(),
                agent_type: "Codex".into(),
                session_id: "".into(),
                model: None,
                conversation_id: None,
            },
        ] {
            let resp = peer_join(State(state.clone()), Json(bad)).await;
            assert!(!resp.0.success);
        }
    }

    #[tokio::test]
    async fn peer_join_records_declared_model_and_rejects_overlong() {
        // KT-37 — a JOIN may self-declare its model (recorded verbatim as
        // declared-at-join, trimmed); an over-long declaration is refused, never
        // silently truncated.
        let state = make_state_with_disc("d-join-model").await;
        let invite = invite_peer(State(state.clone()), Path("d-join-model".to_string())).await;
        let token = invite.0.data.unwrap().token;

        let ok = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token: token.clone(),
                agent_type: "Codex".into(),
                session_id: "sess-model".into(),
                model: Some("  gpt-5-codex  ".into()),
                conversation_id: None,
            }),
        )
        .await;
        assert!(
            ok.0.success,
            "join with a declared model must succeed: {:?}",
            ok.0.error
        );
        let sessions = state
            .db
            .with_conn(|conn| {
                crate::db::discussion_sessions::list_sessions(conn, "d-join-model", false)
            })
            .await
            .unwrap();
        assert_eq!(
            sessions
                .iter()
                .find(|s| s.session_id.as_deref() == Some("sess-model"))
                .unwrap()
                .model
                .as_deref(),
            Some("gpt-5-codex"),
            "declared model is trimmed + recorded verbatim (no truncation)"
        );

        // Over-long → clean refusal, never a mangled name.
        let rejected = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token: token.clone(),
                agent_type: "Codex".into(),
                session_id: "sess-long".into(),
                model: Some("m".repeat(201)),
                conversation_id: None,
            }),
        )
        .await;
        assert!(!rejected.0.success);
        assert!(rejected.0.error.unwrap().contains("too long"));
    }

    #[tokio::test]
    async fn peer_join_rejects_non_uuid_conversation_id_without_creating_a_session() {
        let state = make_state_with_disc("d-join-native-invalid").await;
        let invite = invite_peer(
            State(state.clone()),
            Path("d-join-native-invalid".to_string()),
        )
        .await;
        let token = invite.0.data.unwrap().token;

        let rejected = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token,
                agent_type: "Codex".into(),
                session_id: "sess-native-invalid".into(),
                model: None,
                conversation_id: Some("$(unsafe)".into()),
            }),
        )
        .await;
        assert!(!rejected.0.success);
        assert!(rejected.0.error.unwrap().contains("canonical UUID"));
        let sessions = state
            .db
            .with_conn(|conn| {
                db::discussion_sessions::list_sessions(conn, "d-join-native-invalid", false)
            })
            .await
            .unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn peer_join_multi_use_within_ttl() {
        // 0.8.6 fix 2026-05-21 — token is multi-use within TTL. The
        // route contract must let N peers join with the same token,
        // up to expiry. UX win : user clicks [+ Inviter] once for
        // the whole multi-agent room (3 agents = 1 invite instead
        // of 3).
        let state = make_state_with_disc("d-join-4").await;
        let invite = invite_peer(State(state.clone()), Path("d-join-4".to_string())).await;
        let token = invite.0.data.unwrap().token;

        for (agent, sess) in [
            ("ClaudeCode", "sess-A"),
            ("Codex", "sess-B"),
            ("GeminiCli", "sess-C"),
        ] {
            let r = peer_join(
                State(state.clone()),
                Json(PeerJoinRequest {
                    token: token.clone(),
                    agent_type: agent.into(),
                    session_id: sess.into(),
                    model: None,
                    conversation_id: None,
                }),
            )
            .await;
            assert!(r.0.success, "{} could not join: {:?}", agent, r.0.error);
        }
    }

    // ─── E2E 2-peer collab (0.8.6 phase 4) ─────────────────────
    //
    // The whole point of phase 1-3 was : two CLI agents sit in the
    // same Kronn disc and dialogue without a human messenger. This
    // test exercises the full chain end-to-end at the handler layer :
    //
    //   1. user creates a disc (project + discussion rows seeded)
    //   2. user mints invite #1, agent A joins (peer row #1)
    //   3. agent A "writes" a message (direct INSERT into `messages`
    //      — simulates what `disc_append` would do without coupling
    //      this test to the cross-agent-memory endpoint)
    //   4. user mints invite #2, agent B joins (peer row #2)
    //   5. agent B calls `wait_for_peer` excluding its own
    //      agent_type → receives A's message immediately
    //   6. agent B writes its own message
    //   7. agent A calls `wait_for_peer` excluding ITS own type →
    //      receives B's message
    //   8. agent A leaves → header drops to 1 participant
    //   9. agent B leaves → header empty
    //
    // Passes only when every layer (invite tokens, sessions table,
    // wait long-poll, leave handler, participants list) is correctly
    // wired. Catches regressions where a single layer drifts.

    async fn insert_message(
        state: &AppState,
        disc_id: &str,
        msg_id: &str,
        sort_order: i64,
        author_agent: &str,
        content: &str,
    ) {
        let disc_id = disc_id.to_string();
        let msg_id = msg_id.to_string();
        let author = author_agent.to_string();
        let content = content.to_string();
        state
            .db
            .with_conn(move |conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages
                        (id, discussion_id, role, content, agent_type, timestamp, sort_order)
                     VALUES (?1, ?2, 'Agent', ?3, ?4, ?5, ?6)",
                    rusqlite::params![&msg_id, &disc_id, &content, &author, now, sort_order],
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn e2e_two_peer_collab_full_dialogue_via_handlers() {
        let state = make_state_with_disc("d-e2e").await;

        // ── Step 2: agent A (ClaudeCode) joins via invite #1 ──
        let inv1 = invite_peer(State(state.clone()), Path("d-e2e".to_string())).await;
        let token_a = inv1.0.data.unwrap().token;
        let join_a = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token: token_a,
                agent_type: "ClaudeCode".into(),
                session_id: "sess-A".into(),
                model: None,
                conversation_id: None,
            }),
        )
        .await;
        assert!(
            join_a.0.success,
            "agent A join failed: {:?}",
            join_a.0.error
        );
        let join_a_data = join_a.0.data.unwrap();
        assert_eq!(join_a_data.peer_count, 1);

        // Header shows 1 active participant : agent A.
        let parts1 = list_participants(State(state.clone()), Path("d-e2e".to_string())).await;
        let p1 = parts1.0.data.unwrap();
        assert_eq!(p1.len(), 1);
        assert_eq!(p1[0].agent_type, "ClaudeCode");

        // ── Step 3: agent A writes a message ──
        insert_message(
            &state,
            "d-e2e",
            "msg-1",
            1,
            "ClaudeCode",
            "hello, anyone here ?",
        )
        .await;

        // ── Step 4: agent B (Codex) joins via invite #2 ──
        let inv2 = invite_peer(State(state.clone()), Path("d-e2e".to_string())).await;
        let token_b = inv2.0.data.unwrap().token;
        let join_b = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token: token_b,
                agent_type: "Codex".into(),
                session_id: "sess-B".into(),
                model: None,
                conversation_id: None,
            }),
        )
        .await;
        assert!(
            join_b.0.success,
            "agent B join failed: {:?}",
            join_b.0.error
        );
        let join_b_data = join_b.0.data.unwrap();
        assert_eq!(join_b_data.peer_count, 2, "both A and B now active");
        // join() returns recent_messages — agent B sees agent A's hello.
        assert_eq!(join_b_data.recent_messages.len(), 1);
        assert!(join_b_data.recent_messages[0].preview.contains("hello"));

        // Header now shows 2 active participants.
        let parts2 = list_participants(State(state.clone()), Path("d-e2e".to_string())).await;
        let p2 = parts2.0.data.unwrap();
        assert_eq!(p2.len(), 2);
        let types: Vec<&str> = p2.iter().map(|s| s.agent_type.as_str()).collect();
        assert!(types.contains(&"ClaudeCode"));
        assert!(types.contains(&"Codex"));

        // ── Step 5: agent B's wait_for_peer receives agent A's msg ──
        // since=0 + exclude=Codex → message from ClaudeCode wakes it.
        let wait_b = wait_for_peer(
            State(state.clone()),
            Path("d-e2e".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(3),
                exclude_agent_type: Some("Codex".into()),
                session_id: None,
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await;
        let wait_b_data = wait_b.0.data.unwrap();
        assert!(!wait_b_data.timed_out);
        assert_eq!(wait_b_data.messages.len(), 1);
        assert_eq!(wait_b_data.messages[0].content, "hello, anyone here ?");
        assert_eq!(wait_b_data.latest_sort_order, 1);

        // ── Step 6: agent B replies ──
        insert_message(&state, "d-e2e", "msg-2", 2, "Codex", "yes, codex here").await;

        // ── Step 7: agent A receives agent B's reply ──
        let wait_a = wait_for_peer(
            State(state.clone()),
            Path("d-e2e".to_string()),
            Query(WaitForPeerQuery {
                // Pretend agent A had already advanced past its own
                // message (sort_order=1). Otherwise it would also
                // receive its own back — agents always pass `since`
                // = last_observed.
                since_sort_order: Some(1),
                timeout_secs: Some(3),
                exclude_agent_type: Some("ClaudeCode".into()),
                session_id: None,
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await;
        let wait_a_data = wait_a.0.data.unwrap();
        assert!(!wait_a_data.timed_out);
        assert_eq!(wait_a_data.messages.len(), 1);
        assert_eq!(wait_a_data.messages[0].content, "yes, codex here");

        // ── Step 8: agent A leaves ──
        let leave_a = peer_leave(
            State(state.clone()),
            Json(PeerLeaveRequest {
                agent_type: "ClaudeCode".into(),
                session_id: "sess-A".into(),
            }),
        )
        .await;
        assert!(leave_a.0.data.unwrap().left);
        let parts3 = list_participants(State(state.clone()), Path("d-e2e".to_string())).await;
        let p3 = parts3.0.data.unwrap();
        assert_eq!(p3.len(), 1);
        assert_eq!(p3[0].agent_type, "Codex");

        // ── Step 9: agent B leaves → header empty ──
        let leave_b = peer_leave(
            State(state.clone()),
            Json(PeerLeaveRequest {
                agent_type: "Codex".into(),
                session_id: "sess-B".into(),
            }),
        )
        .await;
        assert!(leave_b.0.data.unwrap().left);
        let parts4 = list_participants(State(state.clone()), Path("d-e2e".to_string())).await;
        assert_eq!(parts4.0.data.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn e2e_each_invite_yields_a_separate_token_so_n_peers_can_join() {
        // Regression guard : a single invite token is single-use, so
        // inviting N peers requires N distinct tokens. We mint 3 in a
        // row and successfully join 3 different agent_types. Locks the
        // contract that the UI is expected to "click invite once per
        // new peer".
        let state = make_state_with_disc("d-e2e-multi").await;
        let mut joined = 0;
        for (agent, sess) in [("ClaudeCode", "s1"), ("Codex", "s2"), ("GeminiCli", "s3")] {
            let inv = invite_peer(State(state.clone()), Path("d-e2e-multi".to_string())).await;
            let token = inv.0.data.unwrap().token;
            let join = peer_join(
                State(state.clone()),
                Json(PeerJoinRequest {
                    token,
                    agent_type: agent.into(),
                    session_id: sess.into(),
                    model: None,
                    conversation_id: None,
                }),
            )
            .await;
            assert!(
                join.0.success,
                "agent {} could not join: {:?}",
                agent, join.0.error
            );
            joined += 1;
        }
        assert_eq!(joined, 3);
        let parts = list_participants(State(state), Path("d-e2e-multi".to_string())).await;
        assert_eq!(parts.0.data.unwrap().len(), 3);
    }

    // ─── peer_leave (0.8.6 phase 3) ────────────────────────────

    #[tokio::test]
    async fn peer_leave_marks_active_session_left_and_is_idempotent() {
        let state = make_state_with_disc("d-leave-1").await;
        let invite = invite_peer(State(state.clone()), Path("d-leave-1".to_string())).await;
        let token = invite.0.data.unwrap().token;
        let _ = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token,
                agent_type: "Codex".into(),
                session_id: "sess-Z".into(),
                model: None,
                conversation_id: None,
            }),
        )
        .await;

        // First leave : found + marked.
        let r1 = peer_leave(
            State(state.clone()),
            Json(PeerLeaveRequest {
                agent_type: "Codex".into(),
                session_id: "sess-Z".into(),
            }),
        )
        .await;
        assert!(r1.0.success);
        assert!(r1.0.data.unwrap().left);

        // Second leave : already gone, returns left=false but no error.
        let r2 = peer_leave(
            State(state.clone()),
            Json(PeerLeaveRequest {
                agent_type: "Codex".into(),
                session_id: "sess-Z".into(),
            }),
        )
        .await;
        assert!(r2.0.success);
        assert!(!r2.0.data.unwrap().left);

        // Header view no longer lists this peer.
        let parts = list_participants(State(state), Path("d-leave-1".to_string())).await;
        assert_eq!(parts.0.data.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn peer_leave_rejects_blank_inputs() {
        let state = make_state_with_disc("d-leave-2").await;
        let resp = peer_leave(
            State(state),
            Json(PeerLeaveRequest {
                agent_type: "".into(),
                session_id: "s".into(),
            }),
        )
        .await;
        assert!(!resp.0.success);
    }

    #[tokio::test]
    async fn peer_leave_returns_false_for_unknown_session_without_error() {
        // Calling leave on a session that never joined must not throw
        // — the agent might call disc_leave defensively at the end of
        // a session even if disc_join failed.
        let state = make_state_with_disc("d-leave-3").await;
        let resp = peer_leave(
            State(state),
            Json(PeerLeaveRequest {
                agent_type: "Codex".into(),
                session_id: "ghost".into(),
            }),
        )
        .await;
        assert!(resp.0.success);
        assert!(!resp.0.data.unwrap().left);
    }

    // ─── wait_for_peer (0.8.6 phase 3) ──────────────────────────

    #[tokio::test]
    async fn wait_for_peer_returns_immediately_when_new_message_exists() {
        // When a message newer than `since` is already in the DB, the
        // endpoint returns on the first poll without waiting.
        let state = make_state_with_disc("d-wait-1").await;
        // Seed a message at sort_order=5.
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages
                        (id, discussion_id, role, content, agent_type, timestamp, sort_order)
                     VALUES ('msg-1', 'd-wait-1', 'Agent', 'hello peer', 'Codex', ?1, 5)",
                    rusqlite::params![now],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let resp = wait_for_peer(
            State(state),
            Path("d-wait-1".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(5),
                exclude_agent_type: None,
                session_id: None,
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await;
        let body = resp.0;
        assert!(body.success);
        let data = body.data.unwrap();
        assert!(!data.timed_out);
        assert_eq!(data.messages.len(), 1);
        assert_eq!(data.messages[0].content, "hello peer");
        assert_eq!(data.latest_sort_order, 5);
    }

    #[tokio::test]
    async fn wait_hides_notes_but_advances_the_durable_cursor() {
        let state = make_state_with_disc("d-wait-note").await;
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages
                        (id, discussion_id, role, channel, content, timestamp, sort_order)
                     VALUES ('note-hidden', 'd-wait-note', 'User', 'note', 'do not deliver', ?1, 5)",
                    rusqlite::params![&now],
                )?;
                conn.execute(
                    "INSERT INTO message_revision_events (
                         id, discussion_id, target_message_id,
                         previous_content_hash, expected_revision, revision,
                         content, idempotency_key, sort_order, created_at
                     ) VALUES (
                         'note-revision-hidden', 'd-wait-note', 'note-hidden',
                         'hash-before', 'opaque-before', ?1,
                         'still do not deliver', 'note-revision-key', 6, ?1
                     )",
                    rusqlite::params![now],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let data = wait_for_peer(
            State(state),
            Path("d-wait-note".into()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: None,
                session_id: None,
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();

        assert!(data.timed_out);
        assert!(data.messages.is_empty());
        assert_eq!(data.latest_sort_order, 6);
    }

    /// KT-43 — the promise is "no silent latency": a wait that is ALREADY
    /// blocking must hand over a message that lands afterwards, and do it fast
    /// enough that the delay is the agent's loop, never Kronn. Measured on the
    /// live backend at 6–14 ms; the bound here is loose enough for CI yet
    /// tight enough to catch a regression to "next poll tick, maybe".
    #[tokio::test]
    async fn wait_already_blocking_delivers_a_later_message_within_a_bounded_delay() {
        let state = make_state_with_disc("d-wait-bounded").await;
        let waiting_state = state.clone();

        let waiter = tokio::spawn(async move {
            let started = std::time::Instant::now();
            let resp = wait_for_peer(
                State(waiting_state),
                Path("d-wait-bounded".to_string()),
                Query(WaitForPeerQuery {
                    since_sort_order: Some(0),
                    timeout_secs: Some(10),
                    exclude_agent_type: None,
                    session_id: None,
                    conversation_id: None,
                    ack_awareness_upto: None,
                }),
            )
            .await;
            (resp.0.data.unwrap(), started.elapsed())
        });

        // Let the wait actually enter its loop before the peer speaks —
        // otherwise we would be re-testing the already-present case above.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let posted = std::time::Instant::now();
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages
                        (id, discussion_id, role, content, agent_type, timestamp, sort_order)
                     VALUES ('msg-late', 'd-wait-bounded', 'Agent', 'late peer', 'Codex', ?1, 7)",
                    rusqlite::params![now],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let (data, _held) = waiter.await.unwrap();
        let latency = posted.elapsed();

        assert!(
            !data.timed_out,
            "a message landed — this must not read as a timeout"
        );
        assert_eq!(data.messages.len(), 1);
        assert_eq!(data.messages[0].content, "late peer");
        assert_eq!(data.latest_sort_order, 7);
        assert!(
            latency < Duration::from_secs(3),
            "woke {latency:?} after the peer posted — latency must come from the \
             agent's loop, not from Kronn"
        );
    }

    #[tokio::test]
    async fn wait_for_peer_observes_message_revision_event_beyond_cursor() {
        let state = make_state_with_disc("d-wait-revision").await;
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (
                         id, discussion_id, role, channel, content, timestamp, sort_order
                     ) VALUES (
                         'user-target', 'd-wait-revision', 'User', 'main',
                         'original content', ?1, 3
                     )",
                    rusqlite::params![&now],
                )?;
                conn.execute(
                    "INSERT INTO message_revision_events (
                         id, discussion_id, target_message_id,
                         previous_content_hash, expected_revision, revision,
                         content, idempotency_key, sort_order, created_at
                     ) VALUES (
                         'rev-event', 'd-wait-revision', 'user-target',
                         'hash-before', 'opaque-before', ?1,
                         'edited content', 'revision-key', 4, ?1
                     )",
                    rusqlite::params![now],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let response = wait_for_peer(
            State(state),
            Path("d-wait-revision".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(3),
                timeout_secs: Some(5),
                exclude_agent_type: None,
                session_id: None,
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await;
        let data = response.0.data.unwrap();
        assert!(!data.timed_out);
        assert_eq!(data.latest_sort_order, 4);
        assert_eq!(data.messages.len(), 1);
        assert_eq!(data.messages[0].message_id, "rev-event");
        assert_eq!(
            data.messages[0].event_type.as_deref(),
            Some("message_revised")
        );
        assert_eq!(
            data.messages[0].target_message_id.as_deref(),
            Some("user-target")
        );
        assert!(data.messages[0].content.contains("edited content"));
    }

    #[tokio::test]
    async fn wait_for_peer_excludes_caller_agent_type() {
        // When `exclude_agent_type=ClaudeCode` is set, the endpoint
        // does NOT wake on a ClaudeCode message — the agent is its
        // own author and shouldn't ping itself.
        let state = make_state_with_disc("d-wait-2").await;
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages
                        (id, discussion_id, role, content, agent_type, timestamp, sort_order)
                     VALUES ('msg-self', 'd-wait-2', 'Agent', 'my own msg', 'ClaudeCode', ?1, 7)",
                    rusqlite::params![now],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let resp = wait_for_peer(
            State(state),
            Path("d-wait-2".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                // Tight timeout so the test doesn't drag — fake-time
                // advances automatically with `start_paused = true`.
                timeout_secs: Some(2),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: None,
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await;
        let body = resp.0;
        assert!(body.success);
        let data = body.data.unwrap();
        assert!(data.timed_out, "self-message must not wake the wait");
        assert_eq!(data.messages.len(), 0);
    }

    #[tokio::test]
    async fn wait_for_peer_exposes_all_targets_without_hiding_the_turn() {
        let state = make_state_with_disc("d-wait-target").await;
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (
                         id, discussion_id, role, content, timestamp,
                         sort_order, target_agent
                     ) VALUES (
                         'msg-target', 'd-wait-target', 'User',
                         '@codex confronte @claude', ?1, 3, 'Codex'
                     )",
                    rusqlite::params![now],
                )?;
                conn.execute(
                    "INSERT INTO message_targets (
                         message_id, target_kind, agent_type, cli_session_id, position
                     ) VALUES
                         ('msg-target', 'agent', 'Codex', NULL, 0),
                         ('msg-target', 'agent', 'ClaudeCode', NULL, 1)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let unrelated = wait_for_peer(
            State(state.clone()),
            Path("d-wait-target".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("Vibe".to_string()),
                session_id: None,
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();
        assert!(
            !unrelated.timed_out,
            "all peers must observe the human turn so they can detect a target failure"
        );
        assert_eq!(
            unrelated.messages[0].target_agent.as_deref(),
            Some("Codex"),
            "an unlisted peer receives the routing marker and must abstain"
        );
        assert_eq!(
            unrelated.messages[0].target_agents,
            vec!["Codex", "ClaudeCode"],
        );

        let target = wait_for_peer(
            State(state),
            Path("d-wait-target".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("Codex".to_string()),
                session_id: None,
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();
        assert!(!target.timed_out);
        assert_eq!(target.messages.len(), 1);
        assert_eq!(target.messages[0].target_agent.as_deref(), Some("Codex"));
        assert_eq!(
            target.messages[0].target_agents,
            vec!["Codex", "ClaudeCode"],
        );
    }

    #[tokio::test]
    async fn wait_for_peer_delivers_user_turn_only_to_the_exact_cli_target() {
        let state = make_state_with_disc("d-wait-exact-cli").await;
        let (codex_session, _vibe_session) = state
            .db
            .with_conn(|conn| {
                let codex = crate::db::discussion_sessions::create_session(
                    conn,
                    "d-wait-exact-cli",
                    "Codex",
                    Some("codex-exact"),
                    "peer",
                )?;
                let vibe = crate::db::discussion_sessions::create_session(
                    conn,
                    "d-wait-exact-cli",
                    "Vibe",
                    Some("vibe-unrelated"),
                    "peer",
                )?;
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (
                         id, discussion_id, role, content, timestamp, sort_order,
                         target_agent
                     ) VALUES (
                         'msg-exact-cli', 'd-wait-exact-cli', 'User',
                         'private CLI turn', ?1, 3, 'Codex'
                     )",
                    rusqlite::params![now],
                )?;
                conn.execute(
                    "INSERT INTO message_targets (
                         message_id, target_kind, agent_type, cli_session_id, position
                     ) VALUES ('msg-exact-cli', 'cli', 'Codex', ?1, 0)",
                    [codex],
                )?;
                conn.execute(
                    "INSERT INTO messages (
                         id, discussion_id, role, content, agent_type, timestamp,
                         sort_order, target_agent
                     ) VALUES (
                         'msg-agent-exact-cli', 'd-wait-exact-cli', 'Agent',
                         'private peer-to-peer CLI turn', 'ClaudeCode', ?1, 4, 'Codex'
                     )",
                    rusqlite::params![now],
                )?;
                conn.execute(
                    "INSERT INTO message_targets (
                         message_id, target_kind, agent_type, cli_session_id, position
                     ) VALUES ('msg-agent-exact-cli', 'cli', 'Codex', ?1, 0)",
                    [codex],
                )?;
                Ok((codex, vibe))
            })
            .await
            .unwrap();

        let unrelated = wait_for_peer(
            State(state.clone()),
            Path("d-wait-exact-cli".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("Vibe".to_string()),
                session_id: Some("vibe-unrelated".to_string()),
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();
        assert!(unrelated.timed_out);
        assert!(unrelated.messages.is_empty());
        assert_eq!(
            unrelated.latest_sort_order, 4,
            "a hidden turn advances the cursor instead of waking the unrelated CLI forever"
        );
        assert_eq!(
            unrelated.withheld_by_routing, 2,
            "the cursor moved past two turns, so say so — otherwise this CLI cannot \
             tell 'not for me' from 'never arrived' and reads as having ignored them"
        );

        let addressed = wait_for_peer(
            State(state),
            Path("d-wait-exact-cli".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("Codex".to_string()),
                session_id: Some("codex-exact".to_string()),
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();
        assert!(!addressed.timed_out);
        assert_eq!(addressed.messages.len(), 2);
        assert_eq!(addressed.messages[0].message_id, "msg-exact-cli");
        assert_eq!(addressed.messages[1].message_id, "msg-agent-exact-cli");
        assert_eq!(
            addressed.withheld_by_routing, 0,
            "nothing was held back from the CLI both turns were addressed to"
        );
        assert!(addressed.messages[0].addressed_to_caller);
        assert!(addressed.messages[1].addressed_to_caller);
        assert_eq!(
            addressed.messages[1].content,
            "private peer-to-peer CLI turn"
        );
        assert_eq!(
            addressed.messages[0].targets,
            vec![MessageTarget::cli(
                crate::models::AgentType::Codex,
                codex_session,
            )],
        );
    }

    #[tokio::test]
    async fn wake_attaches_unseen_room_traffic_as_awareness_until_acked() {
        // KT-189 — untargeted room traffic (User AND Agent turns) never wakes
        // a joined CLI; it attaches ONCE ACKED-GATED to its next legitimate
        // wake, flagged `awareness`, so the session keeps full room context
        // at zero extra model turns.
        let state = make_state_with_disc("d-awareness").await;
        let session_pk = state
            .db
            .with_conn(|conn| {
                let pk = crate::db::discussion_sessions::create_session(
                    conn,
                    "d-awareness",
                    "ClaudeCode",
                    Some("cli-a"),
                    "peer",
                )?;
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order)
                     VALUES ('u-1', 'd-awareness', 'User', 'question one', ?1, 1)",
                    rusqlite::params![now],
                )?;
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         agent_type, timestamp, sort_order)
                     VALUES ('a-1', 'd-awareness', 'Agent', 'native answer',
                         'Codex', ?1, 2)",
                    rusqlite::params![now],
                )?;
                // The wake: a turn addressed to this exact CLI session.
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order)
                     VALUES ('u-wake', 'd-awareness', 'User', 'for you', ?1, 3)",
                    rusqlite::params![now],
                )?;
                crate::db::discussions::replace_message_targets(
                    conn,
                    "u-wake",
                    &[crate::models::MessageTarget::cli(
                        crate::models::AgentType::ClaudeCode,
                        pk,
                    )],
                )?;
                Ok(pk)
            })
            .await
            .unwrap();

        let first = wait_for_peer(
            State(state.clone()),
            Path("d-awareness".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: Some("cli-a".to_string()),
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();
        assert!(!first.timed_out);
        let awareness: Vec<_> = first.messages.iter().filter(|m| m.awareness).collect();
        assert_eq!(
            awareness
                .iter()
                .map(|m| m.message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["u-1", "a-1"],
            "both untargeted turns attach as awareness, in transcript order"
        );
        assert!(awareness.iter().all(|m| !m.addressed_to_caller));
        assert!(first
            .messages
            .iter()
            .any(|m| m.message_id == "u-wake" && !m.awareness && m.addressed_to_caller));
        assert_eq!(first.awareness_delivered_upto, Some(2));
        assert_eq!(first.awareness_omitted, 0);

        // Unacked: an identical wait (delivery lost, e.g. cancelled call)
        // replays the SAME awareness batch instead of skipping it.
        let replay = wait_for_peer(
            State(state.clone()),
            Path("d-awareness".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: Some("cli-a".to_string()),
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();
        assert_eq!(replay.messages.iter().filter(|m| m.awareness).count(), 2);

        // Acked: the durable cursor advances and the batch never returns.
        let acked = wait_for_peer(
            State(state.clone()),
            Path("d-awareness".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: Some("cli-a".to_string()),
                conversation_id: None,
                ack_awareness_upto: Some(2),
            }),
        )
        .await
        .0
        .data
        .unwrap();
        assert!(acked.messages.iter().all(|m| !m.awareness));
        assert_eq!(acked.awareness_delivered_upto, None);
        let _ = session_pk;
    }

    #[tokio::test]
    async fn awareness_is_capped_and_the_remainder_returns_with_the_next_wake() {
        let state = make_state_with_disc("d-awareness-cap").await;
        let session_pk = state
            .db
            .with_conn(|conn| {
                let pk = crate::db::discussion_sessions::create_session(
                    conn,
                    "d-awareness-cap",
                    "ClaudeCode",
                    Some("cli-cap"),
                    "peer",
                )?;
                let now = chrono::Utc::now().to_rfc3339();
                for sort_order in 1..=22_i64 {
                    conn.execute(
                        "INSERT INTO messages (id, discussion_id, role, content,
                             timestamp, sort_order)
                         VALUES (?1, 'd-awareness-cap', 'User', ?2, ?3, ?4)",
                        rusqlite::params![
                            format!("u-cap-{sort_order}"),
                            format!("missed turn {sort_order}"),
                            &now,
                            sort_order,
                        ],
                    )?;
                }
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order)
                     VALUES ('u-wake-1', 'd-awareness-cap', 'User', 'ping', ?1, 23)",
                    rusqlite::params![now],
                )?;
                crate::db::discussions::replace_message_targets(
                    conn,
                    "u-wake-1",
                    &[crate::models::MessageTarget::cli(
                        crate::models::AgentType::ClaudeCode,
                        pk,
                    )],
                )?;
                Ok(pk)
            })
            .await
            .unwrap();

        let first = wait_for_peer(
            State(state.clone()),
            Path("d-awareness-cap".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: Some("cli-cap".to_string()),
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();
        assert_eq!(
            first.messages.iter().filter(|m| m.awareness).count(),
            AWARENESS_MAX_MESSAGES,
        );
        assert_eq!(first.awareness_omitted, 2);
        let upto = first.awareness_delivered_upto.unwrap();
        assert_eq!(upto, AWARENESS_MAX_MESSAGES as i64);

        // Ack DELIBERATELY beyond the offered batch (the cap ended at 20):
        // the server must clamp to the last offerable row, so the remainder
        // still returns with the next wake instead of being skipped.
        let (second_pk,) = (session_pk,);
        state
            .db
            .with_conn(move |conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order)
                     VALUES ('u-wake-2', 'd-awareness-cap', 'User', 'ping 2', ?1, 24)",
                    rusqlite::params![now],
                )?;
                crate::db::discussions::replace_message_targets(
                    conn,
                    "u-wake-2",
                    &[crate::models::MessageTarget::cli(
                        crate::models::AgentType::ClaudeCode,
                        second_pk,
                    )],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let second = wait_for_peer(
            State(state),
            Path("d-awareness-cap".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(23),
                timeout_secs: Some(1),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: Some("cli-cap".to_string()),
                conversation_id: None,
                // Oversized on purpose — see comment above.
                ack_awareness_upto: Some(upto + 4),
            }),
        )
        .await
        .0
        .data
        .unwrap();
        let remainder: Vec<_> = second.messages.iter().filter(|m| m.awareness).collect();
        assert_eq!(
            remainder
                .iter()
                .map(|m| m.message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["u-cap-21", "u-cap-22"],
        );
        assert_eq!(second.awareness_omitted, 0);
    }

    #[tokio::test]
    async fn rows_arriving_between_offer_and_ack_are_never_acked_away() {
        // KT-189 scan/offer/ack: a batch is offered (u-1), new awareness
        // rows land BEFORE the client acks, and the ack overshoots them.
        // The clamp to the persisted offered cursor must keep them alive
        // for the next wake.
        let state = make_state_with_disc("d-offer-race").await;
        let session_pk = state
            .db
            .with_conn(|conn| {
                let pk = crate::db::discussion_sessions::create_session(
                    conn,
                    "d-offer-race",
                    "ClaudeCode",
                    Some("cli-r"),
                    "peer",
                )?;
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order)
                     VALUES ('u-1', 'd-offer-race', 'User', 'offered turn', ?1, 1)",
                    rusqlite::params![now],
                )?;
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order)
                     VALUES ('u-wake-1', 'd-offer-race', 'User', 'ping', ?1, 2)",
                    rusqlite::params![now],
                )?;
                crate::db::discussions::replace_message_targets(
                    conn,
                    "u-wake-1",
                    &[crate::models::MessageTarget::cli(
                        crate::models::AgentType::ClaudeCode,
                        pk,
                    )],
                )?;
                Ok(pk)
            })
            .await
            .unwrap();

        // Offer: the wake attaches u-1 (offered cursor → 1).
        let offered = wait_for_peer(
            State(state.clone()),
            Path("d-offer-race".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: Some("cli-r".to_string()),
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();
        assert_eq!(offered.awareness_delivered_upto, Some(1));

        // New awareness rows land BETWEEN the offer and the ack…
        state
            .db
            .with_conn(move |conn| {
                let now = chrono::Utc::now().to_rfc3339();
                for (id, sort) in [("u-3", 3_i64), ("u-4", 4_i64)] {
                    conn.execute(
                        "INSERT INTO messages (id, discussion_id, role, content,
                             timestamp, sort_order)
                         VALUES (?1, 'd-offer-race', 'User', 'racing turn', ?2, ?3)",
                        rusqlite::params![id, now, sort],
                    )?;
                }
                let now2 = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order)
                     VALUES ('u-wake-2', 'd-offer-race', 'User', 'ping 2', ?1, 5)",
                    rusqlite::params![now2],
                )?;
                crate::db::discussions::replace_message_targets(
                    conn,
                    "u-wake-2",
                    &[crate::models::MessageTarget::cli(
                        crate::models::AgentType::ClaudeCode,
                        session_pk,
                    )],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        // …and the buggy ack overshoots to 4: it must clamp to offered=1,
        // so u-3/u-4 still arrive as awareness with this very wake.
        let woken = wait_for_peer(
            State(state),
            Path("d-offer-race".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(2),
                timeout_secs: Some(1),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: Some("cli-r".to_string()),
                conversation_id: None,
                ack_awareness_upto: Some(4),
            }),
        )
        .await
        .0
        .data
        .unwrap();
        let awareness: Vec<_> = woken
            .messages
            .iter()
            .filter(|m| m.awareness)
            .map(|m| m.message_id.as_str())
            .collect();
        assert_eq!(awareness, vec!["u-3", "u-4"]);
    }

    #[tokio::test]
    async fn session_id_only_caller_keeps_presence_alive() {
        // KT-189 review residual 3: a bridge that cannot name its provider
        // omits exclude_agent_type but still sends its session id. It must
        // get the modern contract AND live presence (heartbeat + waiting
        // state), using the agent_type stored on its own session row.
        let state = make_state_with_disc("d-unknown-cli").await;
        state
            .db
            .with_conn(|conn| {
                let pk = crate::db::discussion_sessions::create_session(
                    conn,
                    "d-unknown-cli",
                    "ClaudeCode",
                    Some("cli-unknown"),
                    "peer",
                )?;
                // Make last_seen visibly stale so the heartbeat bump shows.
                conn.execute(
                    "UPDATE discussion_sessions
                        SET last_seen = '2000-01-01T00:00:00Z'
                      WHERE id = ?1",
                    rusqlite::params![pk],
                )?;
                let now = chrono::Utc::now().to_rfc3339();
                // An untargeted Agent turn: must NOT wake a modern caller.
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         agent_type, timestamp, sort_order)
                     VALUES ('a-noise', 'd-unknown-cli', 'Agent', 'chatter',
                         'Codex', ?1, 1)",
                    rusqlite::params![now],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let response = wait_for_peer(
            State(state.clone()),
            Path("d-unknown-cli".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: None,
                session_id: Some("cli-unknown".to_string()),
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();
        // Modern contract applies: the untargeted Agent turn is not a wake.
        assert!(
            response.timed_out,
            "session-id-only caller must not wake-all"
        );
        assert!(
            response.next_poll_at.is_some(),
            "waiting state was recorded"
        );

        let session = state
            .db
            .with_conn(|conn| {
                crate::db::discussion_sessions::list_sessions(conn, "d-unknown-cli", false)
            })
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_ne!(
            session.last_seen.as_deref(),
            Some("2000-01-01T00:00:00Z"),
            "heartbeat bumped without exclude_agent_type"
        );
        assert!(
            session.activity.is_some(),
            "listening/waiting presence recorded without exclude_agent_type"
        );
    }

    #[tokio::test]
    async fn an_ack_pointing_into_the_future_cannot_skip_unwritten_turns() {
        // KT-189 fail-closed guard: ack=1e9 with a 3-row discussion must
        // clamp to the tip, so awareness written AFTERWARDS still reaches
        // the session with its next wake.
        let state = make_state_with_disc("d-future-ack").await;
        let session_pk = state
            .db
            .with_conn(|conn| {
                let pk = crate::db::discussion_sessions::create_session(
                    conn,
                    "d-future-ack",
                    "ClaudeCode",
                    Some("cli-f"),
                    "peer",
                )?;
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order)
                     VALUES ('u-old', 'd-future-ack', 'User', 'seen turn', ?1, 1)",
                    rusqlite::params![now],
                )?;
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order)
                     VALUES ('u-wake-1', 'd-future-ack', 'User', 'ping', ?1, 2)",
                    rusqlite::params![now],
                )?;
                crate::db::discussions::replace_message_targets(
                    conn,
                    "u-wake-1",
                    &[crate::models::MessageTarget::cli(
                        crate::models::AgentType::ClaudeCode,
                        pk,
                    )],
                )?;
                Ok(pk)
            })
            .await
            .unwrap();

        // Absurdly large ack BEFORE any offer: clamps to offered=0, a no-op.
        let _ = wait_for_peer(
            State(state.clone()),
            Path("d-future-ack".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: Some("cli-f".to_string()),
                conversation_id: None,
                ack_awareness_upto: Some(1_000_000_000),
            }),
        )
        .await;

        // New room traffic AFTER the oversized ack…
        state
            .db
            .with_conn(move |conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order)
                     VALUES ('u-later', 'd-future-ack', 'User', 'later context', ?1, 3)",
                    rusqlite::params![now],
                )?;
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order)
                     VALUES ('u-wake-2', 'd-future-ack', 'User', 'ping 2', ?1, 4)",
                    rusqlite::params![now],
                )?;
                crate::db::discussions::replace_message_targets(
                    conn,
                    "u-wake-2",
                    &[crate::models::MessageTarget::cli(
                        crate::models::AgentType::ClaudeCode,
                        session_pk,
                    )],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        // …must still arrive as awareness with the next wake.
        let woken = wait_for_peer(
            State(state),
            Path("d-future-ack".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(3),
                timeout_secs: Some(1),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: Some("cli-f".to_string()),
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();
        assert!(woken
            .messages
            .iter()
            .any(|m| m.message_id == "u-later" && m.awareness));
    }

    #[tokio::test]
    async fn turns_for_another_responder_are_awareness_and_never_wake() {
        // KT-189 sealed contract: `@` picks who WAKES and answers; it does
        // not make the turn invisible to the other participants. A turn
        // addressed to another responder reaches this CLI as awareness on
        // its next wake — and an awareness-only window never ends the wait.
        let state = make_state_with_disc("d-other-responder").await;
        let session_pk = state
            .db
            .with_conn(|conn| {
                let pk = crate::db::discussion_sessions::create_session(
                    conn,
                    "d-other-responder",
                    "ClaudeCode",
                    Some("cli-obs"),
                    "peer",
                )?;
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order)
                     VALUES ('u-h', 'd-other-responder', 'User', 'context turn', ?1, 1)",
                    rusqlite::params![now],
                )?;
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order)
                     VALUES ('u-codex', 'd-other-responder', 'User',
                         'belongs to codex', ?1, 2)",
                    rusqlite::params![now],
                )?;
                crate::db::discussions::replace_message_targets(
                    conn,
                    "u-codex",
                    &[crate::models::MessageTarget::agent(
                        crate::models::AgentType::Codex,
                    )],
                )?;
                Ok(pk)
            })
            .await
            .unwrap();

        // Nothing addresses this CLI: the wait must time out with NOTHING —
        // awareness alone never wakes a model.
        let quiet = wait_for_peer(
            State(state.clone()),
            Path("d-other-responder".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: Some("cli-obs".to_string()),
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();
        assert!(quiet.timed_out);
        assert!(quiet.messages.is_empty());
        assert_eq!(quiet.withheld_by_routing, 2);

        // A wake then carries BOTH earlier turns as awareness, including the
        // one addressed to the other responder (visible, never actionable).
        state
            .db
            .with_conn(move |conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order)
                     VALUES ('u-wake', 'd-other-responder', 'User', 'your turn', ?1, 3)",
                    rusqlite::params![now],
                )?;
                crate::db::discussions::replace_message_targets(
                    conn,
                    "u-wake",
                    &[crate::models::MessageTarget::cli(
                        crate::models::AgentType::ClaudeCode,
                        session_pk,
                    )],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let woken = wait_for_peer(
            State(state),
            Path("d-other-responder".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(2),
                timeout_secs: Some(1),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: Some("cli-obs".to_string()),
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();
        assert!(!woken.timed_out);
        let awareness: Vec<_> = woken.messages.iter().filter(|m| m.awareness).collect();
        assert_eq!(
            awareness
                .iter()
                .map(|m| m.message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["u-h", "u-codex"],
        );
        assert!(
            awareness.iter().all(|m| !m.addressed_to_caller),
            "awareness is context, never actionable"
        );
        assert!(woken
            .messages
            .iter()
            .any(|m| m.message_id == "u-wake" && !m.awareness));
    }

    #[tokio::test]
    async fn wait_for_peer_distinguishes_two_same_provider_cli_authors() {
        let state = make_state_with_disc("d-wait-same-provider").await;
        let codex_b = state
            .db
            .with_conn(|conn| {
                let codex_a = crate::db::discussion_sessions::create_session(
                    conn,
                    "d-wait-same-provider",
                    "Codex",
                    Some("codex-a"),
                    "peer",
                )?;
                let codex_b = crate::db::discussion_sessions::create_session(
                    conn,
                    "d-wait-same-provider",
                    "Codex",
                    Some("codex-b"),
                    "peer",
                )?;
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (
                         id, discussion_id, role, content, agent_type,
                         timestamp, sort_order, target_agent
                     ) VALUES (
                         'msg-from-codex-b', 'd-wait-same-provider', 'Agent',
                         'exact reply from B', 'Codex', ?1, 3, 'Codex'
                     )",
                    rusqlite::params![now],
                )?;
                crate::db::discussions::set_message_cli_author(conn, "msg-from-codex-b", codex_b)?;
                crate::db::discussions::replace_message_targets(
                    conn,
                    "msg-from-codex-b",
                    &[MessageTarget::cli(crate::models::AgentType::Codex, codex_a)],
                )?;
                Ok(codex_b)
            })
            .await
            .unwrap();

        let addressed = wait_for_peer(
            State(state.clone()),
            Path("d-wait-same-provider".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("Codex".to_string()),
                session_id: Some("codex-a".to_string()),
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();
        assert!(!addressed.timed_out);
        assert_eq!(addressed.messages.len(), 1);
        assert!(addressed.messages[0].addressed_to_caller);
        assert_eq!(
            addressed.messages[0].reply_target,
            Some(MessageTarget::cli(crate::models::AgentType::Codex, codex_b))
        );

        let author = wait_for_peer(
            State(state),
            Path("d-wait-same-provider".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("Codex".to_string()),
                session_id: Some("codex-b".to_string()),
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await
        .0
        .data
        .unwrap();
        assert!(
            author.timed_out,
            "only the exact author session filters its own append"
        );
        assert!(author.messages.is_empty());
        assert_eq!(
            author.latest_sort_order, 3,
            "filtering an own message still advances the durable cursor"
        );
    }

    #[tokio::test]
    async fn wait_for_peer_delivers_target_all_expansion_to_every_joined_cli() {
        let state = make_state_with_disc("d-wait-all-cli").await;
        state
            .db
            .with_conn(|conn| {
                let codex = crate::db::discussion_sessions::create_session(
                    conn,
                    "d-wait-all-cli",
                    "Codex",
                    Some("codex-all"),
                    "peer",
                )?;
                let claude = crate::db::discussion_sessions::create_session(
                    conn,
                    "d-wait-all-cli",
                    "ClaudeCode",
                    Some("claude-all"),
                    "peer",
                )?;
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages (
                         id, discussion_id, role, content, timestamp, sort_order
                     ) VALUES (
                         'msg-all-cli', 'd-wait-all-cli', 'User',
                         '@all validate this batch', ?1, 5
                     )",
                    rusqlite::params![now],
                )?;
                conn.execute(
                    "INSERT INTO message_targets (
                         message_id, target_kind, agent_type, cli_session_id, position
                     ) VALUES
                         ('msg-all-cli', 'cli', 'Codex', ?1, 0),
                         ('msg-all-cli', 'cli', 'ClaudeCode', ?2, 1)",
                    rusqlite::params![codex, claude],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        for (agent_type, session_id) in [("Codex", "codex-all"), ("ClaudeCode", "claude-all")] {
            let delivered = wait_for_peer(
                State(state.clone()),
                Path("d-wait-all-cli".to_string()),
                Query(WaitForPeerQuery {
                    since_sort_order: Some(0),
                    timeout_secs: Some(1),
                    exclude_agent_type: Some(agent_type.to_string()),
                    session_id: Some(session_id.to_string()),
                    conversation_id: None,
                    ack_awareness_upto: None,
                }),
            )
            .await
            .0
            .data
            .unwrap();
            assert!(!delivered.timed_out);
            assert_eq!(delivered.messages.len(), 1);
            assert_eq!(delivered.messages[0].message_id, "msg-all-cli");
            assert!(delivered.messages[0].addressed_to_caller);
            assert_eq!(delivered.withheld_by_routing, 0);
        }
    }

    #[tokio::test]
    async fn wait_for_peer_times_out_with_no_messages() {
        let state = make_state_with_disc("d-wait-3").await;
        let resp = wait_for_peer(
            State(state),
            Path("d-wait-3".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(2),
                exclude_agent_type: None,
                session_id: None,
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await;
        let data = resp.0.data.unwrap();
        assert!(data.timed_out);
        assert_eq!(data.messages.len(), 0);
        assert_eq!(data.latest_sort_order, 0);
    }

    // ─── KT-114 — late capture of the native resume id on the idle wait ──

    #[tokio::test]
    async fn wait_for_peer_persists_a_late_conversation_id_on_the_live_session() {
        let state = make_state_with_disc("d-wait-late-cid").await;
        state
            .db
            .with_conn(|conn| {
                crate::db::discussion_sessions::create_session(
                    conn,
                    "d-wait-late-cid",
                    "Codex",
                    Some("codex-late"),
                    "peer",
                )
            })
            .await
            .unwrap();

        let wait = |cid: Option<&str>| {
            let state = state.clone();
            let cid = cid.map(str::to_string);
            async move {
                wait_for_peer(
                    State(state),
                    Path("d-wait-late-cid".to_string()),
                    Query(WaitForPeerQuery {
                        since_sort_order: Some(0),
                        timeout_secs: Some(1),
                        exclude_agent_type: Some("Codex".to_string()),
                        session_id: Some("codex-late".to_string()),
                        conversation_id: cid,
                        ack_awareness_upto: None,
                    }),
                )
                .await
            }
        };
        let stored = |state: &AppState| {
            let state = state.clone();
            async move {
                state
                    .db
                    .with_conn(|conn| {
                        crate::db::discussion_sessions::list_sessions(
                            conn,
                            "d-wait-late-cid",
                            false,
                        )
                    })
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|s| s.session_id.as_deref() == Some("codex-late"))
                    .and_then(|s| s.conversation_id)
            }
        };

        // A fresh session joins with nothing to declare.
        let _ = wait(None).await;
        assert_eq!(stored(&state).await, None);

        // The bridge resolves the id later and piggybacks it on the idle poll.
        let _ = wait(Some("019fad80-1c9a-7333-96b1-c06804f91641")).await;
        assert_eq!(
            stored(&state).await.as_deref(),
            Some("019fad80-1c9a-7333-96b1-c06804f91641"),
            "the Resume button depends on this row being filled late"
        );

        // A malformed value is dropped, never stored: Kronn does not invent
        // a resumable identity out of a corrupted parameter.
        let _ = wait(Some("not-a-uuid")).await;
        assert_eq!(
            stored(&state).await.as_deref(),
            Some("019fad80-1c9a-7333-96b1-c06804f91641"),
        );
    }

    // ─── 0.8.12 PR B — presence phase 1 (server-derived activity) ────

    async fn activity_of(state: &AppState, disc_id: &str, agent: &str) -> Option<String> {
        let did = disc_id.to_string();
        let ag = agent.to_string();
        state
            .db
            .with_conn(move |conn| crate::db::discussion_sessions::list_sessions(conn, &did, false))
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.agent_type == ag)
            .and_then(|s| s.activity)
    }

    #[tokio::test]
    async fn wait_entry_sets_listening_and_empty_timeout_never_sets_reading() {
        let state = make_state_with_disc("d-act-1").await;
        state
            .db
            .with_conn(|conn| {
                crate::db::discussion_sessions::join_disc_session(
                    conn,
                    "d-act-1",
                    "ClaudeCode",
                    "s-act",
                )
                .map(|_| ())
            })
            .await
            .unwrap();

        let resp = wait_for_peer(
            State(state.clone()),
            Path("d-act-1".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: Some("s-act".to_string()),
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await;
        let data = resp.0.data.unwrap();
        assert!(data.timed_out);
        // Presence-gap fix: an EMPTY timeout now transitions listening →
        // `waiting` (dormant during the pacing pause), never a fake
        // `reading`.
        assert_eq!(
            activity_of(&state, "d-act-1", "ClaudeCode")
                .await
                .as_deref(),
            Some("waiting"),
            "empty timeout sets waiting (dormant), never a fake 'reading'",
        );
        // `next_poll_at` must be a parseable RFC3339 instant, in the future,
        // and bounded by the pacing delay (not an arbitrary far-off time).
        let npa = data.next_poll_at.expect("timeout hands back next_poll_at");
        let npa = chrono::DateTime::parse_from_rfc3339(&npa)
            .expect("next_poll_at is valid RFC3339")
            .with_timezone(&chrono::Utc);
        let ahead = (npa - chrono::Utc::now()).num_seconds();
        let max_delay = data.pacing.next_delay_seconds as i64;
        assert!(
            ahead > 0 && ahead <= max_delay + 2,
            "next_poll_at must be ~now + pacing delay ({max_delay}s), got {ahead}s ahead",
        );
    }

    #[tokio::test]
    async fn waiting_activity_expires_on_its_own() {
        // Honest expiry: a `waiting` placeholder past its TTL reads back as
        // None (read-side expiry, no reaper) — a dead process does not stay
        // "dormant" forever.
        let state = make_state_with_disc("d-wait-exp").await;
        state
            .db
            .with_conn(|conn| {
                crate::db::discussion_sessions::join_disc_session(
                    conn,
                    "d-wait-exp",
                    "ClaudeCode",
                    "s-we",
                )?;
                // TTL already elapsed (negative) → immediately expired.
                crate::db::discussion_sessions::set_session_activity(
                    conn,
                    "d-wait-exp",
                    "ClaudeCode",
                    Some("s-we"),
                    "waiting",
                    -1,
                )
            })
            .await
            .unwrap();
        assert!(
            activity_of(&state, "d-wait-exp", "ClaudeCode")
                .await
                .is_none(),
            "an expired waiting placeholder must read back as None",
        );
    }

    #[tokio::test]
    async fn wait_without_session_id_cannot_refresh_presence() {
        let state = make_state_with_disc("d-no-session").await;
        state
            .db
            .with_conn(|conn| {
                crate::db::discussion_sessions::join_disc_session(
                    conn,
                    "d-no-session",
                    "ClaudeCode",
                    "s-current",
                )?;
                conn.execute(
                    "UPDATE discussion_sessions SET last_seen = '2000-01-01T00:00:00Z'\
                     WHERE disc_id = 'd-no-session'",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO messages
                        (id, discussion_id, role, content, agent_type, timestamp, sort_order)
                     VALUES ('m-no-session', 'd-no-session', 'Agent', 'peer msg',
                             'Codex', datetime('now'), 1)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let response = wait_for_peer(
            State(state.clone()),
            Path("d-no-session".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(1),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: None,
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await;
        assert!(!response.0.data.unwrap().timed_out);

        let session = state
            .db
            .with_conn(|conn| {
                crate::db::discussion_sessions::list_sessions(conn, "d-no-session", false)
            })
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(session.last_seen.as_deref(), Some("2000-01-01T00:00:00Z"));
        assert!(session.activity.is_none());
    }

    // The real disc_append→clear path is regression-tested in
    // disc_source.rs (`append_clears_the_activity_placeholder`); here we
    // only assert the DB-level clear this handler module relies on.
    #[tokio::test]
    async fn wait_delivery_flips_to_reading_and_clear_removes_it() {
        let state = make_state_with_disc("d-act-2").await;
        state
            .db
            .with_conn(|conn| {
                let pk = crate::db::discussion_sessions::join_disc_session(
                    conn,
                    "d-act-2",
                    "ClaudeCode",
                    "s-act2",
                )?;
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO messages
                        (id, discussion_id, role, content, agent_type, timestamp, sort_order)
                     VALUES ('m-peer', 'd-act-2', 'Agent', 'peer msg', 'Codex', ?1, 5)",
                    rusqlite::params![now],
                )?;
                // KT-189: only a turn addressed to this exact CLI wakes it.
                crate::db::discussions::replace_message_targets(
                    conn,
                    "m-peer",
                    &[crate::models::MessageTarget::cli(
                        crate::models::AgentType::ClaudeCode,
                        pk,
                    )],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let resp = wait_for_peer(
            State(state.clone()),
            Path("d-act-2".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(5),
                exclude_agent_type: Some("ClaudeCode".to_string()),
                session_id: Some("s-act2".to_string()),
                conversation_id: None,
                ack_awareness_upto: None,
            }),
        )
        .await;
        let data = resp.0.data.unwrap();
        assert!(!data.timed_out && data.messages.len() == 1);
        assert!(
            data.next_poll_at.is_none(),
            "a delivery replies now, not after a pause — no next_poll_at",
        );
        let wire = serde_json::to_value(&data).expect("wait response serializes");
        assert!(
            wire.get("next_poll_at").is_some() && wire["next_poll_at"].is_null(),
            "the non-optional TypeScript field must be serialized as null, never omitted",
        );
        assert_eq!(
            activity_of(&state, "d-act-2", "ClaudeCode")
                .await
                .as_deref(),
            Some("reading"),
            "a delivering wait flips the placeholder to reading",
        );

        // The agent replies → placeholder vanishes with the message.
        state
            .db
            .with_conn(|conn| {
                crate::db::discussion_sessions::clear_session_activity(
                    conn,
                    "d-act-2",
                    "ClaudeCode",
                    Some("s-act2"),
                )
            })
            .await
            .unwrap();
        assert!(activity_of(&state, "d-act-2", "ClaudeCode").await.is_none());
    }

    #[test]
    fn wait_for_peer_timeout_clamp_constants() {
        // We can't realistically exercise the clamp end-to-end in a unit test
        // without fake time (tokio test-util isn't on). This locks the
        // constants instead — the test fails fast if someone changes them in a
        // way that violates the contract.
        assert_eq!(WAIT_TIMEOUT_DEFAULT_SECS, 60);
        // KT-43 — raised from 90 s: each returned wait is a window where the
        // agent has its turn back and may not loop again. Must stay under the
        // MCP bridge's 180 s HTTP client timeout, or a normal long wait would
        // surface as a transport error instead of `timed_out`.
        assert_eq!(WAIT_TIMEOUT_MAX_SECS, 170);
        assert_eq!(WAIT_POLL_INTERVAL_MS, 1000);
        // Default is within the [1, MAX] clamp range.
        const {
            assert!(
                WAIT_TIMEOUT_MAX_SECS < 180,
                "the bridge reads this response with a 180 s client timeout"
            );
            assert!(
                WAIT_TIMEOUT_DEFAULT_SECS >= 1
                    && WAIT_TIMEOUT_DEFAULT_SECS <= WAIT_TIMEOUT_MAX_SECS
            )
        };
    }

    // ─── list_participants — header rendering source ────────────

    #[tokio::test]
    async fn list_participants_returns_empty_for_disc_with_no_sessions() {
        // A disc created via the disc-first flow (no agent launched)
        // has zero `discussion_sessions` rows. The header must render
        // an empty list, not error out.
        let state = make_state_with_disc("d-empty").await;
        let resp = list_participants(State(state), Path("d-empty".to_string())).await;
        assert!(resp.0.success);
        assert_eq!(resp.0.data.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn list_participants_includes_active_peers_after_join() {
        // After a peer joins via token, they appear in the participants
        // list with role='peer' + status='active'. End-to-end through
        // invite → join → list.
        let state = make_state_with_disc("d-active").await;
        let invite = invite_peer(State(state.clone()), Path("d-active".to_string())).await;
        let token = invite.0.data.unwrap().token;
        let _ = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token,
                agent_type: "Codex".into(),
                session_id: "sess-X".into(),
                model: None,
                conversation_id: None,
            }),
        )
        .await;

        let resp = list_participants(State(state), Path("d-active".to_string())).await;
        let participants = resp.0.data.unwrap();
        assert_eq!(participants.len(), 1);
        assert_eq!(participants[0].agent_type, "Codex");
        assert_eq!(participants[0].role, "peer");
        assert_eq!(participants[0].status, "active");
    }

    #[tokio::test]
    async fn invite_peer_each_call_yields_distinct_token() {
        // Two invites = two tokens, both valid until consumed/expired.
        // Lets the user invite N peers without juggling a shared code.
        let state = make_state_with_disc("d-multi").await;
        let r1 = invite_peer(State(state.clone()), Path("d-multi".to_string())).await;
        let r2 = invite_peer(State(state), Path("d-multi".to_string())).await;
        let t1 = r1.0.data.unwrap().token;
        let t2 = r2.0.data.unwrap().token;
        assert_ne!(t1, t2, "every invite must generate a fresh token");
    }

    /// The two failure modes reported live: an agent that joins without reading
    /// the shared plan (and asks the human to re-explain the state), and one that
    /// joins, posts once, then goes quiet — which the human reads as "it left".
    /// The join protocol must spell both out, so assert on it rather than trust
    /// that a future edit keeps them.
    #[test]
    fn join_protocol_demands_reading_the_plan_and_staying() {
        let steps = join_next_steps("d-1", "Room", 2);

        // Read the plan, and know the tasks are writable.
        assert!(steps.contains("plan_get"), "must point at the shared plan");
        assert!(steps.contains("task_list"), "must point at the backlog");
        for tool in [
            "task_create",
            "task_update",
            "task_update_dod",
            "task_add_blocker",
        ] {
            assert!(steps.contains(tool), "must state that {tool} is allowed");
        }
        assert!(
            steps.contains("kronn-internal"),
            "must say the whole MCP surface is available, not only disc_* tools",
        );
        assert!(
            steps.contains("materially changes") && steps.contains("unchanged tasks"),
            "plan maintenance must be event-driven, never a noisy no-op rewrite",
        );
        assert!(
            steps.contains("plan_snapshot") && steps.contains("reconnect the Kronn MCP"),
            "a stale MCP tool catalogue needs an explicit read-only fallback",
        );
        // KT-76 — an agent that asks the human for a fresh token after every
        // reload is the symptom this protocol has to kill.
        assert!(
            steps.contains("disc_find_by_session")
                && steps.contains("session_bound")
                && steps.contains("DO NOT ASK FOR A NEW TOKEN"),
            "reconnection must be described as already handled by the session link",
        );
        assert!(
            steps.contains("BEFORE THE FIRST SUBSTANTIVE ACTION")
                && steps.contains("task / scope / next action")
                && steps.contains("disc_append"),
            "the room must see the agent's intent before implementation starts",
        );

        // Stay and follow: the loop, the "a timeout is not the end" rule, and the
        // explicit ban on going quiet after a summary.
        assert!(steps.contains("disc_wait_for_peer"));
        assert!(steps.contains("timed_out"));
        assert!(
            steps.contains("JOINING IS NOT THE TASK"),
            "joining must not read as the job being done",
        );
        assert!(
            steps.contains("indistinguishable from having left"),
            "must state why silence is not acceptable",
        );
    }

    #[test]
    fn join_protocol_reports_the_room_it_joined() {
        let steps = join_next_steps("disc-42", "Kronn 0.9.2", 3);
        assert!(steps.contains("disc-42"));
        assert!(steps.contains("Kronn 0.9.2"));
        assert!(steps.contains("3 active participant(s)"));
    }
}
