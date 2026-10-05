//! Server copy of the interface preferences the frontend keeps in localStorage.

use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

pub type UiPreferences = BTreeMap<String, String>;

/// An unreadable row reads as empty: preferences are a convenience, never a
/// reason to fail a page load.
pub fn get(conn: &Connection) -> Result<UiPreferences> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT values_json FROM ui_preferences WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(raw
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default())
}

pub fn put(conn: &Connection, values: &UiPreferences) -> Result<()> {
    let json = serde_json::to_string(values)?;
    conn.execute(
        "INSERT INTO ui_preferences (id, values_json, updated_at) VALUES (1, ?1, datetime('now'))
         ON CONFLICT(id) DO UPDATE SET values_json = excluded.values_json,
                                       updated_at = excluded.updated_at",
        params![json],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[tokio::test]
    async fn round_trip_replaces_the_whole_map() {
        let db = Database::open_in_memory().unwrap();
        let read = db
            .with_conn(|conn| {
                assert!(get(conn)?.is_empty());
                let mut first = UiPreferences::new();
                first.insert("kronn:theme".into(), "light".into());
                first.insert("kronn:layoutDensity".into(), "compact".into());
                put(conn, &first)?;
                let mut second = UiPreferences::new();
                second.insert("kronn:theme".into(), "thème-é".into());
                put(conn, &second)?;
                get(conn)
            })
            .await
            .unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read["kronn:theme"], "thème-é");
    }

    #[tokio::test]
    async fn a_corrupt_row_reads_as_empty() {
        let db = Database::open_in_memory().unwrap();
        let read = db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO ui_preferences (id, values_json) VALUES (1, 'not json')",
                    [],
                )?;
                get(conn)
            })
            .await
            .unwrap();
        assert!(read.is_empty());
    }
}
