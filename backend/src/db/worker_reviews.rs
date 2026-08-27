//! KT-319 tranche 3 — principal review persistence (`task_execution_reviews`, 128).
//!
//! Attempt-scoped, upserted idempotently on `(execution, attempt)`: a re-decide or a
//! crash replay refreshes the one row, never duplicates it (DoD-8). Actor identity
//! ("who decided") is NOT stored here — it lives in `task_execution_events` (128
//! header), so these rows carry no `discussion_sessions` FK and no CASCADE/RESTRICT
//! deletion trap. Mirrors `worker_deliveries` (its delivery twin).

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::TaskExecutionReview;

fn row_to_review(row: &rusqlite::Row) -> rusqlite::Result<TaskExecutionReview> {
    Ok(TaskExecutionReview {
        id: row.get(0)?,
        task_execution_id: row.get(1)?,
        attempt_no: row.get::<_, i64>(2)? as u32,
        decision: row.get(3)?,
        decision_json: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

/// Persist (upsert) the principal's validated ReviewDecision for `(exec, attempt)`. The id
/// is deterministic and `ON CONFLICT(exec, attempt)` refreshes the one row in place, so an
/// idempotent re-decide / crash replay never duplicates (DoD-8). MUST run inside the
/// caller's review-checkpoint transaction.
pub fn upsert_review(
    conn: &Connection,
    exec_id: &str,
    attempt_no: u32,
    decision: &str,
    decision_json: &str,
) -> Result<String> {
    let id = format!("review:{exec_id}:{attempt_no}");
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO task_execution_reviews \
         (id, task_execution_id, attempt_no, decision, decision_json, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6) \
         ON CONFLICT(task_execution_id, attempt_no) DO UPDATE SET \
             decision = excluded.decision, \
             decision_json = excluded.decision_json, \
             updated_at = excluded.updated_at",
        params![id, exec_id, attempt_no as i64, decision, decision_json, now],
    )?;
    Ok(id)
}

/// The persisted review for `(exec, attempt)`, if any. Used by the checkpoint's idempotency
/// short-circuit (a resume that re-decides the same attempt is a no-op) and for audit.
pub fn get_review(
    conn: &Connection,
    exec_id: &str,
    attempt_no: u32,
) -> Result<Option<TaskExecutionReview>> {
    let row = conn
        .query_row(
            "SELECT id, task_execution_id, attempt_no, decision, decision_json, \
                    created_at, updated_at \
             FROM task_execution_reviews \
             WHERE task_execution_id = ?1 AND attempt_no = ?2",
            params![exec_id, attempt_no as i64],
            row_to_review,
        )
        .optional()?;
    Ok(row)
}

/// All durable principal decisions for an execution, oldest attempt first.
pub fn list_reviews(conn: &Connection, exec_id: &str) -> Result<Vec<TaskExecutionReview>> {
    let mut statement = conn.prepare(
        "SELECT id, task_execution_id, attempt_no, decision, decision_json, \
                created_at, updated_at \
           FROM task_execution_reviews \
          WHERE task_execution_id = ?1 \
          ORDER BY attempt_no ASC",
    )?;
    let rows = statement
        .query_map([exec_id], row_to_review)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
