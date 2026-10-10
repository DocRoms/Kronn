//! KT-1030 — the marker of content Kronn ships by default. A row means Kronn
//! handled that content once; it is never installed again on its own, even
//! after the user deletes it.

use anyhow::{bail, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultContentStatus {
    Installed,
    /// The user already had their own version: Kronn left it alone.
    KeptExisting,
}

impl DefaultContentStatus {
    fn as_db_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::KeptExisting => "kept_existing",
        }
    }

    fn from_db_str(raw: &str) -> Result<Self> {
        Ok(match raw {
            "installed" => Self::Installed,
            "kept_existing" => Self::KeptExisting,
            other => bail!("Unknown default content status: {other}"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultContentRecord {
    pub key: String,
    pub status: DefaultContentStatus,
    pub page_id: Option<String>,
    pub workflow_ids: Vec<String>,
    pub updated_at: String,
}

pub fn get(conn: &Connection, key: &str) -> Result<Option<DefaultContentRecord>> {
    let row: Option<(String, Option<String>, String, String)> = conn
        .query_row(
            "SELECT status, page_id, workflow_ids, updated_at FROM default_contents WHERE key = ?1",
            [key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    row.map(|(status, page_id, workflow_ids, updated_at)| {
        Ok(DefaultContentRecord {
            key: key.to_string(),
            status: DefaultContentStatus::from_db_str(&status)?,
            page_id,
            workflow_ids: serde_json::from_str(&workflow_ids).unwrap_or_default(),
            updated_at,
        })
    })
    .transpose()
}

pub fn put(
    conn: &Connection,
    key: &str,
    status: DefaultContentStatus,
    page_id: Option<&str>,
    workflow_ids: &[String],
) -> Result<()> {
    conn.execute(
        "INSERT INTO default_contents (key, status, page_id, workflow_ids, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(key) DO UPDATE SET status = excluded.status, page_id = excluded.page_id,
             workflow_ids = excluded.workflow_ids, updated_at = excluded.updated_at",
        params![
            key,
            status.as_db_str(),
            page_id,
            serde_json::to_string(workflow_ids)?,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_round_trips_and_is_replaced_in_place() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        assert!(get(&conn, "todo").unwrap().is_none());
        put(&conn, "todo", DefaultContentStatus::KeptExisting, None, &[]).unwrap();
        put(
            &conn,
            "todo",
            DefaultContentStatus::Installed,
            Some("page-1"),
            &["wf-1".into()],
        )
        .unwrap();
        let record = get(&conn, "todo").unwrap().unwrap();
        assert_eq!(record.status, DefaultContentStatus::Installed);
        assert_eq!(record.page_id.as_deref(), Some("page-1"));
        assert_eq!(record.workflow_ids, vec!["wf-1".to_string()]);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM default_contents", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 1);
    }
}
