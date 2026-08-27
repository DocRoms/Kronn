//! KT-319 tranche 2 — worker delivery persistence (`task_execution_deliveries`, 128).
//!
//! Attempt-scoped, upserted idempotently on `(execution, attempt)`: a re-submit or a
//! crash replay refreshes the one row, never duplicates it (DoD-8). Actor identity
//! ("who delivered") is NOT stored here — it lives in `task_execution_events` (128
//! header), so these rows carry no `discussion_sessions` FK and no CASCADE/RESTRICT
//! deletion trap.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::TaskExecutionDelivery;

fn row_to_delivery(row: &rusqlite::Row) -> rusqlite::Result<TaskExecutionDelivery> {
    Ok(TaskExecutionDelivery {
        id: row.get(0)?,
        task_execution_id: row.get(1)?,
        attempt_no: row.get::<_, i64>(2)? as u32,
        head_sha: row.get(3)?,
        manifest_json: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

/// Persist (upsert) the worker's validated DeliveryManifest for `(exec, attempt)`. The id
/// is deterministic and `ON CONFLICT(exec, attempt)` refreshes the one row in place, so an
/// idempotent re-submit / crash replay never duplicates (DoD-8). MUST run inside the
/// caller's delivery-checkpoint transaction.
pub fn upsert_delivery(
    conn: &Connection,
    exec_id: &str,
    attempt_no: u32,
    head_sha: &str,
    manifest_json: &str,
) -> Result<String> {
    let id = format!("delivery:{exec_id}:{attempt_no}");
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO task_execution_deliveries \
         (id, task_execution_id, attempt_no, head_sha, manifest_json, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6) \
         ON CONFLICT(task_execution_id, attempt_no) DO UPDATE SET \
             head_sha = excluded.head_sha, \
             manifest_json = excluded.manifest_json, \
             updated_at = excluded.updated_at",
        params![id, exec_id, attempt_no as i64, head_sha, manifest_json, now],
    )?;
    Ok(id)
}

/// The persisted delivery for `(exec, attempt)`, if any. Used by the checkpoint's
/// idempotency short-circuit and by the review path (tranche 3) to read the delivered
/// `head_sha` for the DoD-5 drift check.
pub fn get_delivery(
    conn: &Connection,
    exec_id: &str,
    attempt_no: u32,
) -> Result<Option<TaskExecutionDelivery>> {
    let row = conn
        .query_row(
            "SELECT id, task_execution_id, attempt_no, head_sha, manifest_json, \
                    created_at, updated_at \
             FROM task_execution_deliveries \
             WHERE task_execution_id = ?1 AND attempt_no = ?2",
            params![exec_id, attempt_no as i64],
            row_to_delivery,
        )
        .optional()?;
    Ok(row)
}

/// All durable deliveries for an execution, oldest attempt first. The detail UI
/// needs the full review ping-pong rather than only the current attempt.
pub fn list_deliveries(conn: &Connection, exec_id: &str) -> Result<Vec<TaskExecutionDelivery>> {
    let mut statement = conn.prepare(
        "SELECT id, task_execution_id, attempt_no, head_sha, manifest_json, \
                created_at, updated_at \
           FROM task_execution_deliveries \
          WHERE task_execution_id = ?1 \
          ORDER BY attempt_no ASC",
    )?;
    let rows = statement
        .query_map([exec_id], row_to_delivery)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
