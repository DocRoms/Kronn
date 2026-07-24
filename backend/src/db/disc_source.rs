//! 0.8.4 (#294) — Cross-agent memory bindings.
//!
//! Helpers that operate on the `discussions.source_*` columns + the
//! `disc_source_history` append-only chain (migration 054). Powers the
//! 7 HTTP routes that let an external CLI agent (Claude Code, Cursor,
//! Codex, …) push its conversation history into Kronn so the SAME
//! discussion thread can be picked up by a DIFFERENT agent later.
//!
//! The split from `db/discussions.rs` is deliberate: this module
//! touches a narrow slice of the schema (4 new columns + 1 new table)
//! and the helpers all share the same `source_agent + source_session_id`
//! lookup pattern. Keeping them grouped here makes the cross-agent
//! feature easy to reason about (and audit) end-to-end.
//!
//! See `project_cross_agent_memory_0_8_4.md` in memory for the design
//! rationale (last-link-wins, idempotent appends via `source_msg_id`,
//! divergence detection via `diverged_at`).

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One row of source-binding history. `unlinked_at IS NULL` ⇒
/// currently bound. The frontend renders these in a tooltip so the
/// user can see the full "this disc was first owned by ClaudeCode
/// session X, then Cursor session Y" chain.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscSourceHistoryEntry {
    pub source_agent: String,
    pub source_session_id: String,
    pub linked_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unlinked_at: Option<String>,
}

/// Bind a disc to a (source_agent, source_session_id) pair. Sets the
/// 3 source_* columns on the disc AND records a row in
/// `disc_source_history`. Idempotent on the same (agent, session) pair
/// — re-binding the same session does NOT duplicate the history row
/// (open row already exists for that pair).
///
/// If the disc is currently bound to a DIFFERENT (agent, session)
/// pair, the open history row for the previous binding is closed
/// (unlinked_at = now) before the new binding is recorded — "last
/// link wins" semantics.
pub fn bind_to_source(
    conn: &Connection,
    disc_id: &str,
    source_agent: &str,
    source_session_id: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();

    // Check whether a row for THIS pair is already open (idempotent
    // re-bind). If so, only the disc columns need updating — the
    // history row stays untouched.
    let already_open: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM disc_source_history
         WHERE disc_id = ?1
           AND source_agent = ?2
           AND source_session_id = ?3
           AND unlinked_at IS NULL",
            params![disc_id, source_agent, source_session_id],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !already_open {
        // Close any other open binding on this disc — only one source
        // can own a disc at a time.
        conn.execute(
            "UPDATE disc_source_history
             SET unlinked_at = ?2
             WHERE disc_id = ?1 AND unlinked_at IS NULL",
            params![disc_id, now],
        )?;
        conn.execute(
            "INSERT INTO disc_source_history (disc_id, source_agent, source_session_id, linked_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![disc_id, source_agent, source_session_id, now],
        )?;
    }

    conn.execute(
        "UPDATE discussions
         SET source_agent = ?2,
             source_session_id = ?3,
             imported_at = COALESCE(imported_at, ?4),
             diverged_at = NULL
         WHERE id = ?1",
        params![disc_id, source_agent, source_session_id, now],
    )?;
    Ok(())
}

/// Release the current binding. Closes the open history row + clears
/// the disc's source_* columns. No-op when the disc has no active
/// binding. The history chain is preserved so the UI can still show
/// "was previously imported from ClaudeCode session X".
pub fn unbind_from_source(conn: &Connection, disc_id: &str) -> Result<bool> {
    let now = Utc::now().to_rfc3339();
    let closed = conn.execute(
        "UPDATE disc_source_history
         SET unlinked_at = ?2
         WHERE disc_id = ?1 AND unlinked_at IS NULL",
        params![disc_id, now],
    )?;
    if closed > 0 {
        conn.execute(
            "UPDATE discussions
             SET source_agent = NULL,
                 source_session_id = NULL
             WHERE id = ?1",
            params![disc_id],
        )?;
    }
    Ok(closed > 0)
}

/// Resolve a (source_agent, source_session_id) pair to its current
/// disc_id (the one with an open history row). Returns `None` when
/// the session has never been bound or was unlinked.
pub fn find_disc_by_source_session(
    conn: &Connection,
    source_agent: &str,
    source_session_id: &str,
) -> Result<Option<String>> {
    let id: Option<String> = conn
        .query_row(
            "SELECT disc_id FROM disc_source_history
         WHERE source_agent = ?1
           AND source_session_id = ?2
           AND unlinked_at IS NULL
         ORDER BY linked_at DESC
         LIMIT 1",
            params![source_agent, source_session_id],
            |row| row.get(0),
        )
        .ok();
    Ok(id)
}

/// Snapshot of every disc currently bound to a source. Used by the
/// frontend sidebar to decorate items with a "from X" badge without
/// having to query per-disc. Returns `(disc_id, source_agent,
/// source_session_id, imported_at, diverged_at)` tuples for every
/// disc where `source_agent IS NOT NULL`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscSourceBinding {
    pub disc_id: String,
    pub source_agent: String,
    pub source_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diverged_at: Option<String>,
}

pub fn list_all_source_bindings(conn: &Connection) -> Result<Vec<DiscSourceBinding>> {
    let mut stmt = conn.prepare(
        "SELECT id, source_agent, source_session_id, imported_at, diverged_at
         FROM discussions
         WHERE source_agent IS NOT NULL AND source_session_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(DiscSourceBinding {
            disc_id: row.get(0)?,
            source_agent: row.get(1)?,
            source_session_id: row.get(2)?,
            imported_at: row.get(3)?,
            diverged_at: row.get(4)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Full history chain for a disc (most recent first). Used by the
/// frontend tooltip + a forensic "where did this thread come from?"
/// view. Closed rows surface as `unlinked_at: Some(...)`.
pub fn list_source_history(
    conn: &Connection,
    disc_id: &str,
) -> Result<Vec<DiscSourceHistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT source_agent, source_session_id, linked_at, unlinked_at
         FROM disc_source_history
         WHERE disc_id = ?1
         ORDER BY linked_at DESC",
    )?;
    let rows = stmt.query_map(params![disc_id], |row| {
        Ok(DiscSourceHistoryEntry {
            source_agent: row.get(0)?,
            source_session_id: row.get(1)?,
            linked_at: row.get(2)?,
            unlinked_at: row.get(3)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Read the disc's `diverged_at` timestamp (RFC3339 string) directly
/// from the column. Not on the `Discussion` struct (kept lean — see
/// migration 054 + the comment in `models/discussions.rs`), so we
/// query the column here.
pub fn get_diverged_at(conn: &Connection, disc_id: &str) -> Result<Option<String>> {
    let v: Option<String> = conn
        .query_row(
            "SELECT diverged_at FROM discussions WHERE id = ?1",
            params![disc_id],
            |row| row.get(0),
        )
        .ok()
        .flatten();
    Ok(v)
}

/// Flag a disc as "diverged" — the user has edited messages inside
/// Kronn AFTER an import, so a later `disc_append` from the original
/// source should NOT silently overwrite their edits. The frontend
/// uses this to render a warning on the import button.
pub fn mark_diverged(conn: &Connection, disc_id: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE discussions
         SET diverged_at = COALESCE(diverged_at, ?2)
         WHERE id = ?1 AND source_session_id IS NOT NULL",
        params![disc_id, now],
    )?;
    Ok(())
}

/// Check whether a `(disc_id, source_msg_id)` pair already exists in
/// `messages`. Drives the dedup pass during `disc_append`.
pub fn message_exists_for_source_id(
    conn: &Connection,
    disc_id: &str,
    source_msg_id: &str,
) -> Result<bool> {
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM messages
         WHERE discussion_id = ?1 AND source_msg_id = ?2",
            params![disc_id, source_msg_id],
            |row| row.get(0),
        )
        .unwrap_or(false);
    Ok(exists)
}

/// LIKE-based full-text search across disc titles + message content.
/// Cheap-and-cheerful (no FTS5 wiring): finds the N most recent discs
/// where title OR any message content matches `%q%` (case-insensitive
/// — SQLite's LIKE is CI on ASCII by default; for non-ASCII queries
/// the user just adds wildcards).
///
/// Returns (disc_id, title, snippet) tuples — snippet is the first
/// 80 chars of the first matching message body, or the title if the
/// match was on the title.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscSearchHit {
    pub disc_id: String,
    pub title: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<String>,
}

pub fn search_discussions(conn: &Connection, q: &str, limit: u32) -> Result<Vec<DiscSearchHit>> {
    let pattern = format!("%{}%", q.replace('%', "\\%").replace('_', "\\_"));
    let lim = limit.clamp(1, 50);

    let mut stmt = conn.prepare(
        "SELECT d.id, d.title, d.source_agent, d.source_session_id,
                COALESCE(
                    (SELECT m.content FROM messages m
                     WHERE m.discussion_id = d.id AND m.content LIKE ?1 ESCAPE '\\'
                     ORDER BY m.sort_order ASC LIMIT 1),
                    d.title
                ) AS snippet
         FROM discussions d
         WHERE d.title LIKE ?1 ESCAPE '\\'
            OR EXISTS (
                SELECT 1 FROM messages m
                WHERE m.discussion_id = d.id AND m.content LIKE ?1 ESCAPE '\\'
            )
         ORDER BY d.updated_at DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, lim as i64], |row| {
        let raw_snip: String = row.get(4)?;
        let trimmed = if raw_snip.chars().count() > 80 {
            let cutoff = raw_snip
                .char_indices()
                .nth(80)
                .map(|(i, _)| i)
                .unwrap_or(raw_snip.len());
            format!("{}…", &raw_snip[..cutoff])
        } else {
            raw_snip
        };
        Ok(DiscSearchHit {
            disc_id: row.get(0)?,
            title: row.get(1)?,
            source_agent: row.get(2)?,
            source_session_id: row.get(3)?,
            snippet: trimmed,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DiscussionMessage;

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("
            CREATE TABLE discussions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                message_count INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                source_agent TEXT,
                source_session_id TEXT,
                imported_at DATETIME,
                diverged_at DATETIME
            );
            CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                discussion_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                sort_order INTEGER NOT NULL,
                source_msg_id TEXT
            );
            CREATE INDEX idx_msg_source_dedup ON messages(discussion_id, source_msg_id);
            CREATE TABLE disc_source_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                disc_id TEXT NOT NULL,
                source_agent TEXT NOT NULL,
                source_session_id TEXT NOT NULL,
                linked_at DATETIME NOT NULL,
                unlinked_at DATETIME
            );
            CREATE INDEX idx_disc_src_hist_lookup ON disc_source_history(source_agent, source_session_id);
            CREATE INDEX idx_disc_src_hist_disc ON disc_source_history(disc_id);
            INSERT INTO discussions (id, title, updated_at) VALUES
                ('d-alpha', 'First disc', '2026-05-15T10:00:00Z'),
                ('d-beta',  'Second disc', '2026-05-15T11:00:00Z');
            INSERT INTO messages (id, discussion_id, role, content, sort_order, source_msg_id) VALUES
                ('m1', 'd-alpha', 'user',  'Hello from ClaudeCode session A', 1, 'cc-msg-1'),
                ('m2', 'd-alpha', 'agent', 'Hi back', 2, 'cc-msg-2'),
                ('m3', 'd-beta',  'user',  'A totally different conversation', 1, NULL);
        ").unwrap();
        // sanity-check the imports compile against the live model.
        let _ = std::any::type_name::<DiscussionMessage>();
        conn
    }

    #[test]
    fn bind_then_find_round_trip() {
        let conn = fresh_conn();
        bind_to_source(&conn, "d-alpha", "ClaudeCode", "sess-abc").unwrap();
        let id = find_disc_by_source_session(&conn, "ClaudeCode", "sess-abc").unwrap();
        assert_eq!(id.as_deref(), Some("d-alpha"));
        // Sister history row is open.
        let hist = list_source_history(&conn, "d-alpha").unwrap();
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].source_agent, "ClaudeCode");
        assert!(hist[0].unlinked_at.is_none());
    }

    #[test]
    fn bind_same_session_twice_is_idempotent() {
        // 0.8.4 (#294) — re-binding the SAME (agent, session) pair
        // must not duplicate history rows. Otherwise an agent that
        // re-pushes its handshake on every reconnect would balloon
        // the table.
        let conn = fresh_conn();
        bind_to_source(&conn, "d-alpha", "ClaudeCode", "sess-abc").unwrap();
        bind_to_source(&conn, "d-alpha", "ClaudeCode", "sess-abc").unwrap();
        bind_to_source(&conn, "d-alpha", "ClaudeCode", "sess-abc").unwrap();
        let hist = list_source_history(&conn, "d-alpha").unwrap();
        assert_eq!(hist.len(), 1, "idempotent on same session pair");
    }

    #[test]
    fn rebinding_different_session_closes_previous_chain() {
        // Last-link-wins: a fresh (agent, session) binding closes the
        // open row from the previous binding so only ONE row is open
        // at a time.
        let conn = fresh_conn();
        bind_to_source(&conn, "d-alpha", "ClaudeCode", "sess-A").unwrap();
        bind_to_source(&conn, "d-alpha", "Cursor", "sess-B").unwrap();

        let hist = list_source_history(&conn, "d-alpha").unwrap();
        assert_eq!(hist.len(), 2);
        // Newest first: B is open, A is closed.
        assert_eq!(hist[0].source_session_id, "sess-B");
        assert!(hist[0].unlinked_at.is_none());
        assert_eq!(hist[1].source_session_id, "sess-A");
        assert!(
            hist[1].unlinked_at.is_some(),
            "previous binding must be closed on re-link"
        );

        // find_by_source_session must return d-alpha only for the new pair.
        assert_eq!(
            find_disc_by_source_session(&conn, "Cursor", "sess-B")
                .unwrap()
                .as_deref(),
            Some("d-alpha")
        );
        assert!(
            find_disc_by_source_session(&conn, "ClaudeCode", "sess-A")
                .unwrap()
                .is_none(),
            "old (closed) binding must not resolve anymore"
        );
    }

    #[test]
    fn unbind_clears_columns_and_closes_chain() {
        let conn = fresh_conn();
        bind_to_source(&conn, "d-alpha", "ClaudeCode", "sess-Z").unwrap();
        let closed = unbind_from_source(&conn, "d-alpha").unwrap();
        assert!(closed);
        // History row preserved but closed.
        let hist = list_source_history(&conn, "d-alpha").unwrap();
        assert_eq!(hist.len(), 1);
        assert!(hist[0].unlinked_at.is_some());
        // find resolves to None.
        assert!(find_disc_by_source_session(&conn, "ClaudeCode", "sess-Z")
            .unwrap()
            .is_none());
    }

    #[test]
    fn unbind_is_noop_when_nothing_bound() {
        let conn = fresh_conn();
        let closed = unbind_from_source(&conn, "d-beta").unwrap();
        assert!(!closed, "no open binding to close");
    }

    #[test]
    fn message_exists_for_source_id_finds_match() {
        let conn = fresh_conn();
        assert!(message_exists_for_source_id(&conn, "d-alpha", "cc-msg-1").unwrap());
        assert!(message_exists_for_source_id(&conn, "d-alpha", "cc-msg-2").unwrap());
        assert!(!message_exists_for_source_id(&conn, "d-alpha", "cc-msg-999").unwrap());
        assert!(
            !message_exists_for_source_id(&conn, "d-beta", "cc-msg-1").unwrap(),
            "scope must be (disc_id, source_msg_id) — no cross-disc leak"
        );
    }

    #[test]
    fn search_discussions_matches_title_and_content() {
        let conn = fresh_conn();
        let hits = search_discussions(&conn, "ClaudeCode", 10).unwrap();
        assert_eq!(hits.len(), 1, "matches the m1 content body");
        assert_eq!(hits[0].disc_id, "d-alpha");

        let hits2 = search_discussions(&conn, "Second", 10).unwrap();
        assert_eq!(hits2.len(), 1, "matches the d-beta title");
        assert_eq!(hits2[0].disc_id, "d-beta");
    }

    #[test]
    fn search_discussions_escapes_like_metachars() {
        // A query containing `%` or `_` must NOT be interpreted as a
        // wildcard — search('100%') should match the literal string
        // "100%" only, not "1000" or anything else.
        let conn = fresh_conn();
        conn.execute(
            "INSERT INTO discussions (id, title, updated_at) VALUES ('d-pct', '100% coverage report', '2026-05-15T12:00:00Z')",
            [],
        ).unwrap();
        let hits = search_discussions(&conn, "100%", 10).unwrap();
        assert!(
            hits.iter().any(|h| h.disc_id == "d-pct"),
            "must still find the literal-% disc"
        );
    }

    #[test]
    fn mark_diverged_only_acts_on_imported_discs() {
        let conn = fresh_conn();
        // d-beta has no source binding → mark_diverged is a no-op.
        mark_diverged(&conn, "d-beta").unwrap();
        let diverged: Option<String> = conn
            .query_row(
                "SELECT diverged_at FROM discussions WHERE id = 'd-beta'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(diverged.is_none(), "non-imported disc cannot diverge");

        // After bind, mark_diverged populates the column.
        bind_to_source(&conn, "d-beta", "Codex", "sess-div").unwrap();
        mark_diverged(&conn, "d-beta").unwrap();
        let diverged: Option<String> = conn
            .query_row(
                "SELECT diverged_at FROM discussions WHERE id = 'd-beta'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            diverged.is_some(),
            "bound disc must now be flagged diverged"
        );

        // Second mark_diverged is idempotent (COALESCE preserves the
        // original timestamp).
        let original = diverged.clone();
        mark_diverged(&conn, "d-beta").unwrap();
        let diverged2: Option<String> = conn
            .query_row(
                "SELECT diverged_at FROM discussions WHERE id = 'd-beta'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            diverged2, original,
            "diverged_at must NOT be overwritten on re-mark"
        );
    }
}
