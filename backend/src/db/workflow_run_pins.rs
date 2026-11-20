//! Rows of `workflow_run_pins` (KT-1096): what a run froze when it started.
//! Content is the resource serialised as JSON; the runtime half is
//! `workflows::run_pins`.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::models::Workflow;

/// The row holding the run's own workflow definition.
pub const RUN_KIND: &str = "run";
/// Pin kind of a Quick API.
pub const QUICK_API: &str = "quick_api";

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

#[derive(Serialize, Deserialize)]
pub struct RunHeader {
    pub pinned_at: DateTime<Utc>,
    /// `None` for a BatchQuickPrompt run, which has no workflow of its own.
    #[serde(default)]
    pub workflow: Option<Workflow>,
    /// `revision_fingerprint` at pin time; `None` when inherited.
    #[serde(default)]
    pub fingerprint: Option<String>,
    /// Zone of the run's `{{time.now}}` (KT-1103); absent (pinned before
    /// zones existed) means UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// `revision_fingerprint` of the pinned closure without repository
    /// profiles: what a human-validated workflow scope (KT-1270) is compared
    /// with. `None` when inherited or pinned before scopes existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_revision: Option<String>,
}

pub fn header(conn: &Connection, run_id: &str) -> Result<Option<RunHeader>> {
    match get(conn, run_id, RUN_KIND, "")? {
        Some(json) => Ok(Some(serde_json::from_str(&json)?)),
        None => Ok(None),
    }
}

/// The definition `run` pinned, if it has one.
pub fn pinned_workflow(conn: &Connection, run_id: &str) -> Result<Option<Workflow>> {
    Ok(header(conn, run_id)?.and_then(|header| header.workflow))
}

/// The pinned revision of a resource when the run has a pin, else the live row.
pub fn pinned_or_live<T: DeserializeOwned>(
    conn: &Connection,
    run_id: Option<&str>,
    kind: &str,
    label: &str,
    id: &str,
    live: impl Fn(&Connection, &str) -> Result<Option<T>>,
) -> Result<std::result::Result<Option<T>, String>> {
    let Some(run_id) = run_id.filter(|run_id| has_pin(conn, run_id).unwrap_or(false)) else {
        return Ok(Ok(live(conn, id)?));
    };
    let Some(json) = get(conn, run_id, kind, id)? else {
        return Ok(Err(format!(
            "{label} `{id}` is not part of the revision this run started with: launch a new run to use it."
        )));
    };
    // The pinned revision runs only while the resource still exists.
    if live(conn, id)?.is_none() {
        return Ok(Err(format!(
            "{label} `{id}` was deleted while this run was in progress: the run cannot load it."
        )));
    }
    Ok(Ok(Some(serde_json::from_str(&json)?)))
}
