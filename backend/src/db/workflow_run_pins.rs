//! Rows of `workflow_run_pins` (KT-1096): what a run froze when it started.
//! Content is the resource serialised as JSON; the runtime half is
//! `workflows::run_pins`.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// The row holding the run's own workflow definition.
pub const RUN_KIND: &str = "run";

pub fn has_pin(conn: &Connection, run_id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM workflow_run_pins WHERE run_id = ?1 AND kind = ?2",
            params![run_id, RUN_KIND],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Never overwrites: what a run pinned stays what it pinned.
pub fn insert(
    conn: &Connection,
    run_id: &str,
    kind: &str,
    resource_id: &str,
    content_json: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO workflow_run_pins (run_id, kind, resource_id, content_json)
         VALUES (?1, ?2, ?3, ?4)",
        params![run_id, kind, resource_id, content_json],
    )?;
    Ok(())
}

pub fn get(
    conn: &Connection,
    run_id: &str,
    kind: &str,
    resource_id: &str,
) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT content_json FROM workflow_run_pins
              WHERE run_id = ?1 AND kind = ?2 AND resource_id = ?3",
            params![run_id, kind, resource_id],
            |row| row.get(0),
        )
        .optional()?)
}

/// Every pinned resource of `kind` for the run, as `(id, content_json)`.
pub fn list_kind(conn: &Connection, run_id: &str, kind: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT resource_id, content_json FROM workflow_run_pins
          WHERE run_id = ?1 AND kind = ?2 ORDER BY resource_id",
    )?;
    let rows = stmt
        .query_map(params![run_id, kind], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// A child inherits its parent's dependencies, not its parent's own workflow.
pub fn copy_dependencies(conn: &Connection, from_run: &str, to_run: &str) -> Result<usize> {
    Ok(conn.execute(
        "INSERT OR IGNORE INTO workflow_run_pins (run_id, kind, resource_id, content_json)
         SELECT ?2, kind, resource_id, content_json FROM workflow_run_pins
          WHERE run_id = ?1 AND kind != ?3",
        params![from_run, to_run, RUN_KIND],
    )?)
}

pub fn purge(conn: &Connection, run_id: &str) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM workflow_run_pins WHERE run_id = ?1",
        params![run_id],
    )?)
}
