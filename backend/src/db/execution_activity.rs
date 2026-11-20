//! Silent-worker clocks and room-ownership lookups of task executions, kept below
//! `db::orchestration` so the dispatch and discussion writers do not depend on it.

use std::str::FromStr;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::TaskExecutionStatus;

pub fn parse_dt(value: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&value)
        .map(|date| date.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// KT-1247 — a silent worker must be detectable even when the campaign set no
/// resilience policy (every campaign so far). Twenty minutes without a tool
/// call, a message, a commit or a delivery is the default; both defaults can be
/// moved globally through the environment, and per campaign through the
/// `orchestration_run_resilience_policy` row, which always wins.
pub const DEFAULT_ACTIVITY_TIMEOUT_SECS: u32 = 20 * 60;
/// How long a delivery may sit unreviewed before the principal is alerted.
pub const DEFAULT_REVIEW_TIMEOUT_SECS: u32 = 4 * 60 * 60;
const ACTIVITY_TIMEOUT_ENV: &str = "KRONN_TASK_ACTIVITY_TIMEOUT_SECS";
const REVIEW_TIMEOUT_ENV: &str = "KRONN_TASK_REVIEW_TIMEOUT_SECS";

pub fn timeout_secs_from_env(name: &str, fallback: u32) -> u32 {
    crate::core::child_env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(fallback)
}

/// Global default activity timeout (`KRONN_TASK_ACTIVITY_TIMEOUT_SECS`).
pub fn default_activity_timeout_secs() -> u32 {
    timeout_secs_from_env(ACTIVITY_TIMEOUT_ENV, DEFAULT_ACTIVITY_TIMEOUT_SECS)
}

/// Global default review timeout (`KRONN_TASK_REVIEW_TIMEOUT_SECS`).
pub fn default_review_timeout_secs() -> u32 {
    timeout_secs_from_env(REVIEW_TIMEOUT_ENV, DEFAULT_REVIEW_TIMEOUT_SECS)
}

/// The activity clock a status owes. A worker owes activity only while it is
/// `Working`; the default therefore arms nowhere else, so an `Interrupted`,
/// `AwaitingReview` or `Escalated` row is never "silent". An explicit campaign
/// policy keeps the clock it always armed.
pub fn effective_activity_secs(policy: Option<i64>, status: TaskExecutionStatus) -> Option<i64> {
    policy.or_else(|| {
        (status == TaskExecutionStatus::Working).then(|| i64::from(default_activity_timeout_secs()))
    })
}

pub fn effective_review_secs(policy: Option<i64>) -> i64 {
    policy.unwrap_or_else(|| i64::from(default_review_timeout_secs()))
}

pub fn touch_execution_activity(conn: &Connection, exec_id: &str) -> Result<()> {
    let now = Utc::now();
    type RecoveryClocks = (
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        String,
        String,
    );
    let clocks: Option<RecoveryClocks> = conn
        .query_row(
            "SELECT p.activity_timeout_secs, p.review_timeout_secs, \
                    p.human_wait_timeout_secs, r.timeout_secs, e.created_at, e.status \
             FROM task_executions e \
             JOIN orchestration_runs r ON r.id = e.orchestration_run_id \
             LEFT JOIN orchestration_run_resilience_policy p \
               ON p.orchestration_run_id = e.orchestration_run_id \
             WHERE e.id = ?1",
            [exec_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;
    let Some((activity_secs, review_secs, _human_secs, total_secs, created_at, status)) = clocks
    else {
        return Ok(());
    };
    let current_status = TaskExecutionStatus::from_str(&status)?;
    let deadline = effective_activity_secs(activity_secs, current_status)
        .map(|seconds| now + chrono::Duration::seconds(seconds));
    let review_deadline = if current_status == TaskExecutionStatus::AwaitingReview {
        Some(now + chrono::Duration::seconds(effective_review_secs(review_secs)))
    } else {
        None
    };
    let human_started = if status == TaskExecutionStatus::Escalated.as_str() {
        Some(now)
    } else {
        None
    };
    let total_deadline =
        total_secs.map(|seconds| parse_dt(created_at.clone()) + chrono::Duration::seconds(seconds));
    conn.execute(
        "INSERT OR IGNORE INTO task_execution_recovery (\
             task_execution_id, recovery_action, recovery_reason, last_activity_at, \
             total_deadline_at, activity_deadline_at, review_deadline_at, \
             human_wait_started_at, assignment_generation, pending, updated_at\
         ) VALUES (?1, 'resume_worker', 'runtime clocks', ?2, ?3, ?4, ?5, ?6, 0, 0, ?2)",
        params![
            exec_id,
            now.to_rfc3339(),
            total_deadline.map(|value| value.to_rfc3339()),
            deadline.map(|value| value.to_rfc3339()),
            review_deadline.map(|value| value.to_rfc3339()),
            human_started.map(|value| value.to_rfc3339()),
        ],
    )?;
    conn.execute(
        "UPDATE task_execution_recovery SET last_activity_at = ?2, \
                activity_deadline_at = ?3, review_deadline_at = ?4, \
                human_wait_started_at = ?5, updated_at = ?2 \
         WHERE task_execution_id = ?1",
        params![
            exec_id,
            now.to_rfc3339(),
            deadline.map(|value| value.to_rfc3339()),
            review_deadline.map(|value| value.to_rfc3339()),
            human_started.map(|value| value.to_rfc3339()),
        ],
    )?;
    Ok(())
}

/// Push the silent-worker clock of the `Working` execution owning this dispatch.
/// Best effort by design: failing to move a clock must never fail the progress
/// write or the message that carries the activity.
pub fn note_dispatch_activity(conn: &Connection, dispatch_id: &str) -> Result<()> {
    let exec_id: Option<String> = conn
        .query_row(
            "SELECT id FROM task_executions \
             WHERE dispatch_job_id = ?1 AND status = 'Working' LIMIT 1",
            [dispatch_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(exec_id) = exec_id {
        if let Err(error) = touch_execution_activity(conn, &exec_id) {
            tracing::warn!(execution_id = %exec_id, "could not push activity clock: {error}");
        }
    }
    Ok(())
}

/// Same, keyed by the worker room a message was posted in.
pub fn note_room_activity(conn: &Connection, discussion_id: &str) -> Result<()> {
    let exec_id: Option<String> = conn
        .query_row(
            "SELECT id FROM task_executions \
             WHERE sub_discussion_id = ?1 AND status = 'Working' LIMIT 1",
            [discussion_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(exec_id) = exec_id {
        if let Err(error) = touch_execution_activity(conn, &exec_id) {
            tracing::warn!(execution_id = %exec_id, "could not push activity clock: {error}");
        }
    }
    Ok(())
}

/// A worker's durable assignment owns its room even when its CLI is absent,
/// left, or not polling. Only an explicit execution reassignment changes this
/// authority; presence must never create a second writer in the worktree.
pub fn discussion_has_cli_worker(conn: &Connection, discussion_id: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_executions
          WHERE sub_discussion_id = ?1 AND worker_target_kind = 'cli')",
        [discussion_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

/// KT-1291 — the sentence the execution was launched with for DoD item
/// `dod_id`; `None` for an execution launched before the snapshot recorded
/// sentences.
pub fn execution_dod_sentence(
    conn: &Connection,
    exec_id: &str,
    dod_id: &str,
) -> Result<Option<String>> {
    let payloads = conn
        .prepare(
            "SELECT changes_json FROM task_execution_events
              WHERE task_execution_id = ?1 AND action = 'created'
              ORDER BY created_at ASC, rowid ASC",
        )?
        .query_map(params![exec_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for payload in payloads {
        let Ok(changes) = serde_json::from_str::<serde_json::Value>(&payload) else {
            continue;
        };
        let found = changes
            .get("worker_dod")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .find(|item| item.get("id").and_then(serde_json::Value::as_str) == Some(dod_id))
            .and_then(|item| item.get("sentence").and_then(serde_json::Value::as_str));
        if let Some(sentence) = found {
            return Ok(Some(sentence.to_string()));
        }
    }
    Ok(None)
}
