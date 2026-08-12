//! Native token counters for joined CLI sessions — KT-190.
//!
//! `None` means NOT MEASURED, and that distinction is the point of this module.
//! Vibe publishes no cache breakdown, so its cache counters are absent rather
//! than zero; collapsing the two would let a report assert something about a
//! field nobody measured. Every accessor here preserves it.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One vendor's counters for one CLI session. A `None` counter is a counter the
/// vendor does not publish — never a zero.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CliSessionTelemetry {
    pub cli_session_pk: i64,
    pub vendor: String,
    /// Where the numbers came from, e.g. `claude-code-transcript`. Nothing in
    /// this table is an estimate, and a consumer must be able to prove it.
    pub provenance: String,
    pub input_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub measured_responses: Option<i64>,
    pub models_json: Option<String>,
    pub window_start: Option<String>,
    pub window_end: Option<String>,
    /// The VENDOR's own cost figure when it publishes one (Vibe does, Claude
    /// Code does not). Never merged with a Kronn estimate.
    pub vendor_cost_usd: Option<f64>,
    pub read_offset: i64,
    pub updated_at: String,
}

impl CliSessionTelemetry {
    /// Everything the vendor reported, cache reads included. `None` when not one
    /// counter was measured — a total of 0 over four absent counters would be a
    /// fabricated figure.
    pub fn traffic_tokens(&self) -> Option<i64> {
        let parts = [
            self.input_tokens,
            self.cache_creation_tokens,
            self.cache_read_tokens,
            self.output_tokens,
        ];
        if parts.iter().all(Option::is_none) {
            return None;
        }
        Some(parts.iter().filter_map(|part| *part).sum())
    }

    /// Traffic minus cache reads, which bill at roughly a tenth. `None` when the
    /// vendor does not report cache reads: without them "billable" cannot be
    /// derived, and guessing it is how a release note stops being checkable.
    pub fn billable_tokens(&self) -> Option<i64> {
        let cache_read = self.cache_read_tokens?;
        Some(self.traffic_tokens()? - cache_read)
    }
}

/// Telemetry coverage for one agent type, as a dashboard must state it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TelemetryCoverage {
    pub agent_type: String,
    /// CLI sessions of this agent type that exist.
    pub sessions: i64,
    /// Sessions with a telemetry row. The rest are UNKNOWN, not free.
    pub attributed: i64,
    /// Attributed sessions whose counters are all absent — a vendor with no
    /// collector yet. Counted apart so "attributed" cannot flatter itself.
    pub attributed_without_counters: i64,
}

impl TelemetryCoverage {
    /// Share of sessions with at least one real counter. `None` when there are
    /// no sessions: 0% would read as a failure where there is simply nothing to
    /// measure yet.
    pub fn measured_ratio(&self) -> Option<f64> {
        if self.sessions == 0 {
            return None;
        }
        let measured = self.attributed - self.attributed_without_counters;
        Some(measured as f64 / self.sessions as f64)
    }
}

/// Insert or update one session's counters, keeping absence absent.
pub fn upsert(conn: &Connection, row: &CliSessionTelemetry) -> Result<()> {
    conn.execute(
        "INSERT INTO cli_session_telemetry (
             cli_session_pk, vendor, provenance, input_tokens,
             cache_creation_tokens, cache_read_tokens, output_tokens,
             measured_responses, models_json, window_start, window_end,
             vendor_cost_usd, read_offset, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(cli_session_pk) DO UPDATE SET
             vendor = excluded.vendor,
             provenance = excluded.provenance,
             input_tokens = excluded.input_tokens,
             cache_creation_tokens = excluded.cache_creation_tokens,
             cache_read_tokens = excluded.cache_read_tokens,
             output_tokens = excluded.output_tokens,
             measured_responses = excluded.measured_responses,
             models_json = excluded.models_json,
             window_start = COALESCE(cli_session_telemetry.window_start,
                                     excluded.window_start),
             window_end = excluded.window_end,
             vendor_cost_usd = excluded.vendor_cost_usd,
             -- Monotonic: a stale or replayed report must never rewind the
             -- cursor, which would re-collect and double-count.
             read_offset = MAX(cli_session_telemetry.read_offset,
                               excluded.read_offset),
             updated_at = excluded.updated_at",
        params![
            row.cli_session_pk,
            row.vendor,
            row.provenance,
            row.input_tokens,
            row.cache_creation_tokens,
            row.cache_read_tokens,
            row.output_tokens,
            row.measured_responses,
            row.models_json,
            row.window_start,
            row.window_end,
            row.vendor_cost_usd,
            row.read_offset,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// The byte cursor a collector should resume from. 0 when nothing was collected
/// yet, which is also the correct value for a snapshot-based vendor.
pub fn read_offset(conn: &Connection, cli_session_pk: i64) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT read_offset FROM cli_session_telemetry WHERE cli_session_pk = ?1",
            params![cli_session_pk],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

pub fn get(conn: &Connection, cli_session_pk: i64) -> Result<Option<CliSessionTelemetry>> {
    Ok(conn
        .query_row(
            "SELECT cli_session_pk, vendor, provenance, input_tokens,
                    cache_creation_tokens, cache_read_tokens, output_tokens,
                    measured_responses, models_json, window_start, window_end,
                    vendor_cost_usd, read_offset, updated_at
             FROM cli_session_telemetry WHERE cli_session_pk = ?1",
            params![cli_session_pk],
            |row| {
                Ok(CliSessionTelemetry {
                    cli_session_pk: row.get(0)?,
                    vendor: row.get(1)?,
                    provenance: row.get(2)?,
                    input_tokens: row.get(3)?,
                    cache_creation_tokens: row.get(4)?,
                    cache_read_tokens: row.get(5)?,
                    output_tokens: row.get(6)?,
                    measured_responses: row.get(7)?,
                    models_json: row.get(8)?,
                    window_start: row.get(9)?,
                    window_end: row.get(10)?,
                    vendor_cost_usd: row.get(11)?,
                    read_offset: row.get(12)?,
                    updated_at: row.get(13)?,
                })
            },
        )
        .optional()?)
}

/// Coverage per agent type. A session with no telemetry row is UNKNOWN, and this
/// is what makes that visible instead of letting it read as zero cost.
pub fn coverage(conn: &Connection) -> Result<Vec<TelemetryCoverage>> {
    let mut statement = conn.prepare(
        "SELECT s.agent_type,
                COUNT(*) AS sessions,
                SUM(t.cli_session_pk IS NOT NULL) AS attributed,
                SUM(t.cli_session_pk IS NOT NULL
                    AND t.input_tokens IS NULL
                    AND t.cache_creation_tokens IS NULL
                    AND t.cache_read_tokens IS NULL
                    AND t.output_tokens IS NULL) AS empty_rows
         FROM discussion_sessions s
         LEFT JOIN cli_session_telemetry t ON t.cli_session_pk = s.id
         GROUP BY s.agent_type
         ORDER BY s.agent_type",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok(TelemetryCoverage {
                agent_type: row.get(0)?,
                sessions: row.get(1)?,
                attributed: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                attributed_without_counters: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// One timestamped response from a vendor transcript, as the bridge reports it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResponseUsage {
    /// RFC3339 instant the vendor recorded for this response.
    pub at: String,
    #[serde(default)]
    pub input: Option<i64>,
    #[serde(default)]
    pub cache_creation: Option<i64>,
    #[serde(default)]
    pub cache_read: Option<i64>,
    #[serde(default)]
    pub output: Option<i64>,
}

impl ResponseUsage {
    fn total(&self) -> i64 {
        [
            self.input,
            self.cache_creation,
            self.cache_read,
            self.output,
        ]
        .iter()
        .filter_map(|part| *part)
        .sum()
    }
}

/// Stamp each of a CLI session's messages with what the SESSION had cost by then.
///
/// A joined CLI's messages have always read `tokens_used = 0` — 994 of them on
/// the real database, against 1 045 non-zero for the agents Kronn spawns. So the
/// UI could price an agent's bubble and not a CLI's.
///
/// A running total, not a per-message cost, and that choice is the substance
/// here. A CLI's spend cannot be cut up per message: between two room messages
/// it also read files, ran tests, and may have answered in ANOTHER room.
/// Charging that window to the message that follows would attribute work it did
/// not do. A cumulative figure claims nothing about the message — it states what
/// the session had spent at that instant, which is simply true. The gap between
/// two rows stays visible to anyone who wants to estimate a delta, without Kronn
/// asserting one.
///
/// Written to `session_tokens_at_message`, never to `tokens_used`: that column
/// means "this reply cost that much" for an agent, and a cumulative value in the
/// same slot would be rendered as a per-message cost.
///
/// `baseline` is what the session had already spent before this batch of
/// responses — the totals from earlier reports — so a report covering only the
/// newest slice still produces true absolute figures.
///
/// Returns how many messages were stamped.
pub fn attribute_to_messages(
    conn: &Connection,
    cli_session_pk: i64,
    responses: &[ResponseUsage],
    baseline: i64,
) -> Result<usize> {
    if responses.is_empty() {
        return Ok(0);
    }
    // Messages this exact session authored, oldest first. `message_cli_authors`
    // is the durable link — matching on agent_type would charge a peer of the
    // same provider.
    let mut statement = conn.prepare(
        "SELECT m.id, m.timestamp
           FROM messages m
           JOIN message_cli_authors mca ON mca.message_id = m.id
          WHERE mca.cli_session_id = ?1
          ORDER BY m.sort_order ASC",
    )?;
    let messages: Vec<(String, String)> = statement
        .query_map(params![cli_session_pk], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if messages.is_empty() {
        return Ok(0);
    }

    let parsed: Vec<(String, chrono::DateTime<Utc>)> = messages
        .into_iter()
        .filter_map(|(id, stamp)| {
            chrono::DateTime::parse_from_rfc3339(&stamp)
                .ok()
                .map(|when| (id, when.with_timezone(&Utc)))
        })
        .collect();

    // Responses in time order, so the running total only ever moves forward.
    let mut timed: Vec<(chrono::DateTime<Utc>, i64)> = responses
        .iter()
        .filter_map(|response| {
            chrono::DateTime::parse_from_rfc3339(&response.at)
                .ok()
                // An unparseable instant cannot be placed against any message,
                // so it is left out rather than guessed into the wrong one.
                .map(|at| (at.with_timezone(&Utc), response.total()))
        })
        .collect();
    timed.sort_by_key(|(at, _)| *at);

    let mut stamped = 0;
    let mut running = baseline;
    let mut next = 0_usize;
    for (message_id, when) in &parsed {
        // Everything spent up to and including this message's instant.
        while next < timed.len() && timed[next].0 <= *when {
            running += timed[next].1;
            next += 1;
        }
        if running <= 0 {
            // Nothing measured yet at this point in the session. Leaving NULL
            // keeps "not measured" distinct from "cost nothing".
            continue;
        }
        // MAX, not assignment: a later report can re-read an overlapping window
        // when the cursor did not advance, and a cumulative figure that dropped
        // would be a visible lie — the session's spend only ever grows.
        stamped += conn.execute(
            "UPDATE messages
                SET session_tokens_at_message =
                        MAX(COALESCE(session_tokens_at_message, 0), ?2)
              WHERE id = ?1",
            params![message_id, running],
        )?;
    }
    Ok(stamped)
}

/// Token spend rolled up to one Kronn object (a task, a project, a discussion).
///
/// The counts are as important as the tokens. Summing the measured sessions and
/// presenting the result as "what this task cost" would be wrong whenever one
/// session was never measured — and that is the normal case today, since Codex
/// and Copilot have no collector. So the unmeasured count travels with the
/// figure, and the figure itself is `None` when nothing at all was measured.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ObjectSpend {
    /// Human-facing key: a task reference, a project id, a discussion id.
    pub object_key: String,
    pub sessions: i64,
    pub measured_sessions: i64,
    /// Sessions with no readable counter. Their cost is UNKNOWN, not zero, and a
    /// reader must see the number to judge the total beside it.
    pub unmeasured_sessions: i64,
    /// Everything the vendors reported, cache reads included. `None` when no
    /// session was measured.
    pub traffic_tokens: Option<i64>,
    /// Traffic minus cache reads. `None` unless EVERY measured session reports
    /// cache reads: mixing a vendor that splits caches (Claude Code) with one
    /// that does not (Vibe) makes the difference underivable, and summing what
    /// is available would understate the cache share instead of admitting the
    /// gap.
    pub billable_tokens: Option<i64>,
}

/// Spend per planning task, resolved through the discussions linked to it.
///
/// A join rather than duplicated foreign keys on the telemetry row: the links
/// already exist (session → discussion → task), and copying them would let the
/// copy drift from the truth.
pub fn spend_by_task(conn: &Connection) -> Result<Vec<ObjectSpend>> {
    rollup(
        conn,
        "SELECT 'KT-' || pt.task_number AS object_key,
                COUNT(*) AS sessions,
                SUM(t.cli_session_pk IS NOT NULL
                    AND (t.input_tokens IS NOT NULL
                         OR t.cache_creation_tokens IS NOT NULL
                         OR t.cache_read_tokens IS NOT NULL
                         OR t.output_tokens IS NOT NULL)) AS measured,
                SUM(COALESCE(t.input_tokens, 0)
                    + COALESCE(t.cache_creation_tokens, 0)
                    + COALESCE(t.cache_read_tokens, 0)
                    + COALESCE(t.output_tokens, 0)) AS traffic,
                SUM(COALESCE(t.cache_read_tokens, 0)) AS cache_read,
                -- Any measured session missing cache reads makes `billable`
                -- underivable for the whole object.
                SUM(t.cli_session_pk IS NOT NULL
                    AND t.cache_read_tokens IS NULL
                    AND (t.input_tokens IS NOT NULL
                         OR t.output_tokens IS NOT NULL)) AS measured_without_cache
         FROM discussion_sessions s
         JOIN planning_task_discussions ptd ON ptd.discussion_id = s.disc_id
         JOIN planning_tasks pt ON pt.id = ptd.task_id
         LEFT JOIN cli_session_telemetry t ON t.cli_session_pk = s.id
         GROUP BY pt.task_number
         ORDER BY pt.task_number",
    )
}

/// Spend per project, resolved through each session's discussion.
pub fn spend_by_project(conn: &Connection) -> Result<Vec<ObjectSpend>> {
    rollup(
        conn,
        "SELECT d.project_id AS object_key,
                COUNT(*) AS sessions,
                SUM(t.cli_session_pk IS NOT NULL
                    AND (t.input_tokens IS NOT NULL
                         OR t.cache_creation_tokens IS NOT NULL
                         OR t.cache_read_tokens IS NOT NULL
                         OR t.output_tokens IS NOT NULL)) AS measured,
                SUM(COALESCE(t.input_tokens, 0)
                    + COALESCE(t.cache_creation_tokens, 0)
                    + COALESCE(t.cache_read_tokens, 0)
                    + COALESCE(t.output_tokens, 0)) AS traffic,
                SUM(COALESCE(t.cache_read_tokens, 0)) AS cache_read,
                SUM(t.cli_session_pk IS NOT NULL
                    AND t.cache_read_tokens IS NULL
                    AND (t.input_tokens IS NOT NULL
                         OR t.output_tokens IS NOT NULL)) AS measured_without_cache
         FROM discussion_sessions s
         JOIN discussions d ON d.id = s.disc_id
         LEFT JOIN cli_session_telemetry t ON t.cli_session_pk = s.id
         WHERE d.project_id IS NOT NULL
         GROUP BY d.project_id
         ORDER BY d.project_id",
    )
}

/// What one discussion cost — KT-254.
///
/// TWO figures, deliberately never one. The agents Kronn spawns report a cost PER
/// REPLY; a joined CLI reports a RUNNING TOTAL for its whole session, which also
/// covers file reads, test runs and work in other rooms. Adding them would both
/// double-count the CLI's own messages and charge this discussion for work done
/// elsewhere, and the sum would carry no unit anyone could name.
///
/// So the header shows them side by side, each with what it covers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DiscussionTokenCost {
    pub disc_id: String,
    /// Summed `tokens_used` over replies from agents Kronn spawned. Exact for what
    /// it covers, and 0 genuinely means no in-app agent replied.
    pub in_app_tokens: i64,
    pub in_app_messages: i64,
    /// The CLI side: traffic the vendors reported for the sessions joined here.
    /// `None` when nothing was measured — never 0, because an unmeasured session
    /// is unknown, not free.
    pub cli_traffic_tokens: Option<i64>,
    /// Traffic minus cache reads. `None` unless every measured session reports
    /// cache reads; see `ObjectSpend::billable_tokens` for why mixing vendors
    /// makes this underivable rather than approximable.
    pub cli_billable_tokens: Option<i64>,
    pub cli_sessions: i64,
    pub cli_sessions_measured: i64,
    /// Sessions with no readable counter. A reader needs this number to judge the
    /// figure beside it.
    pub cli_sessions_unmeasured: i64,
}

impl DiscussionTokenCost {
    /// Whether the CLI side can be shown as a figure at all.
    pub fn cli_is_known(&self) -> bool {
        self.cli_traffic_tokens.is_some()
    }
}

/// Cost of one discussion, as two separate figures.
pub fn cost_for_discussion(conn: &Connection, disc_id: &str) -> Result<DiscussionTokenCost> {
    let (in_app_tokens, in_app_messages) = conn.query_row(
        // Only replies that carry a per-reply cost. A CLI's messages sit at 0 here
        // by construction, so counting them would report a message count with no
        // cost behind it.
        "SELECT COALESCE(SUM(tokens_used), 0), COUNT(*)
           FROM messages
          WHERE discussion_id = ?1 AND tokens_used > 0",
        [disc_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;

    let (sessions, measured, traffic, cache_read, missing_cache) = conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(t.cli_session_pk IS NOT NULL
                    AND (t.input_tokens IS NOT NULL
                         OR t.cache_creation_tokens IS NOT NULL
                         OR t.cache_read_tokens IS NOT NULL
                         OR t.output_tokens IS NOT NULL)), 0),
                COALESCE(SUM(COALESCE(t.input_tokens, 0)
                    + COALESCE(t.cache_creation_tokens, 0)
                    + COALESCE(t.cache_read_tokens, 0)
                    + COALESCE(t.output_tokens, 0)), 0),
                COALESCE(SUM(COALESCE(t.cache_read_tokens, 0)), 0),
                COALESCE(SUM(t.cli_session_pk IS NOT NULL
                    AND t.cache_read_tokens IS NULL
                    AND (t.input_tokens IS NOT NULL
                         OR t.output_tokens IS NOT NULL)), 0)
           FROM discussion_sessions s
           LEFT JOIN cli_session_telemetry t ON t.cli_session_pk = s.id
          WHERE s.disc_id = ?1",
        [disc_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        },
    )?;

    Ok(DiscussionTokenCost {
        disc_id: disc_id.to_string(),
        in_app_tokens,
        in_app_messages,
        cli_traffic_tokens: (measured > 0).then_some(traffic),
        cli_billable_tokens: (measured > 0 && missing_cache == 0).then_some(traffic - cache_read),
        cli_sessions: sessions,
        cli_sessions_measured: measured,
        cli_sessions_unmeasured: sessions - measured,
    })
}

fn rollup(conn: &Connection, sql: &str) -> Result<Vec<ObjectSpend>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement
        .query_map([], |row| {
            let sessions: i64 = row.get(1)?;
            let measured: i64 = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
            let traffic: i64 = row.get::<_, Option<i64>>(3)?.unwrap_or(0);
            let cache_read: i64 = row.get::<_, Option<i64>>(4)?.unwrap_or(0);
            let missing_cache: i64 = row.get::<_, Option<i64>>(5)?.unwrap_or(0);
            Ok(ObjectSpend {
                object_key: row.get(0)?,
                sessions,
                measured_sessions: measured,
                unmeasured_sessions: sessions - measured,
                // Zero measured sessions means unknown, not free.
                traffic_tokens: (measured > 0).then_some(traffic),
                billable_tokens: (measured > 0 && missing_cache == 0)
                    .then_some(traffic - cache_read),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
#[path = "cli_telemetry_test.rs"]
mod cli_telemetry_test;

/// Assess one CLI session against the shipped budget — KT-193.
///
/// Reads the three axes from what Kronn actually knows: traffic from the
/// telemetry row (`None` when the vendor has no collector, which yields
/// `Unknown` rather than a comfortable `Ok`), age from `joined_at`, turns from
/// the messages this exact session authored.
pub fn assess_session(
    conn: &Connection,
    cli_session_pk: i64,
    budget: &crate::core::session_budget::SessionBudget,
) -> Result<crate::core::session_budget::BudgetAssessment> {
    let traffic = get(conn, cli_session_pk)?.and_then(|row| row.traffic_tokens());

    let joined: Option<String> = conn
        .query_row(
            "SELECT joined_at FROM discussion_sessions WHERE id = ?1",
            params![cli_session_pk],
            |row| row.get(0),
        )
        .optional()?;
    let age_hours = joined
        .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(&stamp).ok())
        .map(|when| (Utc::now() - when.with_timezone(&Utc)).num_hours());

    // Turns this session POSTED, via the durable author link — not messages in
    // the room, which would count everyone else's.
    let turns: i64 = conn.query_row(
        "SELECT COUNT(*) FROM message_cli_authors WHERE cli_session_id = ?1",
        params![cli_session_pk],
        |row| row.get(0),
    )?;

    Ok(crate::core::session_budget::assess(
        budget,
        traffic,
        age_hours,
        Some(turns),
    ))
}

/// Rotation metrics — KT-193 DoD 6.
///
/// "Show the gain and any loss of quality or continuity." Both halves matter: a
/// figure that only showed the saving would be advertising, and the whole release
/// rests on numbers being checkable.
///
/// The gain is what rotating buys: traffic per turn falls when a session stops
/// carrying a week of history. The loss is what it costs: a fresh session may
/// re-ask something the old one knew.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RotationMetrics {
    pub agent_type: String,
    /// Sessions with at least one real counter. The rest are excluded from the
    /// averages entirely rather than counted as cheap.
    pub measured_sessions: i64,
    /// Sessions with no readable counter. Named so an average over 3 of 40
    /// sessions cannot pass for a fact about all 40.
    pub unmeasured_sessions: i64,
    /// Median traffic per posted turn, across measured sessions. Median and not
    /// mean: one 4-billion-token session would drag a mean anywhere.
    pub median_traffic_per_turn: Option<i64>,
    /// The heaviest measured session — the case a cap is meant to catch.
    pub worst_traffic: Option<i64>,
    /// Turns posted by sessions that produced no measurable traffic. A CONTINUITY
    /// signal, not a saving: a session that spoke without spending is usually one
    /// whose telemetry is missing, and reading it as efficiency would be exactly
    /// the wrong conclusion.
    pub turns_without_traffic: i64,
}

/// Per-agent rotation metrics, measured sessions only.
pub fn rotation_metrics(conn: &Connection) -> Result<Vec<RotationMetrics>> {
    let mut statement = conn.prepare(
        "SELECT s.agent_type,
                t.cli_session_pk IS NOT NULL
                  AND (t.input_tokens IS NOT NULL
                       OR t.cache_creation_tokens IS NOT NULL
                       OR t.cache_read_tokens IS NOT NULL
                       OR t.output_tokens IS NOT NULL) AS measured,
                COALESCE(t.input_tokens, 0) + COALESCE(t.cache_creation_tokens, 0)
                  + COALESCE(t.cache_read_tokens, 0) + COALESCE(t.output_tokens, 0)
                  AS traffic,
                (SELECT COUNT(*) FROM message_cli_authors mca
                  WHERE mca.cli_session_id = s.id) AS turns
         FROM discussion_sessions s
         LEFT JOIN cli_session_telemetry t ON t.cli_session_pk = s.id
         ORDER BY s.agent_type",
    )?;
    let rows: Vec<(String, bool, i64, i64)> = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut by_agent: std::collections::BTreeMap<String, Vec<(bool, i64, i64)>> =
        std::collections::BTreeMap::new();
    for (agent, measured, traffic, turns) in rows {
        by_agent
            .entry(agent)
            .or_default()
            .push((measured, traffic, turns));
    }

    let mut out = Vec::new();
    for (agent_type, sessions) in by_agent {
        let measured: Vec<&(bool, i64, i64)> = sessions
            .iter()
            .filter(|(measured, _, _)| *measured)
            .collect();
        let unmeasured = sessions.len() as i64 - measured.len() as i64;

        // Only sessions that actually POSTED can yield a per-turn figure;
        // dividing by zero turns would invent an enormous ratio.
        let mut per_turn: Vec<i64> = measured
            .iter()
            .filter(|(_, _, turns)| *turns > 0)
            .map(|(_, traffic, turns)| traffic / turns)
            .collect();
        per_turn.sort_unstable();
        let median = (!per_turn.is_empty()).then(|| per_turn[per_turn.len() / 2]);

        let worst = measured.iter().map(|(_, traffic, _)| *traffic).max();

        let turns_without_traffic = sessions
            .iter()
            .filter(|(measured, traffic, turns)| *turns > 0 && (!*measured || *traffic == 0))
            .map(|(_, _, turns)| *turns)
            .sum();

        out.push(RotationMetrics {
            agent_type,
            measured_sessions: measured.len() as i64,
            unmeasured_sessions: unmeasured,
            median_traffic_per_turn: median,
            worst_traffic: worst,
            turns_without_traffic,
        });
    }
    Ok(out)
}
