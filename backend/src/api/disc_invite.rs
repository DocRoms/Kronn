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

use crate::db;
use crate::models::ApiResponse;
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
    pub instruction_text: String,
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
    pub peer_count: i64,
    /// Title of the disc, surfaced in the agent's first reply so the
    /// human can verify it joined the right conversation.
    pub disc_title: String,
    /// Last N messages already in the disc (default 10). Empty for a
    /// freshly-created topic.
    pub recent_messages: Vec<RecentMessagePreview>,
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

    let token = req.token.clone();
    let agent_type = req.agent_type.clone();
    let session_id = req.session_id.clone();

    // Resolve (disc_id, session_pk): first try a LOCAL token join; on a local
    // miss, fall back to asking our contacts who hosts the room (the unified
    // "join by code"). The owning peer shares the disc back over the WS
    // federation, we mirror it, and bind a session to the mirror.
    let (disc_id, session_pk) = {
        let (t, a, s) = (token.clone(), agent_type.clone(), session_id.clone());
        let local = state
            .db
            .with_conn(move |conn| db::discussion_sessions::join_via_token(conn, &t, &a, &s))
            .await;
        match local {
            Ok(j) => (j.disc_id, j.session_pk),
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
            let peer_count = db::discussion_sessions::count_active_participants(
                conn,
                &disc_id,
            )?;

            // Step 3 — recent messages (last 10, trimmed). Newest last
            // so the agent can read top→bottom.
            let mut stmt = conn.prepare(
                "SELECT sort_order, role, agent_type, content, timestamp
                   FROM messages
                  WHERE discussion_id = ?1
                  ORDER BY sort_order DESC
                  LIMIT 10",
            )?;
            let mut rows: Vec<RecentMessagePreview> = stmt
                .query_map(rusqlite::params![&disc_id], |r| {
                    let content: String = r.get(3)?;
                    let preview: String =
                        content.chars().take(400).collect();
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
            let next_steps = format!(
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
                 STEP 3 (loop until task done or user says stop) :\n\
                 a. Call `disc_wait_for_peer({{timeout_secs: 90}})` to block until \
                 another agent posts something new.\n\
                 b. If it returns `timed_out: true` with NO new messages, that is \
                 NORMAL (the peer may still be thinking) — immediately call \
                 `disc_wait_for_peer` AGAIN. A quiet window is NOT the end of the \
                 conversation; never stop or leave just because a wait timed out.\n\
                 c. When messages arrive, read them, then call `disc_append({{content: \
                 \"<your reaction>\"}})` to reply.\n\
                 d. Go back to (a).\n\n\
                 To leave the room : `disc_leave()`. Don't leave until the task \
                 is done or the user explicitly tells you to stop.",
                disc_id, disc_title, peer_count,
            );

            Ok::<_, anyhow::Error>(PeerJoinResponse {
                poll_policy: crate::api::disc_introspection::PollBackoffPolicy::default(),
                disc_id,
                session_pk,
                peer_count,
                disc_title,
                recent_messages: rows,
                next_steps,
            })
        })
        .await;

    match res {
        Ok(r) => Json(ApiResponse::ok(r)),
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
) -> anyhow::Result<Option<(String, i64)>> {
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
        let pk = state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::join_disc_session(conn, &mdid, &a, &s)
            })
            .await?;
        return Ok(Some((mirror_disc_id, pk)));
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
            let row =
                db::discussion_sessions::find_active_session(conn, &agent_type, &session_id)?;
            let Some(s) = row else {
                return Ok(PeerLeaveResponse { left: false });
            };
            db::discussion_sessions::mark_session_left(conn, s.id)?;
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
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WaitForPeerMessage {
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
}

const WAIT_POLL_INTERVAL_MS: u64 = 1000;
const WAIT_TIMEOUT_DEFAULT_SECS: u64 = 60;
const WAIT_TIMEOUT_MAX_SECS: u64 = 90;

/// `GET /api/discussions/:id/wait`
///
/// Long-polling endpoint : sleeps in ~1s ticks, returning as soon as
/// a new message (newer than `since_sort_order`, optionally excluding
/// the caller's own `agent_type`) appears in the disc. Bounded by
/// `timeout_secs` (default 60s, max 90s).
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

    // Liveness heartbeat (migration 064). The agent's idle loop calls
    // this every ≤90s with `exclude_agent_type = its own type` (so it
    // doesn't wake on its own append) — that's exactly its identity, and
    // entering the wait is proof it's alive. Bump last_seen at the START
    // (not after the long-poll) so a crashed agent's session goes stale
    // promptly. Best-effort: a DB hiccup here must not block the wait.
    if let Some(ref agent_type) = exclude {
        let disc_id_touch = disc_id.clone();
        let agent_touch = agent_type.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_sessions::touch_session_by_agent(conn, &disc_id_touch, &agent_touch)
            })
            .await
        {
            tracing::warn!("wait_for_peer: failed to bump session heartbeat: {e}");
        }
    }

    loop {
        let disc_id_clone = disc_id.clone();
        let exclude_clone = exclude.clone();
        let messages: anyhow::Result<Vec<WaitForPeerMessage>> = state
            .db
            .with_conn(move |conn| {
                // Pull every message after `since` ; filter the
                // exclude_agent_type in Rust to avoid threading an
                // Option<String> through the SQL binder.
                let mut stmt = conn.prepare(
                    "SELECT sort_order, role, agent_type, content, timestamp, author_pseudo
                       FROM messages
                      WHERE discussion_id = ?1 AND sort_order > ?2
                      ORDER BY sort_order ASC",
                )?;
                let rows: Vec<WaitForPeerMessage> = stmt
                    .query_map(rusqlite::params![&disc_id_clone, since], |r| {
                        Ok(WaitForPeerMessage {
                            sort_order: r.get(0)?,
                            role: r.get(1)?,
                            agent_type: r.get(2)?,
                            content: r.get(3)?,
                            timestamp: r.get(4)?,
                            author_pseudo: r.get(5)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let filtered = rows
                    .into_iter()
                    // Exclude only OUR OWN local appends (same agent_type AND no
                    // author_pseudo). A federated peer message carries an
                    // author_pseudo, so a same-agent_type peer (another
                    // ClaudeCode instance across the wire) is NOT filtered —
                    // otherwise two ClaudeCode peers go deaf to each other.
                    .filter(|m| match (&exclude_clone, &m.agent_type) {
                        (Some(ex), Some(ag)) if ex == ag => m.author_pseudo.is_some(),
                        _ => true,
                    })
                    .collect();
                Ok(filtered)
            })
            .await;

        let messages = match messages {
            Ok(m) => m,
            Err(e) => return Json(ApiResponse::err(format!("wait_for_peer db error: {e}"))),
        };

        if !messages.is_empty() {
            let latest_sort_order = messages.iter().map(|m| m.sort_order).max().unwrap_or(since);
            return Json(ApiResponse::ok(WaitForPeerResponse {
                timed_out: false,
                messages,
                latest_sort_order,
            }));
        }

        if std::time::Instant::now() >= deadline {
            return Json(ApiResponse::ok(WaitForPeerResponse {
                timed_out: true,
                messages: vec![],
                latest_sort_order: since,
            }));
        }

        sleep(Duration::from_millis(WAIT_POLL_INTERVAL_MS)).await;
    }
}

/// `GET /api/discussions/:id/participants`
///
/// Returns the active+paused participants of a disc — what the
/// header renders as small agent icons next to the disc title.
/// `left` sessions are excluded (audit history only).
pub async fn list_participants(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
) -> Json<ApiResponse<Vec<db::discussion_sessions::DiscussionSession>>> {
    let res = state
        .db
        .with_conn(move |conn| {
            db::discussion_sessions::list_sessions(conn, &disc_id, false)
        })
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
                return Err(anyhow::anyhow!(
                    "discussion `{disc_id_for_db}` not found"
                ));
            }
            db::discussion_sessions::create_invite_token(conn, &disc_id_for_db)
        })
        .await;

    let issued = match issued {
        Ok(i) => i,
        Err(e) => return Json(ApiResponse::err(e.to_string())),
    };

    let instruction_text = format!(
        "Joins-toi à cette discussion Kronn en appelant l'outil MCP : disc_join({{token: \"{}\"}})",
        issued.token
    );

    Json(ApiResponse::ok(InviteResponse {
        token: issued.token,
        disc_id: issued.disc_id,
        expires_at: issued.expires_at,
        ttl_seconds: db::discussion_sessions::INVITE_TTL_SECS,
        instruction_text,
    }))
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
            return Json(ApiResponse::err("unknown peer (invite code not in contacts)"));
        }
        Ok(crate::db::contacts::InviteAuth::Unknown) => {
            return Json(ApiResponse::err("unknown peer (invite code not in contacts)"))
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
            crate::db::discussions::update_discussion_sharing(conn, &did, &shared_id, &shared_with)?;
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
            return Json(ApiResponse::err("unknown peer (invite code not in contacts)"));
        }
        Ok(crate::db::contacts::InviteAuth::Unknown) => {
            return Json(ApiResponse::err("unknown peer (invite code not in contacts)"))
        }
        Err(e) => return Json(ApiResponse::err(format!("contact lookup error: {e}"))),
    };

    let file_id = req.file_id.clone();
    let cf = match state
        .db
        .with_conn(move |conn| crate::db::discussions::get_context_file(conn, &file_id).map_err(|e| anyhow::anyhow!(e)))
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
        state.db.with_conn(move |conn| {
            crate::db::contacts::insert_contact(conn, &crate::models::Contact {
                id: "c-1".into(),
                pseudo: "peer".into(),
                avatar_email: None,
                kronn_url: "http://peer.local".into(),
                invite_code: "kr-inv-abc".into(),
                status: "accepted".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })?;
            crate::db::discussions::insert_federated_context_file(
                conn, "f-1", "d-fetch-1", "m-1", "doc.pdf", "application/pdf", 5, &blob_str,
            ).map_err(|e| anyhow::anyhow!(e))?;
            Ok(())
        }).await.unwrap();

        // Known contact, but the discussion is NOT shared with it → found: false.
        let resp = fetch_file(State(state.clone()), Json(FetchFileRequest {
            file_id: "f-1".into(),
            from_invite_code: "kr-inv-abc".into(),
        })).await;
        let body = resp.0.data.expect("ok envelope (no existence oracle)");
        assert!(!body.found, "unshared discussion must not leak files");
        assert!(body.data_base64.is_none());

        // Share the discussion with that contact → the bytes flow.
        state.db.with_conn(|conn| {
            crate::db::discussions::update_discussion_sharing(conn, "d-fetch-1", "sh-1", &["c-1".to_string()]).map(|_| ())
        }).await.unwrap();
        let resp = fetch_file(State(state.clone()), Json(FetchFileRequest {
            file_id: "f-1".into(),
            from_invite_code: "kr-inv-abc".into(),
        })).await;
        let body = resp.0.data.unwrap();
        assert!(body.found);
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(body.data_base64.unwrap()).unwrap();
        assert_eq!(bytes, b"BYTES");
    }

    #[tokio::test]
    async fn non_accepted_contacts_are_refused_like_unknown_codes() {
        // Passe D (Codex constat n°1) — a pending/refused contact keeps its
        // invite code but must NOT pass the auth-exempt P2P routes, and the
        // refusal must be indistinguishable from an unknown code (no oracle).
        let state = make_state_with_disc("d-auth-1").await;
        state.db.with_conn(|conn| {
            for (id, code, status) in [
                ("c-pend", "kr-inv-pend", "pending"),
                ("c-ref", "kr-inv-ref", "refused"),
            ] {
                crate::db::contacts::insert_contact(conn, &crate::models::Contact {
                    id: id.into(),
                    pseudo: format!("peer-{status}"),
                    avatar_email: None,
                    kronn_url: "http://peer.local".into(),
                    invite_code: code.into(),
                    status: status.into(),
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                })?;
            }
            Ok(())
        }).await.unwrap();

        for code in ["kr-inv-pend", "kr-inv-ref", "kr-inv-ghost"] {
            // fetch-file: same error string for pending, refused and unknown.
            let resp = fetch_file(State(state.clone()), Json(FetchFileRequest {
                file_id: "f-any".into(),
                from_invite_code: code.into(),
            })).await;
            assert_eq!(
                resp.0.error.as_deref(),
                Some("unknown peer (invite code not in contacts)"),
                "{code} must be refused with the unknown-code message"
            );

            // claim-by-token: same contract.
            let resp = claim_by_token(State(state.clone()), Json(ClaimByTokenRequest {
                token: "kr-join-whatever".into(),
                from_invite_code: code.into(),
            })).await;
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
        assert_eq!(
            data.ttl_seconds,
            db::discussion_sessions::INVITE_TTL_SECS
        );
        assert!(data.instruction_text.contains(&data.token));
        assert!(data.instruction_text.contains("disc_join"));
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
        let invite_resp =
            invite_peer(State(state.clone()), Path("d-join-1".to_string())).await;
        let token = invite_resp.0.data.unwrap().token;

        let join_resp = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token,
                agent_type: "Codex".into(),
                session_id: "sess-cdx-1".into(),
            }),
        )
        .await;
        let body = join_resp.0;
        assert!(body.success, "got error: {:?}", body.error);
        let data = body.data.unwrap();
        assert_eq!(data.disc_id, "d-join-1");
        assert!(data.session_pk > 0);
        assert_eq!(data.peer_count, 1, "exactly the joining session is active");
        assert_eq!(data.disc_title, "Test disc");
        assert_eq!(data.recent_messages.len(), 0, "empty disc → no previews");
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
            },
            PeerJoinRequest {
                token: "kr-join-x".into(),
                agent_type: "".into(),
                session_id: "s".into(),
            },
            PeerJoinRequest {
                token: "kr-join-x".into(),
                agent_type: "Codex".into(),
                session_id: "".into(),
            },
        ] {
            let resp = peer_join(State(state.clone()), Json(bad)).await;
            assert!(!resp.0.success);
        }
    }

    #[tokio::test]
    async fn peer_join_multi_use_within_ttl() {
        // 0.8.6 fix 2026-05-21 — token is multi-use within TTL. The
        // route contract must let N peers join with the same token,
        // up to expiry. UX win : user clicks [+ Inviter] once for
        // the whole multi-agent room (3 agents = 1 invite instead
        // of 3).
        let state = make_state_with_disc("d-join-4").await;
        let invite =
            invite_peer(State(state.clone()), Path("d-join-4".to_string())).await;
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
        let inv1 =
            invite_peer(State(state.clone()), Path("d-e2e".to_string())).await;
        let token_a = inv1.0.data.unwrap().token;
        let join_a = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token: token_a,
                agent_type: "ClaudeCode".into(),
                session_id: "sess-A".into(),
            }),
        )
        .await;
        assert!(join_a.0.success, "agent A join failed: {:?}", join_a.0.error);
        let join_a_data = join_a.0.data.unwrap();
        assert_eq!(join_a_data.peer_count, 1);

        // Header shows 1 active participant : agent A.
        let parts1 =
            list_participants(State(state.clone()), Path("d-e2e".to_string())).await;
        let p1 = parts1.0.data.unwrap();
        assert_eq!(p1.len(), 1);
        assert_eq!(p1[0].agent_type, "ClaudeCode");

        // ── Step 3: agent A writes a message ──
        insert_message(&state, "d-e2e", "msg-1", 1, "ClaudeCode", "hello, anyone here ?").await;

        // ── Step 4: agent B (Codex) joins via invite #2 ──
        let inv2 =
            invite_peer(State(state.clone()), Path("d-e2e".to_string())).await;
        let token_b = inv2.0.data.unwrap().token;
        let join_b = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token: token_b,
                agent_type: "Codex".into(),
                session_id: "sess-B".into(),
            }),
        )
        .await;
        assert!(join_b.0.success, "agent B join failed: {:?}", join_b.0.error);
        let join_b_data = join_b.0.data.unwrap();
        assert_eq!(join_b_data.peer_count, 2, "both A and B now active");
        // join() returns recent_messages — agent B sees agent A's hello.
        assert_eq!(join_b_data.recent_messages.len(), 1);
        assert!(join_b_data.recent_messages[0].preview.contains("hello"));

        // Header now shows 2 active participants.
        let parts2 =
            list_participants(State(state.clone()), Path("d-e2e".to_string())).await;
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
        let parts3 =
            list_participants(State(state.clone()), Path("d-e2e".to_string())).await;
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
        let parts4 =
            list_participants(State(state.clone()), Path("d-e2e".to_string())).await;
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
            let inv = invite_peer(
                State(state.clone()),
                Path("d-e2e-multi".to_string()),
            )
            .await;
            let token = inv.0.data.unwrap().token;
            let join = peer_join(
                State(state.clone()),
                Json(PeerJoinRequest {
                    token,
                    agent_type: agent.into(),
                    session_id: sess.into(),
                }),
            )
            .await;
            assert!(join.0.success, "agent {} could not join: {:?}", agent, join.0.error);
            joined += 1;
        }
        assert_eq!(joined, 3);
        let parts =
            list_participants(State(state), Path("d-e2e-multi".to_string())).await;
        assert_eq!(parts.0.data.unwrap().len(), 3);
    }

    // ─── peer_leave (0.8.6 phase 3) ────────────────────────────

    #[tokio::test]
    async fn peer_leave_marks_active_session_left_and_is_idempotent() {
        let state = make_state_with_disc("d-leave-1").await;
        let invite =
            invite_peer(State(state.clone()), Path("d-leave-1".to_string())).await;
        let token = invite.0.data.unwrap().token;
        let _ = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token,
                agent_type: "Codex".into(),
                session_id: "sess-Z".into(),
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
        let parts =
            list_participants(State(state), Path("d-leave-1".to_string())).await;
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
    async fn wait_for_peer_times_out_with_no_messages() {
        let state = make_state_with_disc("d-wait-3").await;
        let resp = wait_for_peer(
            State(state),
            Path("d-wait-3".to_string()),
            Query(WaitForPeerQuery {
                since_sort_order: Some(0),
                timeout_secs: Some(2),
                exclude_agent_type: None,
            }),
        )
        .await;
        let data = resp.0.data.unwrap();
        assert!(data.timed_out);
        assert_eq!(data.messages.len(), 0);
        assert_eq!(data.latest_sort_order, 0);
    }

    #[test]
    fn wait_for_peer_timeout_clamp_constants() {
        // We can't realistically exercise the 90s clamp end-to-end in
        // a unit test without fake time (tokio test-util isn't on).
        // This locks the constants instead — the test fails fast if
        // someone changes them in a way that violates the contract.
        assert_eq!(WAIT_TIMEOUT_DEFAULT_SECS, 60);
        assert_eq!(WAIT_TIMEOUT_MAX_SECS, 90);
        assert_eq!(WAIT_POLL_INTERVAL_MS, 1000);
        // Default is within the [1, MAX] clamp range.
        const { assert!(WAIT_TIMEOUT_DEFAULT_SECS >= 1 && WAIT_TIMEOUT_DEFAULT_SECS <= WAIT_TIMEOUT_MAX_SECS) };
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
        let invite =
            invite_peer(State(state.clone()), Path("d-active".to_string())).await;
        let token = invite.0.data.unwrap().token;
        let _ = peer_join(
            State(state.clone()),
            Json(PeerJoinRequest {
                token,
                agent_type: "Codex".into(),
                session_id: "sess-X".into(),
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
}
