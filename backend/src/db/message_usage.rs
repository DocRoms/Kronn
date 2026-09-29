//! The counters a reply's cost was computed from — KT-894.
//!
//! `messages.tokens_used` is a total, and for Codex it contains the cache reads:
//! a 25.2M-token run was 98.6% cache. Summing totals and calling the result "what
//! this run consumed" is what made a ~$13 run read $111. These rows keep the
//! parts apart so a total can be shown as real input, cache and output.
//!
//! `None` means NOT REPORTED, never zero, on every counter.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::pricing::{TokenCounters, UnknownCost};

/// What is worth keeping about one reply's usage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MessageUsage {
    /// `None` when only a total was reported, or the cache split is unknown.
    pub counters: Option<TokenCounters>,
    /// Why `messages.cost_usd` is NULL for this reply, when it is.
    pub cost_unknown: Option<UnknownCost>,
}

/// Store one reply's usage. Nothing is written when there is nothing to say.
///
/// Counters are kept only when the cache read is known: without it the input
/// cannot be told apart from cache, and a stored figure would look like a
/// measurement of "real input" that it is not.
pub fn record(conn: &Connection, message_id: &str, usage: &MessageUsage) -> Result<()> {
    let counters = usage
        .counters
        .filter(|counters| counters.cache_read_tokens.is_some());
    if counters.is_none() && usage.cost_unknown.is_none() {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO message_usage (
             message_id, input_tokens, cache_read_tokens, cache_write_tokens,
             output_tokens, cost_unknown_reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(message_id) DO UPDATE SET
             input_tokens = excluded.input_tokens,
             cache_read_tokens = excluded.cache_read_tokens,
             cache_write_tokens = excluded.cache_write_tokens,
             output_tokens = excluded.output_tokens,
             cost_unknown_reason = excluded.cost_unknown_reason",
        params![
            message_id,
            counters.map(|c| c.input_tokens as i64),
            counters.and_then(|c| c.cache_read_tokens).map(|n| n as i64),
            counters
                .and_then(|c| c.cache_write_tokens)
                .map(|n| n as i64),
            counters.map(|c| c.output_tokens as i64),
            usage.cost_unknown.map(|reason| reason.reason()),
        ],
    )?;
    Ok(())
}

/// A discussion's in-app tokens with the cache told apart from the real input.
///
/// Covers only the replies that reported their counters: `messages` says how
/// many, so a reader can compare it with the reply count beside it instead of
/// assuming these figures are the whole total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InAppTokenBreakdown {
    /// Replies counted in the figures below.
    pub messages: i64,
    /// Input that was neither served from cache nor written to it.
    pub input_tokens: i64,
    /// Input served from the prompt cache. Reported by every reply counted.
    pub cache_read_tokens: i64,
    /// Sum over the replies that reported a cache write only; `None` when none did.
    pub cache_write_tokens: Option<i64>,
    pub output_tokens: i64,
}

/// The breakdown of one discussion's in-app replies. `None` when no reply
/// reported its counters — unknown, not "all of it was input".
pub fn breakdown_for_discussion(
    conn: &Connection,
    disc_id: &str,
) -> Result<Option<InAppTokenBreakdown>> {
    let (messages, input, cache_read, cache_write, cache_write_reported, output) = conn.query_row(
        "SELECT COUNT(*),
                    COALESCE(SUM(u.input_tokens), 0),
                    COALESCE(SUM(u.cache_read_tokens), 0),
                    COALESCE(SUM(u.cache_write_tokens), 0),
                    COALESCE(SUM(u.cache_write_tokens IS NOT NULL), 0),
                    COALESCE(SUM(u.output_tokens), 0)
               FROM messages m
               JOIN message_usage u ON u.message_id = m.id
              WHERE m.discussion_id = ?1
                AND m.tokens_used > 0
                AND u.cache_read_tokens IS NOT NULL",
        [disc_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        },
    )?;
    Ok((messages > 0).then_some(InAppTokenBreakdown {
        messages,
        input_tokens: input,
        cache_read_tokens: cache_read,
        cache_write_tokens: (cache_write_reported > 0).then_some(cache_write),
        output_tokens: output,
    }))
}

/// The distinct reasons this discussion's replies have no cost, sorted. Empty
/// when every reply that consumed tokens was priced.
pub fn unknown_cost_reasons(conn: &Connection, disc_id: &str) -> Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT DISTINCT u.cost_unknown_reason
           FROM messages m
           JOIN message_usage u ON u.message_id = m.id
          WHERE m.discussion_id = ?1
            AND m.tokens_used > 0
            AND m.cost_usd IS NULL
            AND u.cost_unknown_reason IS NOT NULL
          ORDER BY u.cost_unknown_reason",
    )?;
    let reasons = statement
        .query_map([disc_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(reasons)
}

#[cfg(test)]
#[path = "message_usage_test.rs"]
mod message_usage_test;
