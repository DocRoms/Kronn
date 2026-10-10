//! Per-workflow state of a `Watch` trigger (KT-1099): the source's last
//! validators and fingerprint, and the poll counters shown on the card.

use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::models::{WatchPollResult, WatchStatus};

/// Consecutive failed polls from which the workflow shows as failing.
pub const WATCH_FAILURE_THRESHOLD: u32 = 3;

/// What the next poll compares against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchBaseline {
    pub source_key: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub fingerprint: Option<String>,
    /// Advances with the baseline (see `workflow_watch_occurrences`).
    pub seq: i64,
}

/// A detected change for one project: the baseline it departs from and the
/// state it reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchOccurrence {
    pub baseline_seq: i64,
    pub state_key: String,
    /// The served project's id, empty for a projectless run.
    pub project_key: String,
}

/// One poll's effect on the stored state.
#[derive(Debug, Clone)]
pub struct WatchPollRecord {
    pub at: DateTime<Utc>,
    pub result: WatchPollResult,
    pub http_status: Option<u16>,
    pub error: Option<String>,
    /// The baseline to keep; `None` leaves the stored one as it is.
    pub baseline: Option<WatchBaseline>,
}

fn result_name(result: WatchPollResult) -> &'static str {
    match result {
        WatchPollResult::Baseline => "baseline",
        WatchPollResult::Unchanged => "unchanged",
        WatchPollResult::Changed => "changed",
        WatchPollResult::Deferred => "deferred",
        WatchPollResult::Error => "error",
    }
}

fn parse_result(name: &str) -> Option<WatchPollResult> {
    Some(match name {
        "baseline" => WatchPollResult::Baseline,
        "unchanged" => WatchPollResult::Unchanged,
        "changed" => WatchPollResult::Changed,
        "deferred" => WatchPollResult::Deferred,
        "error" => WatchPollResult::Error,
        _ => return None,
    })
}

fn parse_time(value: Option<String>) -> Option<DateTime<Utc>> {
    value
        .and_then(|v| DateTime::parse_from_rfc3339(&v).ok())
        .map(|v| v.with_timezone(&Utc))
}

pub fn get_baseline(conn: &Connection, workflow_id: &str) -> Result<Option<WatchBaseline>> {
    Ok(conn
        .query_row(
            "SELECT source_key, etag, last_modified, fingerprint, baseline_seq
               FROM workflow_watch_state WHERE workflow_id = ?1",
            params![workflow_id],
            |row| {
                Ok(WatchBaseline {
                    source_key: row.get(0)?,
                    etag: row.get(1)?,
                    last_modified: row.get(2)?,
                    fingerprint: row.get(3)?,
                    seq: row.get(4)?,
                })
            },
        )
        .optional()?)
}

/// Whether `occurrence` already has a run.
pub fn is_admitted(
    conn: &Connection,
    workflow_id: &str,
    occurrence: &WatchOccurrence,
) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM workflow_watch_occurrences
              WHERE workflow_id = ?1 AND baseline_seq = ?2 AND state_key = ?3 AND project_key = ?4",
            params![
                workflow_id,
                occurrence.baseline_seq,
                occurrence.state_key,
                occurrence.project_key
            ],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Records `run_id` as the run of `occurrence`; the caller's transaction also
/// inserts that run.
pub fn mark_admitted(
    conn: &Connection,
    workflow_id: &str,
    occurrence: &WatchOccurrence,
    run_id: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO workflow_watch_occurrences
             (workflow_id, baseline_seq, state_key, project_key, run_id, admitted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            workflow_id,
            occurrence.baseline_seq,
            occurrence.state_key,
            occurrence.project_key,
            run_id,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

/// Applies one poll: counters, last result and, when given, the new baseline.
/// A Baseline or Changed poll advances the sequence and drops the occurrences
/// of the baseline it left.
pub fn record_poll(conn: &Connection, workflow_id: &str, poll: &WatchPollRecord) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    record_poll_in(&tx, workflow_id, poll)?;
    tx.commit()?;
    Ok(())
}

fn record_poll_in(conn: &Connection, workflow_id: &str, poll: &WatchPollRecord) -> Result<()> {
    let advance = poll.baseline.is_some()
        && matches!(
            poll.result,
            WatchPollResult::Baseline | WatchPollResult::Changed
        );
    let failed = poll.result == WatchPollResult::Error;
    let (unchanged, changed, errors) = match poll.result {
        WatchPollResult::Unchanged => (1, 0, 0),
        WatchPollResult::Changed => (0, 1, 0),
        WatchPollResult::Error => (0, 0, 1),
        WatchPollResult::Baseline | WatchPollResult::Deferred => (0, 0, 0),
    };
    let at = poll.at.to_rfc3339();
    let change_at = (poll.result == WatchPollResult::Changed).then(|| at.clone());
    let baseline = poll.baseline.as_ref();
    conn.execute(
        "INSERT INTO workflow_watch_state (
             workflow_id, source_key, etag, last_modified, fingerprint,
             last_poll_at, last_result, last_http_status, last_error, last_change_at,
             unchanged_count, changed_count, error_count, consecutive_failures)
         VALUES (?1, COALESCE(?2, ''), ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(workflow_id) DO UPDATE SET
             source_key = CASE WHEN ?15 THEN COALESCE(?2, '') ELSE source_key END,
             etag = CASE WHEN ?15 THEN ?3 ELSE etag END,
             last_modified = CASE WHEN ?15 THEN ?4 ELSE last_modified END,
             fingerprint = CASE WHEN ?15 THEN ?5 ELSE fingerprint END,
             last_poll_at = ?6,
             last_result = ?7,
             last_http_status = ?8,
             last_error = ?9,
             last_change_at = COALESCE(?10, last_change_at),
             unchanged_count = unchanged_count + ?11,
             changed_count = changed_count + ?12,
             error_count = error_count + ?13,
             consecutive_failures = CASE WHEN ?14 = 1 THEN consecutive_failures + 1 ELSE 0 END,
             baseline_seq = baseline_seq + ?16",
        params![
            workflow_id,
            baseline.map(|b| b.source_key.as_str()),
            baseline.and_then(|b| b.etag.as_deref()),
            baseline.and_then(|b| b.last_modified.as_deref()),
            baseline.and_then(|b| b.fingerprint.as_deref()),
            at,
            result_name(poll.result),
            poll.http_status,
            poll.error.as_deref(),
            change_at,
            unchanged,
            changed,
            errors,
            i64::from(failed),
            baseline.is_some(),
            i64::from(advance),
        ],
    )?;
    if advance {
        conn.execute(
            "DELETE FROM workflow_watch_occurrences
              WHERE workflow_id = ?1
                AND baseline_seq < (SELECT baseline_seq FROM workflow_watch_state
                                     WHERE workflow_id = ?1)",
            params![workflow_id],
        )?;
    }
    Ok(())
}

fn status_from_row(row: &Row<'_>) -> rusqlite::Result<(String, WatchStatus)> {
    let consecutive_failures: u32 = row.get(9)?;
    Ok((
        row.get(0)?,
        WatchStatus {
            last_poll_at: parse_time(row.get(1)?),
            last_result: row
                .get::<_, Option<String>>(2)?
                .as_deref()
                .and_then(parse_result),
            last_http_status: row.get(3)?,
            last_error: row.get(4)?,
            last_change_at: parse_time(row.get(5)?),
            unchanged_count: row.get::<_, i64>(6)?.max(0) as u64,
            changed_count: row.get::<_, i64>(7)?.max(0) as u64,
            error_count: row.get::<_, i64>(8)?.max(0) as u64,
            consecutive_failures,
            failing: consecutive_failures >= WATCH_FAILURE_THRESHOLD,
        },
    ))
}

const STATUS_COLUMNS: &str = "workflow_id, last_poll_at, last_result, last_http_status, \
     last_error, last_change_at, unchanged_count, changed_count, error_count, \
     consecutive_failures";

/// Every workflow's poll status, for the workflow list.
pub fn list_statuses(conn: &Connection) -> Result<HashMap<String, WatchStatus>> {
    let sql = format!("SELECT {STATUS_COLUMNS} FROM workflow_watch_state");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], status_from_row)?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

pub fn get_status(conn: &Connection, workflow_id: &str) -> Result<Option<WatchStatus>> {
    let sql = format!("SELECT {STATUS_COLUMNS} FROM workflow_watch_state WHERE workflow_id = ?1");
    Ok(conn
        .query_row(&sql, params![workflow_id], status_from_row)
        .optional()?
        .map(|(_, status)| status))
}
