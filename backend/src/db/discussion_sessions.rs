//! 0.8.6 phase 2 — Discussion-first session bindings.
//!
//! Helpers for `discussion_sessions` + `discussion_invite_tokens`
//! (migration 060). Powers the multi-agent collab flow : disc lives
//! independently of its participants ; each CLI session is a row in
//! `discussion_sessions`, joined via invite tokens or upfront when an
//! agent is launched from the UI.
//!
//! Why a separate module from `disc_source.rs` ? — that file owns the
//! 0.8.4 cross-agent-memory schema (`discussions.source_*` columns +
//! `disc_source_history` linear chain). The new schema is orthogonal :
//! it tracks LIVE participation (multiple parallel sessions per disc)
//! rather than the linear "this disc was last bound to session X"
//! lineage. Both will coexist in 0.8.6 ; 0.9.0 may consolidate.
//!
//! Token security model : plaintext only ever returned by
//! `create_invite_token` (single response, single use). DB stores
//! SHA-256(token). A leaked DB row can't be replayed.
//!
//! See `project_cross_agent_collab_demo.md` in memory for the wider
//! design (form simplification, `[+ Inviter]` header button, etc.).
use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;
use uuid::Uuid;

/// A row of `discussion_sessions` — one live (or historical)
/// binding between a disc and a CLI session.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionSession {
    pub id: i64,
    pub disc_id: String,
    pub agent_type: String,
    pub session_id: Option<String>,
    pub role: String,
    pub status: String,
    pub joined_at: String,
    pub left_at: Option<String>,
    /// Last activity heartbeat (migration 064): bumped on every `disc_append`.
    /// Surfaced so the UI can show presence freshness — "active" vs "silent
    /// (working)" vs "away" — instead of a binary present/absent that makes a
    /// quietly-working peer look like a dead room. `None` for rows predating 064.
    pub last_seen: Option<String>,
    /// 0.8.12 PR B — server-derived activity placeholder: `"listening"`
    /// (open wait long-poll) or `"reading"` (messages delivered, no reply
    /// yet). Expiry is applied at read time — an expired activity is None
    /// here, callers never see a stale placeholder.
    pub activity: Option<String>,
}

/// Metadata returned by `create_invite_token`. The plain `token` field
/// is the ONLY place the plaintext value ever lives outside the agent
/// that's about to consume it — never logged, never persisted.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InviteTokenIssued {
    pub token: String,
    pub disc_id: String,
    pub expires_at: String,
}

/// One invite-token row, hash-only. Returned by audit-style queries
/// (who joined via which invite). Never carries plaintext.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InviteTokenRecord {
    pub id: i64,
    pub disc_id: String,
    pub created_at: String,
    pub expires_at: String,
    pub used_at: Option<String>,
    pub used_by_session_id: Option<i64>,
}

// ─── discussion_sessions CRUD ───────────────────────────────────────

/// Insert a new participant row. Returns the rowid the caller can use
/// to update status later (pause, leave).
///
/// `session_id` is allowed to be `None` only briefly during invite
/// acceptance (between token validation and the agent's first heartbeat
/// — there's a partial unique index guarding against duplicates once
/// a real id is in).
pub fn create_session(
    conn: &Connection,
    disc_id: &str,
    agent_type: &str,
    session_id: Option<&str>,
    role: &str,
) -> Result<i64> {
    if role != "owner" && role != "peer" {
        return Err(anyhow!("invalid role `{role}` — expected owner|peer"));
    }
    let now = Utc::now().to_rfc3339();
    // Seed last_seen at join (migration 064) so a freshly-joined agent
    // counts as a live responder immediately — before its first
    // disc_wait_for_peer heartbeat — closing the window where a human
    // message right after join would still trigger Kronn's auto-response.
    conn.execute(
        "INSERT INTO discussion_sessions
            (disc_id, agent_type, session_id, role, status, joined_at, last_seen)
         VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?5)",
        params![disc_id, agent_type, session_id, role, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Mark a session as having left the disc. Idempotent — re-calling on
/// an already-`left` row is a no-op (no error). Used by `disc_leave`
/// and the cleanup path when a session times out.
pub fn mark_session_left(conn: &Connection, session_pk: i64) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE discussion_sessions
            SET status = 'left', left_at = COALESCE(left_at, ?2)
          WHERE id = ?1",
        params![session_pk, now],
    )?;
    Ok(())
}

/// Toggle between 'active' and 'paused'. Used by the UI "pause this
/// agent" button + the runner when it observes a CLI go idle.
pub fn set_session_status(conn: &Connection, session_pk: i64, status: &str) -> Result<()> {
    if status != "active" && status != "paused" && status != "left" {
        return Err(anyhow!("invalid status `{status}`"));
    }
    let now = Utc::now().to_rfc3339();
    let left_at_set = status == "left";
    conn.execute(
        "UPDATE discussion_sessions
            SET status = ?2,
                left_at = CASE WHEN ?3 THEN COALESCE(left_at, ?4) ELSE left_at END
          WHERE id = ?1",
        params![session_pk, status, left_at_set, now],
    )?;
    Ok(())
}

/// List participants for a disc, ordered by `joined_at`. Default
/// `include_left=false` matches the header rendering (we only want
/// active+paused). Audit views pass `true`.
pub fn list_sessions(
    conn: &Connection,
    disc_id: &str,
    include_left: bool,
) -> Result<Vec<DiscussionSession>> {
    let sql = if include_left {
        "SELECT id, disc_id, agent_type, session_id, role, status, joined_at, left_at, last_seen, activity, activity_expires_at
           FROM discussion_sessions
          WHERE disc_id = ?1
          ORDER BY joined_at ASC"
    } else {
        "SELECT id, disc_id, agent_type, session_id, role, status, joined_at, left_at, last_seen, activity, activity_expires_at
           FROM discussion_sessions
          WHERE disc_id = ?1 AND status != 'left'
          ORDER BY joined_at ASC"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params![disc_id], |r| {
            Ok(DiscussionSession {
                id: r.get(0)?,
                disc_id: r.get(1)?,
                agent_type: r.get(2)?,
                session_id: r.get(3)?,
                role: r.get(4)?,
                status: r.get(5)?,
                joined_at: r.get(6)?,
                left_at: r.get(7)?,
                last_seen: r.get(8)?,
                activity: activity_of_row(r.get(9)?, r.get(10)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Resolve the disc currently bound to a (agent_type, session_id)
/// pair. Used by the bridge when it gets a tool call and needs to
/// verify the caller is still an active participant. Returns the
/// session row so callers can also see `role` / `status`.
pub fn find_active_session(
    conn: &Connection,
    agent_type: &str,
    session_id: &str,
) -> Result<Option<DiscussionSession>> {
    let row = conn
        .query_row(
            "SELECT id, disc_id, agent_type, session_id, role, status, joined_at, left_at, last_seen, activity, activity_expires_at
               FROM discussion_sessions
              WHERE agent_type = ?1 AND session_id = ?2 AND status != 'left'
              LIMIT 1",
            params![agent_type, session_id],
            |r| {
                Ok(DiscussionSession {
                    id: r.get(0)?,
                    disc_id: r.get(1)?,
                    agent_type: r.get(2)?,
                    session_id: r.get(3)?,
                    role: r.get(4)?,
                    status: r.get(5)?,
                    joined_at: r.get(6)?,
                    left_at: r.get(7)?,
                    last_seen: r.get(8)?,
                    activity: activity_of_row(r.get(9)?, r.get(10)?),
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Count active+paused participants (the number the UI badge shows).
/// `left` sessions don't count — they're audit history only.
pub fn count_active_participants(conn: &Connection, disc_id: &str) -> Result<i64> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM discussion_sessions
          WHERE disc_id = ?1 AND status != 'left'",
        params![disc_id],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// A session that hasn't heartbeated (`last_seen`, migration 064) in this
/// long is considered ABANDONED — the agent crashed or dropped without
/// `disc_leave`. This is deliberately NOT a per-message liveness window:
/// an earlier 300s window (2026-06-04) proved far too aggressive — a
/// turn-based CLI peer idles well over 5min between human turns, so it read
/// as dead and Kronn double-replied (observed live on disc ca495847,
/// 2026-06-08). 24h is longer than any realistic turn gap, so a genuinely
/// present peer is never reaped, while a long-dead ghost eventually stops
/// pinning presence-sticky `count_live_participants`.
pub const SESSION_ABANDON_SECS: i64 = 86_400;

/// Reaping keys off wall-clock deltas, so a badly wrong system clock could
/// mass-retire LIVE sessions. If `now` is more than this far AHEAD of the newest
/// recorded session activity, we treat it as garbage/skew and skip the pass.
/// Set to 10 years — beyond Kronn's own existence, so any *genuine* absence still
/// reaps on a sane clock, and only a nonsensical future timestamp trips it. (The
/// common real skews — dead RTC / VM restore — move the clock BACKWARD instead,
/// caught by the separate `now < newest` check.)
pub const SESSION_SKEW_SANITY_DAYS: i64 = 3650;

/// Bump `last_seen = now` for the live session of (disc_id, agent_type)
/// (migration 064 heartbeat). Called whenever an agent proves it's alive
/// — every `disc_wait_for_peer` long-poll (idle loop) and `disc_append`
/// (posting). Only touches `status='active'` rows; a paused/left session
/// isn't a live responder so its heartbeat is irrelevant. No-op (0 rows)
/// when the caller isn't a tracked participant (e.g. a Kronn-launched
/// agent with no session row) — harmless.
pub fn touch_session_by_agent(conn: &Connection, disc_id: &str, agent_type: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE discussion_sessions
            SET last_seen = ?3
          WHERE disc_id = ?1 AND agent_type = ?2 AND status = 'active'",
        params![disc_id, agent_type, now],
    )?;
    Ok(())
}

/// 0.8.12 PR B — presence phase 1. Set the server-derived activity of an
/// agent's active session: `"listening"` (an open wait_for_peer) or
/// `"reading"` (a wait just delivered messages, no reply posted yet).
/// The TTL is declarative — readers treat an expired activity as absent
/// (`activity_of_row`), so nothing ever needs reaping.
///
/// `session_id` scopes the write to ONE session row (Copilot review: two
/// concurrent sessions of the same agent type — multi-machine — must not
/// inherit each other's placeholder). `None` = agent_type granularity,
/// the compat path for older bridges that don't send their session id.
pub fn set_session_activity(
    conn: &Connection,
    disc_id: &str,
    agent_type: &str,
    session_id: Option<&str>,
    activity: &str,
    ttl_secs: i64,
) -> Result<()> {
    let expires = (Utc::now() + chrono::Duration::seconds(ttl_secs)).to_rfc3339();
    conn.execute(
        "UPDATE discussion_sessions
            SET activity = ?3, activity_expires_at = ?4
          WHERE disc_id = ?1 AND agent_type = ?2 AND status = 'active'
            AND (?5 IS NULL OR session_id = ?5)",
        params![disc_id, agent_type, activity, expires, session_id],
    )?;
    Ok(())
}

/// Clear the activity — the agent replied (`disc_append`) or left: the
/// placeholder must vanish the instant its cause disappears. Same
/// session scoping as the setter; a broad clear (None) is the SAFE
/// direction on paths that can't know the session (a sibling session's
/// label reappears at its next wait ≤90s).
pub fn clear_session_activity(
    conn: &Connection,
    disc_id: &str,
    agent_type: &str,
    session_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE discussion_sessions
            SET activity = NULL, activity_expires_at = NULL
          WHERE disc_id = ?1 AND agent_type = ?2
            AND (?3 IS NULL OR session_id = ?3)",
        params![disc_id, agent_type, session_id],
    )?;
    Ok(())
}

/// Expiry evaluated at READ time: an activity past its TTL reads as None —
/// a crashed agent's "listening" dies on its own, no background job.
/// Real DateTime comparison (Copilot review: lexicographic RFC3339 breaks
/// on format drift); an unparseable timestamp reads as EXPIRED.
fn activity_of_row(activity: Option<String>, expires_at: Option<String>) -> Option<String> {
    let act = activity?;
    let exp = expires_at?;
    let exp_dt = chrono::DateTime::parse_from_rfc3339(&exp).ok()?.with_timezone(&Utc);
    (exp_dt > Utc::now()).then_some(act)
}

/// Count LIVE responders = MCP-joined agents currently `status='active'`
/// (NOT `paused`, NOT `left`) on this disc. Used by `send_message` to
/// suppress the double-responder bug: when ≥1 agent is connected, Kronn
/// must NOT auto-spawn its own runner on a human message — the connected
/// agent answers (the user message is persisted + broadcast, so the peer
/// picks it up via its own loop or when the human relays it).
///
/// PRESENCE-STICKY (2026-06-08, user decision): a session counts as long as
/// it is `active`, with NO time-since-heartbeat window. The earlier 300s
/// staleness window broke turn-based collaboration — a CLI peer that idles
/// more than 5 minutes between human turns read as dead and Kronn
/// double-replied (reproduced on disc ca495847: user msg at 14:05 → Kronn
/// native reply 14:05:50 AND the CLI peer's MCP reply 14:06:38, because the
/// peer's last_seen was 48min old). Crashed-peer safety is handled OUT of
/// band, not by shrinking this window:
///   (a) [`reap_abandoned_sessions`] retires sessions idle > 24h, and
///   (b) the user can always force a reply via `POST /run` (`run_agent`),
///       which is intentionally NOT gated.
/// `paused` is excluded on purpose: a paused agent won't reply → Kronn
/// SHOULD still auto-respond if every connected agent is paused.
pub fn count_live_participants(conn: &Connection, disc_id: &str) -> Result<i64> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM discussion_sessions
          WHERE disc_id = ?1 AND status = 'active'",
        params![disc_id],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// Garbage-collect abandoned sessions: mark every `status='active'` row
/// whose `last_seen` (falling back to `joined_at` for pre-heartbeat rows)
/// is older than [`SESSION_ABANDON_SECS`] as `left`. Idempotent. Run at
/// boot (and once via migration 065) so presence-sticky
/// [`count_live_participants`] is never pinned by a long-dead ghost — an
/// agent that exited without `disc_leave`. Returns the rows retired.
pub fn reap_abandoned_sessions(conn: &Connection) -> Result<u64> {
    let now_dt = Utc::now();

    // Clock-skew guard (I8). A wrong system clock (VM restored from snapshot,
    // dead RTC, WSL drift) shifts `now`, and since reaping compares `now - 24h`
    // against each session's `last_seen`, a clock that jumped far AHEAD would
    // mark every live session abandoned at once — collapsing the double-responder
    // gate so Kronn replies over live peers. Compare `now` to the newest recorded
    // activity: if the clock sits behind it (impossible under a sane clock → it
    // moved backward) or absurdly ahead of it, skip this pass and warn. A stale
    // session lingering one more boot is harmless; wrongly reaping a live one is
    // not — and genuinely-live sessions re-heartbeat within ~90s, so a skipped
    // reap self-corrects on the next sane-clock pass. (Sub-threshold forward skew
    // is tolerated for the same self-healing reason.)
    let newest: Option<String> = conn.query_row(
        "SELECT MAX(COALESCE(last_seen, joined_at)) FROM discussion_sessions WHERE status = 'active'",
        [],
        |r| r.get(0),
    ).optional()?.flatten();
    if let Some(newest) = newest {
        if let Ok(newest_dt) = DateTime::parse_from_rfc3339(&newest) {
            let newest_utc = newest_dt.with_timezone(&Utc);
            if now_dt < newest_utc {
                tracing::warn!(
                    "reap: system clock ({}) is behind the newest session activity ({}) — \
                     skipping reap (suspected clock skew)", now_dt, newest_utc);
                return Ok(0);
            }
            if now_dt - newest_utc > Duration::days(SESSION_SKEW_SANITY_DAYS) {
                tracing::warn!(
                    "reap: system clock is implausibly far ahead (>{} days) of the newest \
                     session activity ({}) — skipping reap (suspected clock skew / garbage \
                     timestamp)", SESSION_SKEW_SANITY_DAYS, newest_utc);
                return Ok(0);
            }
        }
    }

    let cutoff = (now_dt - Duration::seconds(SESSION_ABANDON_SECS)).to_rfc3339();
    let now = now_dt.to_rfc3339();
    let n = conn.execute(
        "UPDATE discussion_sessions
            SET status = 'left', left_at = COALESCE(left_at, ?2)
          WHERE status = 'active'
            AND COALESCE(last_seen, joined_at) < ?1",
        params![cutoff, now],
    )?;
    Ok(n as u64)
}

// ─── invite tokens ──────────────────────────────────────────────────

/// Default TTL for an invite token. 10 minutes is the sweet spot :
/// long enough for the user to alt-tab to a terminal and paste the
/// instruction, short enough that a leaked token is basically dead
/// before an attacker could use it.
pub const INVITE_TTL_SECS: i64 = 600;

/// Generate a fresh invite token bound to `disc_id`. Returns the
/// PLAIN token (the only place it lives outside the wire response) +
/// metadata. The DB only ever sees `SHA-256(token)`.
///
/// Token shape : `kr-join-<uuid-hex-no-dashes>`. The `kr-join-`
/// prefix makes leaked tokens easy to grep for in logs / pastebins
/// (rotating policy : if you ever spot a `kr-join-…` in a public
/// place, treat the disc as compromised).
pub fn create_invite_token(conn: &Connection, disc_id: &str) -> Result<InviteTokenIssued> {
    let plain = format!("kr-join-{}", Uuid::new_v4().simple());
    let hash = sha256_hex(&plain);
    let now = Utc::now();
    let expires_at = now + Duration::seconds(INVITE_TTL_SECS);

    conn.execute(
        "INSERT INTO discussion_invite_tokens
            (token_hash, disc_id, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            hash,
            disc_id,
            now.to_rfc3339(),
            expires_at.to_rfc3339(),
        ],
    )?;
    Ok(InviteTokenIssued {
        token: plain,
        disc_id: disc_id.to_string(),
        expires_at: expires_at.to_rfc3339(),
    })
}

/// Read-only resolution of an invite token → its `disc_id`, WITHOUT consuming it
/// or creating a session. Returns `None` if the token is unknown or expired.
///
/// Used by the cross-instance **claim-by-token** flow: when `disc_join(code)`
/// misses locally, each accepted contact is asked "do you host the room behind
/// this code?" — the owner resolves it here and shares the disc back. This is
/// what unifies the two former mechanisms (local token-join vs contact-share)
/// into a single "paste a code, it just works wherever the room lives".
pub fn resolve_token_disc(conn: &Connection, plain_token: &str) -> Result<Option<String>> {
    let hash = sha256_hex(plain_token);
    let now = Utc::now().to_rfc3339();
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT disc_id, expires_at FROM discussion_invite_tokens WHERE token_hash = ?1",
            params![hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match row {
        Some((disc_id, expires_at)) if expires_at.as_str() >= now.as_str() => Some(disc_id),
        _ => None,
    })
}

/// Consume an invite token : validate it exists + is not expired,
/// then record the use (idempotent — same token CAN be reused by N
/// agents within its TTL window, that's a usability fix shipped
/// 2026-05-21 after the live tennis-match test where the user had
/// to click the invite button 3 times for 3 peers).
///
/// `used_at` + `used_by_session_id` now record the FIRST use only
/// (audit trail of who arrived first). Subsequent uses leave those
/// columns untouched — they're a free history snapshot, not a
/// uniqueness lock.
///
/// Fails with a clear message on : token not found, token expired.
pub fn consume_invite_token(
    conn: &Connection,
    plain_token: &str,
    used_by_session_id: i64,
) -> Result<String> {
    let hash = sha256_hex(plain_token);
    let now = Utc::now().to_rfc3339();

    let tx = conn.unchecked_transaction()?;

    let row: Option<(i64, String, String, Option<String>)> = tx
        .query_row(
            "SELECT id, disc_id, expires_at, used_at
               FROM discussion_invite_tokens
              WHERE token_hash = ?1",
            params![hash],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;

    let (token_id, disc_id, expires_at, used_at) = row
        .ok_or_else(|| anyhow!("invite token not found — was it copy-pasted in full ?"))?;

    if expires_at.as_str() < now.as_str() {
        return Err(anyhow!(
            "invite token expired (TTL is {} min) — ask for a fresh one",
            INVITE_TTL_SECS / 60
        ));
    }

    // First use : stamp the audit columns. Subsequent uses : leave
    // them alone (the token is multi-use within TTL).
    if used_at.is_none() {
        tx.execute(
            "UPDATE discussion_invite_tokens
                SET used_at = ?2, used_by_session_id = ?3
              WHERE id = ?1",
            params![token_id, now, used_by_session_id],
        )?;
    }
    tx.commit()?;

    Ok(disc_id)
}

/// Result of an atomic invite-token consumption + session creation.
/// Used by the `POST /api/discussions/peer-join` endpoint that
/// the `disc_join` MCP tool calls.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct JoinViaTokenResult {
    pub disc_id: String,
    pub session_pk: i64,
}

/// Atomically validate the invite token, create a peer session row,
/// and link them. Either both happen or neither does — important
/// because a half-committed state would leave a phantom participant
/// that messes up the header rendering.
///
/// 0.8.6 fix 2026-05-21 — multi-use tokens within TTL. Same token
/// can be pasted by N peers (Claude, Codex, Vibe…) within its
/// 10-min window. `used_at` + `used_by_session_id` record the
/// FIRST use only (audit trail). Live tennis-match test surfaced
/// the friction : user had to click [+ Inviter] three times to
/// invite three agents — now once is enough.
///
/// Idempotency on the SESSION side is also a concern. The same CLI
/// session shouldn't accumulate phantom rows in `discussion_sessions`
/// if it accidentally joins twice. We check for an existing active
/// session with the same `(agent_type, session_id)` and short-
/// circuit — return the existing pk + the same disc_id.
pub fn join_via_token(
    conn: &Connection,
    plain_token: &str,
    agent_type: &str,
    session_id: &str,
) -> Result<JoinViaTokenResult> {
    let hash = sha256_hex(plain_token);
    let now = Utc::now().to_rfc3339();

    let tx = conn.unchecked_transaction()?;

    // Step 1 — peek the token row.
    let row: Option<(i64, String, String, Option<String>)> = tx
        .query_row(
            "SELECT id, disc_id, expires_at, used_at
               FROM discussion_invite_tokens
              WHERE token_hash = ?1",
            params![hash],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;

    let (token_id, disc_id, expires_at, used_at) = row
        .ok_or_else(|| anyhow!("invite token not found — was it copy-pasted in full ?"))?;

    if expires_at.as_str() < now.as_str() {
        return Err(anyhow!(
            "invite token expired (TTL is {} min) — ask for a fresh one",
            INVITE_TTL_SECS / 60
        ));
    }

    // Step 2a — idempotency on the session side : if THIS CLI session
    // already joined THIS disc, return the existing pk rather than
    // inserting a duplicate row. Without this, a flaky network +
    // retry pattern would accumulate phantom participants.
    let existing_pk: Option<i64> = tx
        .query_row(
            "SELECT id FROM discussion_sessions
              WHERE disc_id = ?1 AND agent_type = ?2 AND session_id = ?3
                AND status != 'left'
              LIMIT 1",
            params![&disc_id, agent_type, session_id],
            |r| r.get(0),
        )
        .optional()?;

    let session_pk = if let Some(pk) = existing_pk {
        pk
    } else {
        // Step 2b — first join for this (agent_type, session_id) on THIS disc.
        // The partial unique index `idx_disc_sessions_session_active` is global
        // across discs, so release any active binding this session holds on a
        // DIFFERENT disc before inserting — a bridge is in one room at a time.
        tx.execute(
            "UPDATE discussion_sessions SET status = 'left', left_at = ?4
              WHERE agent_type = ?1 AND session_id = ?2 AND disc_id != ?3 AND status != 'left'",
            params![agent_type, session_id, &disc_id, now],
        )?;
        tx.execute(
            "INSERT INTO discussion_sessions
                (disc_id, agent_type, session_id, role, status, joined_at)
             VALUES (?1, ?2, ?3, 'peer', 'active', ?4)",
            params![&disc_id, agent_type, session_id, now],
        )?;
        tx.last_insert_rowid()
    };

    // Step 3 — record FIRST use of the token (audit trail). Subsequent
    // uses leave the columns untouched.
    if used_at.is_none() {
        tx.execute(
            "UPDATE discussion_invite_tokens
                SET used_at = ?2, used_by_session_id = ?3
              WHERE id = ?1",
            params![token_id, now, session_pk],
        )?;
    }

    tx.commit()?;
    Ok(JoinViaTokenResult { disc_id, session_pk })
}

/// Bind a CLI session to a disc by id, WITHOUT an invite token. Used when the
/// disc arrived as a cross-instance mirror (the `claim-by-token` flow shares a
/// disc in — the mirror carries a `shared_id` but no local token). Idempotent
/// on `(disc_id, agent_type, session_id)`: a re-join returns the existing pk
/// instead of accumulating phantom participants. Returns the session pk.
pub fn join_disc_session(
    conn: &Connection,
    disc_id: &str,
    agent_type: &str,
    session_id: &str,
) -> Result<i64> {
    let now = Utc::now().to_rfc3339();
    let existing_pk: Option<i64> = conn
        .query_row(
            "SELECT id FROM discussion_sessions
              WHERE disc_id = ?1 AND agent_type = ?2 AND session_id = ?3 AND status != 'left'
              LIMIT 1",
            params![disc_id, agent_type, session_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(pk) = existing_pk {
        return Ok(pk);
    }
    // The partial unique index `idx_disc_sessions_session_active` enforces ONE
    // active binding per (agent_type, session_id) across ALL discs. A bridge
    // session moving to a new room (e.g. joining a freshly mirrored disc while
    // still bound to an earlier one) must release its previous binding first,
    // else the INSERT below trips the unique constraint.
    conn.execute(
        "UPDATE discussion_sessions SET status = 'left', left_at = ?4
          WHERE agent_type = ?1 AND session_id = ?2 AND disc_id != ?3 AND status != 'left'",
        params![agent_type, session_id, disc_id, now],
    )?;
    conn.execute(
        "INSERT INTO discussion_sessions
            (disc_id, agent_type, session_id, role, status, joined_at)
         VALUES (?1, ?2, ?3, 'peer', 'active', ?4)",
        params![disc_id, agent_type, session_id, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// SHA-256 hex digest helper. Pulled out so the token storage
/// algorithm is easy to swap (e.g. argon2 if we ever store
/// long-lived tokens — but for 10-min TTLs SHA-256 + 122-bit UUID
/// entropy is plenty).
fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrations::run(&conn).unwrap();
        // Seed a project + a disc — both have FK constraints.
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('p1', 'Test Project', '/tmp/test', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussions (id, project_id, title, created_at, updated_at)
             VALUES ('d1', 'p1', 'Test disc', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn
    }

    #[test]
    fn resolve_token_disc_returns_disc_for_valid_token() {
        let conn = setup_db();
        let issued = create_invite_token(&conn, "d1").unwrap();
        // The read-only resolver maps a live token → its disc (no consume).
        assert_eq!(
            resolve_token_disc(&conn, &issued.token).unwrap().as_deref(),
            Some("d1")
        );
        // Unknown token → None (the cross-instance "we don't host it" signal).
        assert!(resolve_token_disc(&conn, "kr-join-deadbeef").unwrap().is_none());
    }

    #[test]
    fn resolve_token_disc_rejects_expired_token() {
        let conn = setup_db();
        let plain = "kr-join-expired";
        conn.execute(
            "INSERT INTO discussion_invite_tokens (token_hash, disc_id, created_at, expires_at)
             VALUES (?1, 'd1', ?2, ?2)",
            params![sha256_hex(plain), "2000-01-01T00:00:00+00:00"],
        )
        .unwrap();
        assert!(resolve_token_disc(&conn, plain).unwrap().is_none());
    }

    #[test]
    fn join_disc_session_is_idempotent_and_distinct_per_session() {
        let conn = setup_db();
        // Token-free bind (used for cross-instance mirror discs).
        let pk1 = join_disc_session(&conn, "d1", "ClaudeCode", "sess-1").unwrap();
        let pk2 = join_disc_session(&conn, "d1", "ClaudeCode", "sess-1").unwrap();
        assert_eq!(pk1, pk2, "re-join of same session returns the same pk");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM discussion_sessions WHERE disc_id='d1' AND session_id='sess-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "no phantom duplicate session rows");
        let pk3 = join_disc_session(&conn, "d1", "ClaudeCode", "sess-2").unwrap();
        assert_ne!(pk1, pk3, "a different session id is a distinct participant");
    }

    #[test]
    fn list_sessions_surfaces_last_seen_for_presence_freshness() {
        let conn = setup_db();
        // A bound session starts with no heartbeat…
        join_disc_session(&conn, "d1", "ClaudeCode", "sess-1").unwrap();
        let before = list_sessions(&conn, "d1", false).unwrap();
        let s = before.iter().find(|s| s.session_id.as_deref() == Some("sess-1")).unwrap();
        assert!(s.last_seen.is_none(), "no activity yet → last_seen None");
        // …and after a heartbeat (every disc_append calls this), it's surfaced
        // so the UI can show "active" vs "silent" instead of a dead-looking room.
        touch_session_by_agent(&conn, "d1", "ClaudeCode").unwrap();
        let after = list_sessions(&conn, "d1", false).unwrap();
        let s = after.iter().find(|s| s.session_id.as_deref() == Some("sess-1")).unwrap();
        assert!(s.last_seen.is_some(), "heartbeat must surface in list_sessions");
    }

    #[test]
    fn session_activity_is_set_cleared_and_expires_at_read_time() {
        // 0.8.12 PR B — the activity placeholder lifecycle.
        let conn = setup_db();
        join_disc_session(&conn, "d1", "ClaudeCode", "sess-1").unwrap();

        // No activity until a wait opens.
        let s = &list_sessions(&conn, "d1", false).unwrap()[0];
        assert!(s.activity.is_none());

        // "listening" with a live TTL surfaces.
        set_session_activity(&conn, "d1", "ClaudeCode", None, "listening", 60).unwrap();
        let s = &list_sessions(&conn, "d1", false).unwrap()[0];
        assert_eq!(s.activity.as_deref(), Some("listening"));

        // clear (disc_append / disc_leave) removes it instantly.
        clear_session_activity(&conn, "d1", "ClaudeCode", None).unwrap();
        let s = &list_sessions(&conn, "d1", false).unwrap()[0];
        assert!(s.activity.is_none(), "cleared activity must vanish");

        // An EXPIRED activity reads as None — read-side expiry, no reaper.
        set_session_activity(&conn, "d1", "ClaudeCode", None, "reading", -1).unwrap();
        let s = &list_sessions(&conn, "d1", false).unwrap()[0];
        assert!(s.activity.is_none(), "expired activity must read as None");
    }

    #[test]
    fn session_activity_is_scoped_to_one_session_of_the_agent_type() {
        // Copilot round 9: two concurrent sessions of the SAME agent type
        // (multi-machine) must not inherit each other's placeholder.
        let conn = setup_db();
        join_disc_session(&conn, "d1", "ClaudeCode", "sess-mac").unwrap();
        conn.execute(
            "INSERT INTO discussion_sessions
                (disc_id, agent_type, session_id, role, status, joined_at)
             VALUES ('d1', 'ClaudeCode', 'sess-wsl', 'peer', 'active', datetime('now'))",
            [],
        )
        .unwrap();

        set_session_activity(&conn, "d1", "ClaudeCode", Some("sess-mac"), "listening", 60).unwrap();
        let rows = list_sessions(&conn, "d1", false).unwrap();
        let mac = rows.iter().find(|s| s.session_id.as_deref() == Some("sess-mac")).unwrap();
        let wsl = rows.iter().find(|s| s.session_id.as_deref() == Some("sess-wsl")).unwrap();
        assert_eq!(mac.activity.as_deref(), Some("listening"));
        assert!(wsl.activity.is_none(), "the sibling session must not inherit the placeholder");

        // Scoped clear removes only the targeted session's activity.
        set_session_activity(&conn, "d1", "ClaudeCode", Some("sess-wsl"), "reading", 60).unwrap();
        clear_session_activity(&conn, "d1", "ClaudeCode", Some("sess-mac")).unwrap();
        let rows = list_sessions(&conn, "d1", false).unwrap();
        let mac = rows.iter().find(|s| s.session_id.as_deref() == Some("sess-mac")).unwrap();
        let wsl = rows.iter().find(|s| s.session_id.as_deref() == Some("sess-wsl")).unwrap();
        assert!(mac.activity.is_none());
        assert_eq!(wsl.activity.as_deref(), Some("reading"), "scoped clear spares the sibling");
    }

    #[test]
    fn session_activity_only_touches_active_sessions() {
        let conn = setup_db();
        let pk = create_session(&conn, "d1", "Codex", Some("sess-c"), "peer").unwrap();
        set_session_status(&conn, pk, "paused").unwrap();
        set_session_activity(&conn, "d1", "Codex", None, "listening", 60).unwrap();
        let all = list_sessions(&conn, "d1", false).unwrap();
        let s = all.iter().find(|s| s.session_id.as_deref() == Some("sess-c")).unwrap();
        assert!(s.activity.is_none(), "a paused session is not a live listener");
    }

    #[test]
    fn join_disc_session_rebinds_session_across_discs() {
        // Regression: the partial unique index idx_disc_sessions_session_active
        // is GLOBAL across discs, so a bridge session already active on one disc
        // that joins a second one (e.g. a freshly mirrored cross-instance disc)
        // must release the first binding rather than trip the unique constraint.
        let conn = setup_db();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO discussions (id, project_id, title, created_at, updated_at)
             VALUES ('d2', 'p1', 'Mirror disc', ?1, ?1)",
            params![now],
        )
        .unwrap();

        let pk_d1 = join_disc_session(&conn, "d1", "ClaudeCode", "sess-1").unwrap();
        // Previously panicked here with UNIQUE constraint failed.
        let pk_d2 = join_disc_session(&conn, "d2", "ClaudeCode", "sess-1").unwrap();
        assert_ne!(pk_d1, pk_d2, "the new disc gets its own active row");

        // The old binding is released, exactly one active binding remains.
        let active: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM discussion_sessions
                  WHERE agent_type='ClaudeCode' AND session_id='sess-1' AND status != 'left'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active, 1, "a bridge session is active in exactly one room");
        let d1_status: String = conn
            .query_row(
                "SELECT status FROM discussion_sessions WHERE id = ?1",
                params![pk_d1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(d1_status, "left", "the previous binding is marked left");
    }

    #[test]
    fn migration_backfills_owner_row_for_existing_sourced_disc() {
        // Pre-condition : if a disc was created with source_agent set
        // (via migration 054), the new migration 060 must seed one
        // owner row for it. setup_db's disc has no source_agent so
        // it gets 0 seed rows — we add one manually with source_agent
        // and re-run migrations to assert.
        let conn = Connection::open_in_memory().unwrap();
        // Run migrations UP TO 059 only (we want to insert a sourced
        // disc BEFORE 060 fires). Simulated by running all then
        // hand-deleting the seed row we'd otherwise inherit.
        migrations::run(&conn).unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('p2', 'Test', '/tmp', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussions
                (id, project_id, title, source_agent, source_session_id, created_at, updated_at)
             VALUES ('d2', 'p2', 'Sourced disc', 'ClaudeCode', 'sess-abc', ?1, ?1)",
            params![now],
        )
        .unwrap();
        // Simulate the migration backfill on the new row by replaying
        // the INSERT…SELECT clause (the migration ran but on an empty
        // discussions table earlier). This guards the SELECT clause
        // logic, which is what we actually want to test.
        conn.execute(
            "INSERT INTO discussion_sessions
                (disc_id, agent_type, session_id, role, status, joined_at)
             SELECT id, source_agent, source_session_id, 'owner', 'active', created_at
               FROM discussions
              WHERE id = 'd2'",
            [],
        )
        .unwrap();
        let sessions = list_sessions(&conn, "d2", false).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].agent_type, "ClaudeCode");
        assert_eq!(sessions[0].session_id.as_deref(), Some("sess-abc"));
        assert_eq!(sessions[0].role, "owner");
        assert_eq!(sessions[0].status, "active");
    }

    #[test]
    fn create_session_returns_rowid_and_is_listable() {
        let conn = setup_db();
        let id = create_session(&conn, "d1", "Codex", Some("sess-1"), "peer").unwrap();
        assert!(id > 0);
        let list = list_sessions(&conn, "d1", false).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].agent_type, "Codex");
        assert_eq!(list[0].role, "peer");
        assert_eq!(list[0].status, "active");
    }

    #[test]
    fn create_session_rejects_invalid_role() {
        let conn = setup_db();
        let err =
            create_session(&conn, "d1", "Codex", Some("sess-1"), "admin").unwrap_err();
        assert!(err.to_string().contains("invalid role"));
    }

    #[test]
    fn mark_session_left_is_idempotent() {
        let conn = setup_db();
        let id = create_session(&conn, "d1", "Codex", Some("sess-1"), "peer").unwrap();
        mark_session_left(&conn, id).unwrap();
        mark_session_left(&conn, id).unwrap(); // no-op
        let active = list_sessions(&conn, "d1", false).unwrap();
        assert_eq!(active.len(), 0, "left session must not show in active list");
        let all = list_sessions(&conn, "d1", true).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].status, "left");
        assert!(all[0].left_at.is_some());
    }

    #[test]
    fn set_session_status_toggles_active_paused() {
        let conn = setup_db();
        let id = create_session(&conn, "d1", "Codex", Some("sess-1"), "peer").unwrap();
        set_session_status(&conn, id, "paused").unwrap();
        let list = list_sessions(&conn, "d1", false).unwrap();
        assert_eq!(list[0].status, "paused");
        set_session_status(&conn, id, "active").unwrap();
        let list2 = list_sessions(&conn, "d1", false).unwrap();
        assert_eq!(list2[0].status, "active");
    }

    #[test]
    fn find_active_session_by_cli_returns_match() {
        let conn = setup_db();
        create_session(&conn, "d1", "Codex", Some("sess-X"), "peer").unwrap();
        let hit = find_active_session(&conn, "Codex", "sess-X").unwrap();
        assert!(hit.is_some());
        let miss = find_active_session(&conn, "Codex", "sess-other").unwrap();
        assert!(miss.is_none());
    }

    #[test]
    fn find_active_session_excludes_left_sessions() {
        let conn = setup_db();
        let id = create_session(&conn, "d1", "Codex", Some("sess-X"), "peer").unwrap();
        mark_session_left(&conn, id).unwrap();
        let hit = find_active_session(&conn, "Codex", "sess-X").unwrap();
        assert!(hit.is_none(), "left sessions must not match");
    }

    #[test]
    fn count_active_participants_ignores_left() {
        let conn = setup_db();
        create_session(&conn, "d1", "ClaudeCode", Some("a"), "owner").unwrap();
        create_session(&conn, "d1", "Codex", Some("b"), "peer").unwrap();
        let id3 = create_session(&conn, "d1", "GeminiCli", Some("c"), "peer").unwrap();
        mark_session_left(&conn, id3).unwrap();
        assert_eq!(count_active_participants(&conn, "d1").unwrap(), 2);
    }

    #[test]
    fn count_live_participants_only_counts_active() {
        // Double-responder guard (2026-06-04): only `active` sessions are
        // live responders. `paused` (won't reply) and `left` must NOT count,
        // else Kronn would wrongly suppress its own auto-response.
        let conn = setup_db();
        let id_active = create_session(&conn, "d1", "ClaudeCode", Some("a"), "owner").unwrap();
        let id_paused = create_session(&conn, "d1", "Codex", Some("b"), "peer").unwrap();
        let id_left = create_session(&conn, "d1", "GeminiCli", Some("c"), "peer").unwrap();
        set_session_status(&conn, id_paused, "paused").unwrap();
        mark_session_left(&conn, id_left).unwrap();
        // active=1, paused=1, left=1
        assert_eq!(count_live_participants(&conn, "d1").unwrap(), 1, "only the active session is a live responder");
        assert_eq!(count_active_participants(&conn, "d1").unwrap(), 2, "active+paused for the UI badge");
        // Pausing the last active one → no live responder (Kronn should auto-spawn).
        set_session_status(&conn, id_active, "paused").unwrap();
        assert_eq!(count_live_participants(&conn, "d1").unwrap(), 0, "all paused → no live responder");
    }

    #[test]
    fn count_live_participants_is_presence_sticky() {
        // PRESENCE-STICKY (2026-06-08): an `active` session counts no matter
        // how old its last_seen is — a turn-based CLI peer idles for many
        // minutes between human turns but is NOT gone. The earlier 300s
        // window wrongly dropped it → double-responder. Only `paused`/`left`
        // stop counting.
        let conn = setup_db();
        let id_fresh = create_session(&conn, "d1", "ClaudeCode", Some("a"), "owner").unwrap();
        let id_old = create_session(&conn, "d1", "Codex", Some("b"), "peer").unwrap();
        // Force the peer's heartbeat to 48 min ago (the ca495847 repro gap) —
        // well past the OLD 5min window, but it must STILL count now.
        let old_ts = (Utc::now() - Duration::minutes(48)).to_rfc3339();
        conn.execute(
            "UPDATE discussion_sessions SET last_seen = ?2 WHERE id = ?1",
            params![id_old, old_ts],
        )
        .unwrap();
        assert_eq!(count_live_participants(&conn, "d1").unwrap(), 2, "an idle-but-active peer still counts (sticky)");

        // paused / left drop out.
        set_session_status(&conn, id_fresh, "paused").unwrap();
        assert_eq!(count_live_participants(&conn, "d1").unwrap(), 1, "paused no longer a live responder");
        mark_session_left(&conn, id_old).unwrap();
        assert_eq!(count_live_participants(&conn, "d1").unwrap(), 0, "left no longer a live responder");
    }

    #[test]
    fn reap_abandoned_sessions_retires_only_long_dead_active_rows() {
        // The crashed-peer safety valve for presence-sticky: a session idle
        // beyond SESSION_ABANDON_SECS (24h) is marked 'left' so it stops
        // pinning the gate. A recently-active peer is untouched.
        let conn = setup_db();
        let id_recent = create_session(&conn, "d1", "ClaudeCode", Some("a"), "owner").unwrap();
        let id_dead = create_session(&conn, "d1", "Codex", Some("b"), "peer").unwrap();
        // Recent peer: last_seen 1h ago (well within 24h) → kept.
        conn.execute("UPDATE discussion_sessions SET last_seen = ?2 WHERE id = ?1",
            params![id_recent, (Utc::now() - Duration::hours(1)).to_rfc3339()]).unwrap();
        // Dead ghost: last_seen 3 days ago → reaped.
        conn.execute("UPDATE discussion_sessions SET last_seen = ?2 WHERE id = ?1",
            params![id_dead, (Utc::now() - Duration::days(3)).to_rfc3339()]).unwrap();

        let reaped = reap_abandoned_sessions(&conn).unwrap();
        assert_eq!(reaped, 1, "only the 3-day-old ghost is retired");
        assert_eq!(count_live_participants(&conn, "d1").unwrap(), 1, "recent peer still live, ghost gone");
        // Idempotent: a second pass reaps nothing.
        assert_eq!(reap_abandoned_sessions(&conn).unwrap(), 0, "idempotent");
    }

    /// I8: if the system clock is BEHIND the newest recorded activity (a session
    /// heartbeated "in the future" = the clock rolled back), reaping must skip —
    /// a transient wrong clock must never retire live sessions.
    #[test]
    fn reap_skips_when_clock_is_behind_recorded_activity() {
        let conn = setup_db();
        let id_future = create_session(&conn, "d1", "ClaudeCode", Some("a"), "owner").unwrap();
        let id_ghost = create_session(&conn, "d1", "Codex", Some("b"), "peer").unwrap();
        // Newest activity is in the FUTURE → now < newest → suspected skew.
        conn.execute("UPDATE discussion_sessions SET last_seen = ?2 WHERE id = ?1",
            params![id_future, (Utc::now() + Duration::hours(1)).to_rfc3339()]).unwrap();
        // A normally-reapable 3-day-old ghost — must be spared while skew suspected.
        conn.execute("UPDATE discussion_sessions SET last_seen = ?2 WHERE id = ?1",
            params![id_ghost, (Utc::now() - Duration::days(3)).to_rfc3339()]).unwrap();

        assert_eq!(reap_abandoned_sessions(&conn).unwrap(), 0, "clock behind activity → skip reap");
        assert_eq!(count_live_participants(&conn, "d1").unwrap(), 2, "no session reaped under suspected skew");
    }

    /// I8: if `now` is absurdly far AHEAD of all recorded activity (>10 years — a
    /// garbage/far-future clock), skip rather than mass-reap every session.
    #[test]
    fn reap_skips_when_clock_is_absurdly_ahead_of_activity() {
        let conn = setup_db();
        let id = create_session(&conn, "d1", "ClaudeCode", Some("a"), "owner").unwrap();
        conn.execute("UPDATE discussion_sessions SET last_seen = ?2 WHERE id = ?1",
            params![id, (Utc::now() - Duration::days(SESSION_SKEW_SANITY_DAYS + 30)).to_rfc3339()]).unwrap();

        assert_eq!(reap_abandoned_sessions(&conn).unwrap(), 0, "clock absurdly ahead → skip reap");
        assert_eq!(count_live_participants(&conn, "d1").unwrap(), 1, "session spared under suspected skew");
    }

    #[test]
    fn create_invite_token_yields_kr_join_prefix() {
        let conn = setup_db();
        let issued = create_invite_token(&conn, "d1").unwrap();
        assert!(issued.token.starts_with("kr-join-"), "token = {}", issued.token);
        assert_eq!(issued.disc_id, "d1");
        // Plain token must NOT be stored in DB.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM discussion_invite_tokens
                  WHERE token_hash = ?1",
                params![issued.token],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "plain token must never appear in token_hash column");
        // The HASH should exist.
        let hash = sha256_hex(&issued.token);
        let count_by_hash: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM discussion_invite_tokens
                  WHERE token_hash = ?1",
                params![hash],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count_by_hash, 1);
    }

    #[test]
    fn consume_invite_token_is_multi_use_within_ttl() {
        // 0.8.6 fix 2026-05-21 — tokens are NOT single-use anymore.
        // Same token can be consumed by N peers within its 10-min TTL.
        let conn = setup_db();
        let issued = create_invite_token(&conn, "d1").unwrap();
        let session_a =
            create_session(&conn, "d1", "Codex", Some("sess-a"), "peer").unwrap();
        let session_b =
            create_session(&conn, "d1", "GeminiCli", Some("sess-b"), "peer").unwrap();

        // First use OK.
        assert_eq!(consume_invite_token(&conn, &issued.token, session_a).unwrap(), "d1");
        // Second use ALSO OK (different agent reusing the same invite link).
        assert_eq!(consume_invite_token(&conn, &issued.token, session_b).unwrap(), "d1");
    }

    #[test]
    fn consume_invite_token_records_first_use_only_for_audit() {
        // Audit trail : the token row tracks WHO used it first.
        // Subsequent uses don't overwrite the audit columns.
        let conn = setup_db();
        let issued = create_invite_token(&conn, "d1").unwrap();
        let session_a =
            create_session(&conn, "d1", "Codex", Some("sess-a"), "peer").unwrap();
        let session_b =
            create_session(&conn, "d1", "GeminiCli", Some("sess-b"), "peer").unwrap();

        consume_invite_token(&conn, &issued.token, session_a).unwrap();
        consume_invite_token(&conn, &issued.token, session_b).unwrap();

        let first_user_pk: i64 = conn.query_row(
            "SELECT used_by_session_id FROM discussion_invite_tokens
              WHERE disc_id = 'd1' LIMIT 1",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(
            first_user_pk, session_a,
            "audit column must record FIRST use, not last",
        );
    }

    #[test]
    fn consume_invite_token_rejects_unknown_token() {
        let conn = setup_db();
        let _ = create_session(&conn, "d1", "Codex", Some("sess"), "peer").unwrap();
        let err = consume_invite_token(&conn, "kr-join-totallymadeup", 1).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn consume_invite_token_rejects_expired_token() {
        let conn = setup_db();
        let session_id =
            create_session(&conn, "d1", "Codex", Some("sess"), "peer").unwrap();
        // Insert an expired token directly (bypass create_invite_token's
        // chrono::Utc::now to forge the past).
        let plain = "kr-join-expiredtokenfortest";
        let hash = sha256_hex(plain);
        let created = (Utc::now() - Duration::seconds(1200)).to_rfc3339();
        let expired = (Utc::now() - Duration::seconds(600)).to_rfc3339();
        conn.execute(
            "INSERT INTO discussion_invite_tokens
                (token_hash, disc_id, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![hash, "d1", created, expired],
        )
        .unwrap();
        let err = consume_invite_token(&conn, plain, session_id).unwrap_err();
        assert!(err.to_string().contains("expired"), "got: {err}");
    }

    #[test]
    fn join_via_token_atomically_creates_session_and_marks_token_used() {
        let conn = setup_db();
        let issued = create_invite_token(&conn, "d1").unwrap();
        let result = join_via_token(&conn, &issued.token, "Codex", "sess-peer-1").unwrap();
        assert_eq!(result.disc_id, "d1");
        assert!(result.session_pk > 0);

        // Session row visible.
        let sessions = list_sessions(&conn, "d1", false).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].agent_type, "Codex");
        assert_eq!(sessions[0].role, "peer");
        assert_eq!(sessions[0].status, "active");

        // Token marked used.
        let used: Option<String> = conn
            .query_row(
                "SELECT used_at FROM discussion_invite_tokens WHERE id = (
                    SELECT id FROM discussion_invite_tokens WHERE disc_id = 'd1'
                 )",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(used.is_some());
    }

    #[test]
    fn join_via_token_is_multi_use_within_ttl() {
        // 0.8.6 fix 2026-05-21 — multi-use token. 3 peers can join
        // with the same token within the TTL window. Lets the user
        // click [+ Inviter] once for N peers.
        let conn = setup_db();
        let issued = create_invite_token(&conn, "d1").unwrap();

        let r1 = join_via_token(&conn, &issued.token, "ClaudeCode", "sess-A").unwrap();
        let r2 = join_via_token(&conn, &issued.token, "Codex", "sess-B").unwrap();
        let r3 = join_via_token(&conn, &issued.token, "GeminiCli", "sess-C").unwrap();

        // 3 distinct sessions, all on the same disc.
        assert_eq!(r1.disc_id, "d1");
        assert_eq!(r2.disc_id, "d1");
        assert_eq!(r3.disc_id, "d1");
        assert_ne!(r1.session_pk, r2.session_pk);
        assert_ne!(r2.session_pk, r3.session_pk);

        let sessions = list_sessions(&conn, "d1", false).unwrap();
        assert_eq!(sessions.len(), 3);
        let agents: Vec<&str> = sessions.iter().map(|s| s.agent_type.as_str()).collect();
        assert!(agents.contains(&"ClaudeCode"));
        assert!(agents.contains(&"Codex"));
        assert!(agents.contains(&"GeminiCli"));
    }

    #[test]
    fn join_via_token_is_idempotent_on_same_session_pair() {
        // Same (agent_type, session_id) joining twice with the same
        // token → return the existing pk, don't insert a duplicate.
        // Guards against flaky-network retry duplicates.
        let conn = setup_db();
        let issued = create_invite_token(&conn, "d1").unwrap();
        let r1 = join_via_token(&conn, &issued.token, "Codex", "sess-X").unwrap();
        let r2 = join_via_token(&conn, &issued.token, "Codex", "sess-X").unwrap();
        assert_eq!(r1.session_pk, r2.session_pk, "same pk on re-join");
        let sessions = list_sessions(&conn, "d1", false).unwrap();
        assert_eq!(sessions.len(), 1, "no phantom duplicate row");
    }

    #[test]
    fn join_via_token_rolls_back_session_when_token_expired() {
        let conn = setup_db();
        // Forge an expired token directly.
        let plain = "kr-join-deadtokenfortesting";
        let hash = sha256_hex(plain);
        let created = (Utc::now() - Duration::seconds(1200)).to_rfc3339();
        let expired = (Utc::now() - Duration::seconds(600)).to_rfc3339();
        conn.execute(
            "INSERT INTO discussion_invite_tokens
                (token_hash, disc_id, created_at, expires_at)
             VALUES (?1, 'd1', ?2, ?3)",
            params![hash, created, expired],
        )
        .unwrap();

        let err = join_via_token(&conn, plain, "Codex", "sess-X").unwrap_err();
        assert!(err.to_string().contains("expired"));
        let sessions = list_sessions(&conn, "d1", true).unwrap();
        assert_eq!(sessions.len(), 0, "expired token must not seed a session");
    }

    #[test]
    fn sha256_hex_is_deterministic_and_hex_shaped() {
        let a = sha256_hex("abc");
        let b = sha256_hex("abc");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
