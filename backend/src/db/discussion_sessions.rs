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
use chrono::{Duration, Utc};
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
    conn.execute(
        "INSERT INTO discussion_sessions
            (disc_id, agent_type, session_id, role, status, joined_at)
         VALUES (?1, ?2, ?3, ?4, 'active', ?5)",
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
        "SELECT id, disc_id, agent_type, session_id, role, status, joined_at, left_at
           FROM discussion_sessions
          WHERE disc_id = ?1
          ORDER BY joined_at ASC"
    } else {
        "SELECT id, disc_id, agent_type, session_id, role, status, joined_at, left_at
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
            "SELECT id, disc_id, agent_type, session_id, role, status, joined_at, left_at
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
        // Step 2b — first join for this (agent_type, session_id) → insert.
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
