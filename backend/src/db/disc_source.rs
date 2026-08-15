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
//! rationale (idempotent appends via `source_msg_id`, divergence detection via
//! `diverged_at`). NOTE: the original "last-link-wins" rule is GONE (KT-85) —
//! a discussion carries one open binding per joined session, because a
//! cross-agent room is the normal case and evicting the other peers made their
//! resume lookup silently empty.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const SOURCE_BINDING_VERSION: i64 = 1;

/// One row of source-binding history. `unlinked_at IS NULL` ⇒
/// currently bound. The frontend renders these in a tooltip so the
/// user can see the full "this disc was first owned by ClaudeCode
/// session X, then Cursor session Y" chain.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscSourceHistoryEntry {
    pub binding_version: i64,
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
/// Other sessions bound to the SAME disc are left alone (KT-85): several CLIs
/// legitimately share one room. Only the calling session's binding on ANOTHER
/// disc is released, which is the invariant the partial unique index enforces.
pub fn bind_to_source(
    conn: &Connection,
    disc_id: &str,
    source_agent: &str,
    source_session_id: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();

    // A concrete CLI session can resume exactly one Kronn discussion. Close
    // an older ownership row on another discussion before inserting here.
    // Migration 092 backs this invariant with a partial unique index.
    conn.execute(
        "UPDATE disc_source_history
         SET unlinked_at = ?4
         WHERE source_agent = ?1
           AND source_session_id = ?2
           AND disc_id != ?3
           AND unlinked_at IS NULL",
        params![source_agent, source_session_id, disc_id, now],
    )?;
    // The rooms this session just left keep their OTHER bindings, so their
    // legacy pointer must be recomputed rather than blanked (KT-85).
    let vacated: Vec<String> = {
        let mut statement = conn.prepare(
            "SELECT id FROM discussions
             WHERE id != ?3 AND source_agent = ?1 AND source_session_id = ?2",
        )?;
        let rows = statement
            .query_map(params![source_agent, source_session_id, disc_id], |row| {
                row.get::<_, String>(0)
            })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for vacated_disc in &vacated {
        refresh_legacy_pointer(conn, vacated_disc)?;
    }

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
        // KT-85 — do NOT close the other sessions bound to this discussion. A
        // cross-agent room legitimately carries one binding per joined session,
        // and closing them made the last joiner evict everyone else: their
        // `disc_find_by_session` went silently empty after an MCP reload, which
        // is exactly the reconnection this substrate exists to guarantee.
        // A session bound to ANOTHER discussion is still released above — that
        // invariant is the one the partial unique index protects.
        conn.execute(
            "INSERT INTO disc_source_history
                (disc_id, source_agent, source_session_id, linked_at, binding_version)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                disc_id,
                source_agent,
                source_session_id,
                now,
                SOURCE_BINDING_VERSION
            ],
        )?;
    }

    conn.execute(
        "UPDATE discussions
         SET source_agent = ?2,
             source_session_id = ?3,
             imported_at = COALESCE(imported_at, ?4),
             diverged_at = NULL,
             source_binding_version = ?5
         WHERE id = ?1",
        params![
            disc_id,
            source_agent,
            source_session_id,
            now,
            SOURCE_BINDING_VERSION
        ],
    )?;
    Ok(())
}

/// Move one durable CLI session binding from an expected discussion to a new
/// one. The expected source makes the handoff fail closed when ownership
/// changed between the caller's lookup and transfer request.
///
/// Returns `true` when the binding moved and `false` for an idempotent replay
/// after the same transfer already succeeded.
pub fn transfer_source_binding(
    conn: &Connection,
    from_disc_id: &str,
    to_disc_id: &str,
    source_agent: &str,
    source_session_id: &str,
) -> Result<bool> {
    if from_disc_id == to_disc_id {
        anyhow::bail!("source and target discussions must differ");
    }

    let transaction = conn.unchecked_transaction()?;
    let target_exists = transaction
        .query_row(
            "SELECT 1 FROM discussions WHERE id = ?1",
            [to_disc_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !target_exists {
        anyhow::bail!("target discussion {to_disc_id} does not exist");
    }

    let current = find_disc_by_source_session(&transaction, source_agent, source_session_id)?;
    match current.as_deref() {
        Some(current_disc_id) if current_disc_id == to_disc_id => {
            let matching_closed_source = transaction
                .query_row(
                    "SELECT 1
                     FROM disc_source_history
                     WHERE disc_id = ?1
                       AND source_agent = ?2
                       AND source_session_id = ?3
                       AND unlinked_at IS NOT NULL
                     LIMIT 1",
                    params![from_disc_id, source_agent, source_session_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !matching_closed_source {
                anyhow::bail!(
                    "session is already linked to discussion {to_disc_id}, but no completed transfer from expected discussion {from_disc_id} exists"
                );
            }
            transaction.commit()?;
            return Ok(false);
        }
        Some(current_disc_id) if current_disc_id != from_disc_id => {
            anyhow::bail!(
                "session ownership changed: expected discussion {from_disc_id}, found {current_disc_id}"
            );
        }
        None => anyhow::bail!("session is not linked to expected discussion {from_disc_id}"),
        Some(_) => {}
    }

    bind_to_source(&transaction, to_disc_id, source_agent, source_session_id)?;
    transaction.commit()?;
    Ok(true)
}

/// The discussion's CURRENT binding — the most recent open one.
///
/// KT-85 — the detail endpoint used to scan `list_all_source_bindings()` and take
/// the first hit; now that a room has several, that scan returned the OLDEST
/// binding. This reads the pointer `refresh_legacy_pointer` maintains.
pub fn current_source_binding(
    conn: &Connection,
    disc_id: &str,
) -> Result<Option<DiscSourceBinding>> {
    let row = conn
        .query_row(
            "SELECT id, source_agent, source_session_id, imported_at, diverged_at,
                    source_binding_version
             FROM discussions
             WHERE id = ?1 AND source_agent IS NOT NULL AND source_session_id IS NOT NULL",
            [disc_id],
            |row| {
                Ok(DiscSourceBinding {
                    disc_id: row.get(0)?,
                    source_agent: row.get(1)?,
                    source_session_id: row.get(2)?,
                    imported_at: row.get(3)?,
                    diverged_at: row.get(4)?,
                    binding_version: row.get(5)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Point `discussions.source_*` at the most recent OPEN binding of this disc,
/// or clear it when none remains.
///
/// KT-85 — those columns predate multi-binding and are still read by the detail
/// panel and by divergence. With N bindings they are a "latest" pointer, so any
/// close/move has to recompute them: blanking them while other sessions are
/// still bound left the UI claiming the room had no source at all.
fn refresh_legacy_pointer(conn: &Connection, disc_id: &str) -> Result<()> {
    let latest: Option<(String, String, i64)> = conn
        .query_row(
            "SELECT source_agent, source_session_id, binding_version
             FROM disc_source_history
             WHERE disc_id = ?1 AND unlinked_at IS NULL
             ORDER BY linked_at DESC, id DESC
             LIMIT 1",
            [disc_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    match latest {
        Some((agent, session, version)) => conn.execute(
            "UPDATE discussions
             SET source_agent = ?2, source_session_id = ?3, source_binding_version = ?4
             WHERE id = ?1",
            params![disc_id, agent, session, version],
        )?,
        None => conn.execute(
            "UPDATE discussions
             SET source_agent = NULL, source_session_id = NULL,
                 source_binding_version = NULL
             WHERE id = ?1",
            [disc_id],
        )?,
    };
    Ok(())
}

/// Release the current binding. Closes the open history row + clears
/// the disc's source_* columns. No-op when the disc has no active
/// binding. The history chain is preserved so the UI can still show
/// "was previously imported from ClaudeCode session X".
pub fn unbind_from_source(
    conn: &Connection,
    disc_id: &str,
    // KT-85 — `None` releases EVERY binding of the room, which is only ever a
    // deliberate human choice. A peer letting go of its own link must pass its
    // pair, otherwise "release my link" evicted all the other agents.
    only: Option<(&str, &str)>,
) -> Result<bool> {
    let now = Utc::now().to_rfc3339();
    let closed = match only {
        Some((agent, session)) => conn.execute(
            "UPDATE disc_source_history
             SET unlinked_at = ?4
             WHERE disc_id = ?1 AND source_agent = ?2 AND source_session_id = ?3
               AND unlinked_at IS NULL",
            params![disc_id, agent, session, now],
        )?,
        None => conn.execute(
            "UPDATE disc_source_history
             SET unlinked_at = ?2
             WHERE disc_id = ?1 AND unlinked_at IS NULL",
            params![disc_id, now],
        )?,
    };
    if closed > 0 {
        // Fall back to whichever binding is still open, instead of declaring the
        // room source-less while other peers are still linked.
        refresh_legacy_pointer(conn, disc_id)?;
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
    pub binding_version: i64,
    pub disc_id: String,
    pub source_agent: String,
    pub source_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diverged_at: Option<String>,
}

pub fn list_all_source_bindings(conn: &Connection) -> Result<Vec<DiscSourceBinding>> {
    // KT-85 — read the OPEN history rows rather than the discussion's single
    // `source_*` column pair: a room shared by several CLIs has one binding per
    // joined session, and the columns can only hold the most recent one. They
    // stay as that "latest" pointer for the per-disc detail view.
    // `diverged_at` is a property of the discussion, not of one binding.
    let mut stmt = conn.prepare(
        "SELECT h.disc_id, h.source_agent, h.source_session_id, h.linked_at,
                d.diverged_at, h.binding_version
         FROM disc_source_history h
         JOIN discussions d ON d.id = h.disc_id
         WHERE h.unlinked_at IS NULL
         ORDER BY h.linked_at",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(DiscSourceBinding {
            disc_id: row.get(0)?,
            source_agent: row.get(1)?,
            source_session_id: row.get(2)?,
            imported_at: row.get(3)?,
            diverged_at: row.get(4)?,
            binding_version: row.get(5)?,
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
        "SELECT source_agent, source_session_id, linked_at, unlinked_at,
                binding_version
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
            binding_version: row.get(4)?,
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
    // KT-85 — keyed on a real open binding rather than the legacy pointer, which
    // is now only a "latest" hint and can lag behind a move or an unlink.
    conn.execute(
        "UPDATE discussions
         SET diverged_at = COALESCE(diverged_at, ?2)
         WHERE id = ?1
           AND EXISTS (
               SELECT 1 FROM disc_source_history
               WHERE disc_id = ?1 AND unlinked_at IS NULL
           )",
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

/// Resolve the durable Kronn message id for one imported/live source message.
/// Used by idempotent callers that need to perform a follow-up action (for
/// example pinning an uploaded attachment) even when the append itself was a
/// duplicate retry.
pub fn message_id_for_source_id(
    conn: &Connection,
    disc_id: &str,
    source_msg_id: &str,
) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT id FROM messages
             WHERE discussion_id = ?1 AND source_msg_id = ?2",
            params![disc_id, source_msg_id],
            |row| row.get(0),
        )
        .optional()?)
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

pub fn search_discussions(
    conn: &Connection,
    q: &str,
    limit: u32,
    include_notes: bool,
) -> Result<Vec<DiscSearchHit>> {
    let pattern = format!("%{}%", q.replace('%', "\\%").replace('_', "\\_"));
    let lim = limit.clamp(1, 50);

    let mut stmt = conn.prepare(
        "SELECT d.id, d.title, d.source_agent, d.source_session_id,
                COALESCE(
                    (SELECT m.content FROM messages m
                     WHERE m.discussion_id = d.id
                       AND (?3 = 1 OR m.channel = 'main')
                       AND m.content LIKE ?1 ESCAPE '\\'
                     ORDER BY m.sort_order ASC LIMIT 1),
                    d.title
                ) AS snippet
         FROM discussions d
         WHERE d.title LIKE ?1 ESCAPE '\\'
            OR EXISTS (
                SELECT 1 FROM messages m
                WHERE m.discussion_id = d.id
                  AND (?3 = 1 OR m.channel = 'main')
                  AND m.content LIKE ?1 ESCAPE '\\'
            )
         ORDER BY d.updated_at DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, lim as i64, include_notes], |row| {
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

/// KT-65 — one matching MESSAGE, not one matching discussion.
///
/// `search_discussions` above answers "which rooms mention X" with a single
/// snippet; it cannot take the reader to the message that matched. This shape
/// carries the message identity so the UI can open the exact turn.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MessageSearchHit {
    pub disc_id: String,
    pub disc_title: String,
    pub message_id: String,
    pub sort_order: i64,
    pub role: String,
    pub timestamp: String,
    /// Excerpt centred on the match, not the head of the message — a hit 3 000
    /// characters in is otherwise invisible in the result list.
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_pseudo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// Filters for [`search_messages`]. Every field is optional; combining them
/// narrows the result set (AND semantics).
#[derive(Debug, Clone, Default)]
pub struct MessageSearchFilters<'a> {
    pub discussion_id: Option<&'a str>,
    pub project_id: Option<&'a str>,
    /// Matches `messages.agent_type` OR `messages.author_pseudo`: from the
    /// reader's point of view "who said it" is one question, whether the author
    /// was an agent or a federated human.
    pub author: Option<&'a str>,
    /// RFC3339 bounds, inclusive. Timestamps are stored as sortable strings.
    pub since: Option<&'a str>,
    pub until: Option<&'a str>,
}

/// Excerpt of `content` around the first case-insensitive hit on `needle`.
fn snippet_around(content: &str, needle: &str, window: usize) -> String {
    let lower_content = content.to_lowercase();
    let hit = lower_content.find(&needle.to_lowercase());
    let chars: Vec<char> = content.chars().collect();
    let hit_char = hit
        .map(|byte_idx| content[..byte_idx].chars().count())
        .unwrap_or(0);
    let start = hit_char.saturating_sub(window / 2);
    let end = (start + window).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// Global discussion search with combinable filters. Message-content matches
/// return the exact message. A title/id match returns only the discussion's
/// latest message, which gives the UI a stable jump target without duplicating
/// every message in that room. The SQL remains bounded (LIMIT + OFFSET, newest
/// first) so a large history can't stream the whole database to the client.
pub fn search_messages(
    conn: &Connection,
    q: &str,
    filters: &MessageSearchFilters<'_>,
    limit: u32,
    offset: u32,
) -> Result<Vec<MessageSearchHit>> {
    let trimmed = q.trim();
    let escaped = trimmed.replace('%', "\\%").replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    // UUIDs and legacy copyable ids are prefix-oriented. Requiring a useful
    // length + id-safe charset prevents ordinary short words ("de", "ab")
    // from accidentally matching dozens of UUID substrings.
    let id_prefix_pattern = (trimmed.chars().count() >= 6
        && trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-'))
    .then(|| format!("{escaped}%"));
    let lim = limit.clamp(1, 50) as i64;
    let off = offset.min(10_000) as i64;

    let mut sql = String::from(
        "SELECT m.discussion_id, d.title, m.id, m.sort_order, m.role, m.timestamp,
                m.content, m.agent_type, m.author_pseudo, d.project_id
           FROM messages m
           JOIN discussions d ON d.id = m.discussion_id
          WHERE (
                m.content LIKE ?1 ESCAPE '\\'
                OR (
                    (d.title LIKE ?1 ESCAPE '\\'",
    );
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(pattern.clone())];
    if let Some(id_pattern) = id_prefix_pattern {
        sql.push_str(" OR d.id LIKE ?");
        sql.push_str(&(binds.len() + 1).to_string());
        sql.push_str(" ESCAPE '\\'");
        binds.push(Box::new(id_pattern));
    }
    sql.push_str(
        ")
                    AND m.id = (
                        SELECT latest.id
                          FROM messages latest
                         WHERE latest.discussion_id = d.id
                         ORDER BY latest.sort_order DESC
                         LIMIT 1
                    )
                )
          )",
    );

    if let Some(disc) = filters.discussion_id {
        sql.push_str(" AND m.discussion_id = ?");
        sql.push_str(&(binds.len() + 1).to_string());
        binds.push(Box::new(disc.to_string()));
    }
    if let Some(project) = filters.project_id {
        sql.push_str(" AND d.project_id = ?");
        sql.push_str(&(binds.len() + 1).to_string());
        binds.push(Box::new(project.to_string()));
    }
    if let Some(author) = filters.author {
        sql.push_str(" AND (m.agent_type = ?");
        sql.push_str(&(binds.len() + 1).to_string());
        binds.push(Box::new(author.to_string()));
        sql.push_str(" OR m.author_pseudo = ?");
        sql.push_str(&(binds.len() + 1).to_string());
        binds.push(Box::new(author.to_string()));
        sql.push(')');
    }
    if let Some(since) = filters.since {
        sql.push_str(" AND m.timestamp >= ?");
        sql.push_str(&(binds.len() + 1).to_string());
        binds.push(Box::new(since.to_string()));
    }
    if let Some(until) = filters.until {
        sql.push_str(" AND m.timestamp <= ?");
        sql.push_str(&(binds.len() + 1).to_string());
        binds.push(Box::new(until.to_string()));
    }
    sql.push_str(" ORDER BY m.timestamp DESC, m.sort_order DESC LIMIT ?");
    sql.push_str(&(binds.len() + 1).to_string());
    binds.push(Box::new(lim));
    sql.push_str(" OFFSET ?");
    sql.push_str(&(binds.len() + 1).to_string());
    binds.push(Box::new(off));

    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(params.as_slice(), |row| {
        let content: String = row.get(6)?;
        Ok(MessageSearchHit {
            disc_id: row.get(0)?,
            disc_title: row.get(1)?,
            message_id: row.get(2)?,
            sort_order: row.get(3)?,
            role: row.get(4)?,
            timestamp: row.get(5)?,
            snippet: snippet_around(&content, q, 160),
            agent_type: row.get(7)?,
            author_pseudo: row.get(8)?,
            project_id: row.get(9)?,
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
                project_id TEXT,
                message_count INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                source_agent TEXT,
                source_session_id TEXT,
                source_binding_version INTEGER,
                imported_at DATETIME,
                diverged_at DATETIME
            );
            CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                discussion_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                sort_order INTEGER NOT NULL,
                source_msg_id TEXT,
                agent_type TEXT,
                author_pseudo TEXT,
                channel TEXT NOT NULL DEFAULT 'main',
                timestamp TEXT NOT NULL DEFAULT '2026-05-15T10:00:00Z'
            );
            CREATE INDEX idx_msg_source_dedup ON messages(discussion_id, source_msg_id);
            CREATE TABLE disc_source_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                disc_id TEXT NOT NULL,
                source_agent TEXT NOT NULL,
                source_session_id TEXT NOT NULL,
                linked_at DATETIME NOT NULL,
                unlinked_at DATETIME,
                binding_version INTEGER NOT NULL DEFAULT 1
            );
            CREATE INDEX idx_disc_src_hist_lookup ON disc_source_history(source_agent, source_session_id);
            CREATE INDEX idx_disc_src_hist_disc ON disc_source_history(disc_id);
            CREATE UNIQUE INDEX idx_disc_source_session_one_open
                ON disc_source_history(source_agent, source_session_id)
                WHERE unlinked_at IS NULL;
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

    /// KT-85 — found live by @user: in a cross-agent room the last agent to
    /// bind was closing everyone else's binding, so every other peer's
    /// `disc_find_by_session` went silently empty after an MCP reload.
    #[test]
    fn several_agents_can_own_a_binding_on_the_same_discussion() {
        let conn = fresh_conn();
        bind_to_source(&conn, "d-alpha", "Codex", "cli-codex-1").unwrap();
        bind_to_source(&conn, "d-alpha", "ClaudeCode", "cli-claude-1").unwrap();
        // A third bind by a NEW Codex bridge, as happens after an MCP reload.
        bind_to_source(&conn, "d-alpha", "Codex", "cli-codex-2").unwrap();

        // Every session still resolves to the room — none was evicted.
        for session in ["cli-codex-1", "cli-claude-1", "cli-codex-2"] {
            let agent = if session.contains("claude") {
                "ClaudeCode"
            } else {
                "Codex"
            };
            assert_eq!(
                find_disc_by_source_session(&conn, agent, session)
                    .unwrap()
                    .as_deref(),
                Some("d-alpha"),
                "{session} lost its binding",
            );
        }

        // And the batch reader exposes all three, not just the latest.
        let bindings = list_all_source_bindings(&conn).unwrap();
        let mut seen: Vec<&str> = bindings
            .iter()
            .filter(|b| b.disc_id == "d-alpha")
            .map(|b| b.source_session_id.as_str())
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, ["cli-claude-1", "cli-codex-1", "cli-codex-2"]);
    }

    /// The invariant that must NOT loosen: one concrete session owns one
    /// discussion. Moving it releases the previous room.
    #[test]
    fn one_session_still_owns_a_single_discussion() {
        let conn = fresh_conn();
        bind_to_source(&conn, "d-alpha", "Codex", "cli-codex-1").unwrap();
        bind_to_source(&conn, "d-beta", "Codex", "cli-codex-1").unwrap();

        assert_eq!(
            find_disc_by_source_session(&conn, "Codex", "cli-codex-1")
                .unwrap()
                .as_deref(),
            Some("d-beta"),
        );
        let open_on_alpha = list_source_history(&conn, "d-alpha")
            .unwrap()
            .into_iter()
            .filter(|h| h.unlinked_at.is_none())
            .count();
        assert_eq!(open_on_alpha, 0, "moving a session must free the old room");
    }

    /// KT-85 review (Codex) — the detail panel reads the CURRENT binding. A scan
    /// of every open binding returned the oldest once a room could hold several.
    #[test]
    fn current_source_binding_is_the_most_recent_one() {
        let conn = fresh_conn();
        bind_to_source(&conn, "d-alpha", "Codex", "cli-codex-1").unwrap();
        bind_to_source(&conn, "d-alpha", "ClaudeCode", "cli-claude-1").unwrap();

        let current = current_source_binding(&conn, "d-alpha")
            .unwrap()
            .expect("a shared room still has a current binding");
        assert_eq!(current.source_session_id, "cli-claude-1");
        assert_eq!(current.source_agent, "ClaudeCode");

        // The oldest one is what a naive ascending scan would have surfaced.
        let scanned = list_all_source_bindings(&conn)
            .unwrap()
            .into_iter()
            .find(|b| b.disc_id == "d-alpha")
            .unwrap();
        assert_eq!(
            scanned.source_session_id, "cli-codex-1",
            "guard: the scan really does hit the oldest, which is why the \
             endpoint must not use it",
        );

        // Releasing the current one promotes the survivor.
        unbind_from_source(&conn, "d-alpha", Some(("ClaudeCode", "cli-claude-1"))).unwrap();
        assert_eq!(
            current_source_binding(&conn, "d-alpha")
                .unwrap()
                .unwrap()
                .source_session_id,
            "cli-codex-1",
        );
        // And the last release leaves no current binding at all.
        unbind_from_source(&conn, "d-alpha", Some(("Codex", "cli-codex-1"))).unwrap();
        assert!(current_source_binding(&conn, "d-alpha").unwrap().is_none());
    }

    /// KT-85 review (Codex) — "release MY link" must not evict the other peers.
    #[test]
    fn unlinking_one_peer_keeps_the_other_bindings() {
        let conn = fresh_conn();
        bind_to_source(&conn, "d-alpha", "Codex", "cli-codex-1").unwrap();
        bind_to_source(&conn, "d-alpha", "ClaudeCode", "cli-claude-1").unwrap();

        let closed = unbind_from_source(&conn, "d-alpha", Some(("Codex", "cli-codex-1"))).unwrap();
        assert!(closed);
        assert!(
            find_disc_by_source_session(&conn, "Codex", "cli-codex-1")
                .unwrap()
                .is_none(),
            "the caller's own binding is released",
        );
        assert_eq!(
            find_disc_by_source_session(&conn, "ClaudeCode", "cli-claude-1")
                .unwrap()
                .as_deref(),
            Some("d-alpha"),
            "the peer that did NOT unlink keeps its binding",
        );
        // The legacy pointer falls back to the survivor instead of going NULL.
        let pointer: Option<String> = conn
            .query_row(
                "SELECT source_session_id FROM discussions WHERE id = 'd-alpha'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pointer.as_deref(), Some("cli-claude-1"));
    }

    /// The legacy `discussions.source_*` pointer must name the MOST RECENT open
    /// binding — the detail panel reads it, and an ascending scan made it the
    /// oldest one.
    #[test]
    fn the_legacy_pointer_tracks_the_most_recent_open_binding() {
        let conn = fresh_conn();
        bind_to_source(&conn, "d-alpha", "Codex", "cli-codex-1").unwrap();
        bind_to_source(&conn, "d-alpha", "ClaudeCode", "cli-claude-1").unwrap();
        let pointer: Option<String> = conn
            .query_row(
                "SELECT source_session_id FROM discussions WHERE id = 'd-alpha'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pointer.as_deref(), Some("cli-claude-1"));
    }

    /// Moving a session away from a SHARED room must not blank that room's
    /// pointer: other peers are still bound there.
    #[test]
    fn moving_a_session_out_of_a_shared_room_keeps_the_room_bound() {
        let conn = fresh_conn();
        bind_to_source(&conn, "d-alpha", "Codex", "cli-codex-1").unwrap();
        bind_to_source(&conn, "d-alpha", "ClaudeCode", "cli-claude-1").unwrap();
        // Codex takes its session to another discussion.
        bind_to_source(&conn, "d-beta", "Codex", "cli-codex-1").unwrap();

        let (agent, session): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT source_agent, source_session_id FROM discussions WHERE id = 'd-alpha'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(agent.as_deref(), Some("ClaudeCode"));
        assert_eq!(session.as_deref(), Some("cli-claude-1"));
        // And divergence still applies to the room, which is still bound.
        mark_diverged(&conn, "d-alpha").unwrap();
        assert!(get_diverged_at(&conn, "d-alpha").unwrap().is_some());
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
        assert_eq!(hist[0].binding_version, SOURCE_BINDING_VERSION);
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
    fn a_second_cli_binding_joins_the_discussion_instead_of_taking_it_over() {
        // CONTRACT CHANGE (KT-85). This test previously asserted
        // "last-link-wins": a second (agent, session) closed the first, so a
        // discussion had exactly one owner. That modelled the 0.8.4 cross-agent
        // HANDOFF (a thread imported from ClaudeCode, then taken over by
        // Cursor). It is wrong for the multi-agent rooms of 0.8.6+, where
        // several CLIs work the same discussion at once: evicting the others
        // made their reconnection lookup silently empty. A deliberate handoff is
        // still expressible — `disc_unlink`, or moving the session to another
        // discussion, both close the old row explicitly.
        let conn = fresh_conn();
        bind_to_source(&conn, "d-alpha", "ClaudeCode", "sess-A").unwrap();
        bind_to_source(&conn, "d-alpha", "Cursor", "sess-B").unwrap();

        let hist = list_source_history(&conn, "d-alpha").unwrap();
        assert_eq!(hist.len(), 2);
        assert!(
            hist.iter().all(|h| h.unlinked_at.is_none()),
            "both CLIs keep their binding on a shared discussion",
        );

        // Each pair resolves to the room on its own.
        for (agent, session) in [("Cursor", "sess-B"), ("ClaudeCode", "sess-A")] {
            assert_eq!(
                find_disc_by_source_session(&conn, agent, session)
                    .unwrap()
                    .as_deref(),
                Some("d-alpha"),
                "{agent}/{session} must still resolve",
            );
        }
    }

    #[test]
    fn rebinding_same_cli_session_moves_it_to_one_discussion() {
        let conn = fresh_conn();
        bind_to_source(&conn, "d-alpha", "Codex", "sess-shared").unwrap();
        bind_to_source(&conn, "d-beta", "Codex", "sess-shared").unwrap();

        assert_eq!(
            find_disc_by_source_session(&conn, "Codex", "sess-shared")
                .unwrap()
                .as_deref(),
            Some("d-beta"),
        );
        let alpha_source: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT source_agent, source_session_id
                   FROM discussions WHERE id = 'd-alpha'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(alpha_source, (None, None));
        let alpha_history = list_source_history(&conn, "d-alpha").unwrap();
        assert!(alpha_history[0].unlinked_at.is_some());
        let beta_history = list_source_history(&conn, "d-beta").unwrap();
        assert!(beta_history[0].unlinked_at.is_none());
    }

    #[test]
    fn unbind_clears_columns_and_closes_chain() {
        let conn = fresh_conn();
        bind_to_source(&conn, "d-alpha", "ClaudeCode", "sess-Z").unwrap();
        let closed = unbind_from_source(&conn, "d-alpha", None).unwrap();
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
        let closed = unbind_from_source(&conn, "d-beta", None).unwrap();
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
        let hits = search_discussions(&conn, "ClaudeCode", 10, false).unwrap();
        assert_eq!(hits.len(), 1, "matches the m1 content body");
        assert_eq!(hits[0].disc_id, "d-alpha");

        let hits2 = search_discussions(&conn, "Second", 10, false).unwrap();
        assert_eq!(hits2.len(), 1, "matches the d-beta title");
        assert_eq!(hits2[0].disc_id, "d-beta");

        conn.execute(
            "INSERT INTO messages
                 (id, discussion_id, role, channel, content, sort_order, timestamp)
             VALUES ('note-only', 'd-alpha', 'User', 'note',
                     'private-note-keyword', 20, '2026-05-15T12:00:00Z')",
            [],
        )
        .unwrap();
        assert!(
            search_discussions(&conn, "private-note-keyword", 10, false)
                .unwrap()
                .is_empty(),
            "notes stay out of default agent search"
        );
        assert_eq!(
            search_discussions(&conn, "private-note-keyword", 10, true)
                .unwrap()
                .len(),
            1,
            "explicit include_notes reveals note content"
        );
    }

    // ─── KT-65 — message-level search ───────────────────────────────────────

    /// Seed a richer history than `fresh_conn`: two rooms, two authors, two
    /// dates, and a long body so the excerpt logic has something to centre on.
    fn search_conn() -> Connection {
        let conn = fresh_conn();
        conn.execute_batch(
            "UPDATE discussions SET project_id = 'proj-1' WHERE id = 'd-alpha';
             UPDATE discussions SET project_id = 'proj-2' WHERE id = 'd-beta';
             INSERT INTO messages
                 (id, discussion_id, role, content, sort_order, agent_type, author_pseudo, timestamp)
             VALUES
                 ('s1', 'd-alpha', 'Agent', 'le probe Fastly répond 200 en authentifié', 10,
                  'Codex', NULL, '2026-07-01T10:00:00Z'),
                 ('s2', 'd-alpha', 'User', 'et le probe Fastly côté Docker ?', 11,
                  NULL, 'Romu - mac', '2026-07-15T10:00:00Z'),
                 ('s3', 'd-beta', 'Agent', 'aucun rapport avec Fastly ici', 12,
                  'ClaudeCode', NULL, '2026-07-20T10:00:00Z');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn message_search_returns_the_matching_message_not_just_the_room() {
        let conn = search_conn();
        let hits = search_messages(&conn, "probe Fastly", &Default::default(), 10, 0).unwrap();
        assert_eq!(hits.len(), 2, "two messages mention it, in one room");
        // Newest first, so the reader lands on the most recent mention.
        assert_eq!(hits[0].message_id, "s2");
        assert_eq!(hits[0].disc_title, "First disc");
        assert_eq!(hits[0].author_pseudo.as_deref(), Some("Romu - mac"));
        assert_eq!(hits[1].message_id, "s1");
        assert_eq!(hits[1].agent_type.as_deref(), Some("Codex"));
    }

    #[test]
    fn message_search_also_finds_a_discussion_by_title_or_id_once() {
        let conn = search_conn();

        let by_title = search_messages(&conn, "First disc", &Default::default(), 10, 0).unwrap();
        assert_eq!(
            by_title.len(),
            1,
            "a title match must not duplicate every message"
        );
        assert_eq!(by_title[0].disc_id, "d-alpha");
        assert_eq!(by_title[0].message_id, "s2", "jump to the latest message");

        let by_id = search_messages(&conn, "d-alph", &Default::default(), 10, 0).unwrap();
        assert_eq!(by_id.len(), 1, "an id prefix resolves one discussion hit");
        assert_eq!(by_id[0].disc_id, "d-alpha");
        assert_eq!(by_id[0].message_id, "s2");

        let short_word = search_messages(&conn, "ph", &Default::default(), 10, 0).unwrap();
        assert!(
            short_word.is_empty(),
            "short ordinary words must not match arbitrary id substrings"
        );
    }

    #[test]
    fn message_search_combines_filters_with_and_semantics() {
        let conn = search_conn();

        let by_room = MessageSearchFilters {
            discussion_id: Some("d-beta"),
            ..Default::default()
        };
        let hits = search_messages(&conn, "Fastly", &by_room, 10, 0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message_id, "s3");

        let by_project = MessageSearchFilters {
            project_id: Some("proj-1"),
            ..Default::default()
        };
        assert_eq!(
            search_messages(&conn, "Fastly", &by_project, 10, 0)
                .unwrap()
                .len(),
            2
        );

        // `author` covers an agent type OR a federated human pseudo.
        let by_agent = MessageSearchFilters {
            author: Some("Codex"),
            ..Default::default()
        };
        assert_eq!(
            search_messages(&conn, "Fastly", &by_agent, 10, 0).unwrap()[0].message_id,
            "s1"
        );
        let by_human = MessageSearchFilters {
            author: Some("Romu - mac"),
            ..Default::default()
        };
        assert_eq!(
            search_messages(&conn, "Fastly", &by_human, 10, 0).unwrap()[0].message_id,
            "s2"
        );

        let window = MessageSearchFilters {
            since: Some("2026-07-10T00:00:00Z"),
            until: Some("2026-07-18T00:00:00Z"),
            ..Default::default()
        };
        let dated = search_messages(&conn, "Fastly", &window, 10, 0).unwrap();
        assert_eq!(dated.len(), 1, "only the mid-July message is in range");
        assert_eq!(dated[0].message_id, "s2");

        // Combined: the room AND a window that excludes its only hit → empty.
        let contradictory = MessageSearchFilters {
            discussion_id: Some("d-beta"),
            until: Some("2026-07-01T00:00:00Z"),
            ..Default::default()
        };
        assert!(search_messages(&conn, "Fastly", &contradictory, 10, 0)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn message_search_is_bounded_and_pageable() {
        let conn = search_conn();
        let page1 = search_messages(&conn, "Fastly", &Default::default(), 1, 0).unwrap();
        let page2 = search_messages(&conn, "Fastly", &Default::default(), 1, 1).unwrap();
        assert_eq!(page1.len(), 1);
        assert_eq!(page2.len(), 1);
        assert_ne!(
            page1[0].message_id, page2[0].message_id,
            "offset must advance"
        );

        // An absurd limit is clamped rather than honoured — one keystroke must
        // never stream the whole history.
        let huge = search_messages(&conn, "Fastly", &Default::default(), 10_000, 0).unwrap();
        assert!(huge.len() <= 50);
    }

    #[test]
    fn snippet_is_centred_on_the_hit_not_the_start_of_the_message() {
        let long = format!("{}NEEDLE{}", "a".repeat(400), "b".repeat(400));
        let snippet = snippet_around(&long, "needle", 60);
        assert!(snippet.contains("NEEDLE"), "got: {snippet}");
        assert!(
            snippet.starts_with('…') && snippet.ends_with('…'),
            "got: {snippet}"
        );
        assert!(snippet.chars().count() <= 62);

        // No match (filters matched, the excerpt still has to render something).
        let head = snippet_around("court message", "absent", 60);
        assert!(head.starts_with("court"));
    }

    #[test]
    fn message_search_escapes_like_metachars() {
        let conn = search_conn();
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, sort_order, timestamp)
             VALUES ('pct', 'd-alpha', 'User', 'couverture 100% atteinte', 20, '2026-07-25T10:00:00Z')",
            [],
        )
        .unwrap();
        let hits = search_messages(&conn, "100%", &Default::default(), 10, 0).unwrap();
        assert_eq!(hits.len(), 1, "`%` must be literal, not a wildcard");
        assert_eq!(hits[0].message_id, "pct");
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
        let hits = search_discussions(&conn, "100%", 10, false).unwrap();
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
