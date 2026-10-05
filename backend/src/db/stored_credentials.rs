//! Encrypted credential rows (provider keys and the API auth token).
//!
//! This layer only moves ciphertext; encryption, read-back checks and the
//! config.toml migration live in `core::credential_store`.

use std::collections::HashSet;

use anyhow::Result;
use rusqlite::{params, Connection};

pub const KIND_PROVIDER_KEY: &str = "provider_key";
pub const KIND_AUTH_TOKEN: &str = "auth_token";
/// The auth token is a singleton row under this id.
pub const AUTH_TOKEN_ID: &str = "server";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCredential {
    pub kind: String,
    pub id: String,
    pub name: String,
    pub provider: String,
    pub active: bool,
    pub position: i64,
    pub value_encrypted: String,
}

impl StoredCredential {
    pub fn row_key(&self) -> (String, String) {
        (self.kind.clone(), self.id.clone())
    }
}

pub fn list(conn: &Connection) -> Result<Vec<StoredCredential>> {
    let mut stmt = conn.prepare(
        "SELECT kind, id, name, provider, active, position, value_encrypted
         FROM stored_credentials ORDER BY kind, position, id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(StoredCredential {
                kind: row.get(0)?,
                id: row.get(1)?,
                name: row.get(2)?,
                provider: row.get(3)?,
                active: row.get::<_, i64>(4)? != 0,
                position: row.get(5)?,
                value_encrypted: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn upsert(conn: &Connection, row: &StoredCredential) -> Result<()> {
    conn.execute(
        "INSERT INTO stored_credentials
            (kind, id, name, provider, active, position, value_encrypted, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
         ON CONFLICT(kind, id) DO UPDATE SET
            name = excluded.name,
            provider = excluded.provider,
            active = excluded.active,
            position = excluded.position,
            value_encrypted = excluded.value_encrypted,
            updated_at = excluded.updated_at",
        params![
            row.kind,
            row.id,
            row.name,
            row.provider,
            row.active as i64,
            row.position,
            row.value_encrypted
        ],
    )?;
    Ok(())
}

/// Insert or update `rows` without deleting anything (migration merge).
pub fn upsert_all(conn: &Connection, rows: &[StoredCredential]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for row in rows {
        upsert(&tx, row)?;
    }
    tx.commit()?;
    Ok(())
}

/// Make the table hold exactly `rows`, except the `preserve` keys, which are
/// never touched: rows the current key cannot decrypt must not be deleted.
pub fn replace_all(
    conn: &Connection,
    rows: &[StoredCredential],
    preserve: &HashSet<(String, String)>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let wanted: HashSet<(String, String)> = rows.iter().map(StoredCredential::row_key).collect();
    for existing in list(&tx)? {
        let key = existing.row_key();
        if !wanted.contains(&key) && !preserve.contains(&key) {
            tx.execute(
                "DELETE FROM stored_credentials WHERE kind = ?1 AND id = ?2",
                params![key.0, key.1],
            )?;
        }
    }
    for row in rows {
        if !preserve.contains(&row.row_key()) {
            upsert(&tx, row)?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn delete_all(conn: &Connection) -> Result<usize> {
    Ok(conn.execute("DELETE FROM stored_credentials", [])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn row(kind: &str, id: &str, enc: &str, position: i64) -> StoredCredential {
        StoredCredential {
            kind: kind.into(),
            id: id.into(),
            name: format!("name-{id}"),
            provider: "anthropic".into(),
            active: true,
            position,
            value_encrypted: enc.into(),
        }
    }

    #[tokio::test]
    async fn upsert_then_replace_keeps_preserved_rows_and_order() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            upsert_all(
                conn,
                &[
                    row(KIND_PROVIDER_KEY, "b", "enc-b", 1),
                    row(KIND_PROVIDER_KEY, "a", "enc-a", 0),
                    row(KIND_PROVIDER_KEY, "locked", "enc-old-key", 2),
                ],
            )?;
            let listed = list(conn)?;
            assert_eq!(
                listed.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
                vec!["a", "b", "locked"]
            );

            let mut preserve = HashSet::new();
            preserve.insert((KIND_PROVIDER_KEY.to_string(), "locked".to_string()));
            replace_all(
                conn,
                &[
                    row(KIND_PROVIDER_KEY, "a", "enc-a2", 0),
                    row(KIND_AUTH_TOKEN, AUTH_TOKEN_ID, "enc-tok", 0),
                ],
                &preserve,
            )?;
            let listed = list(conn)?;
            let ids: Vec<_> = listed
                .iter()
                .map(|r| (r.kind.as_str(), r.id.as_str()))
                .collect();
            assert_eq!(
                ids,
                vec![
                    (KIND_AUTH_TOKEN, AUTH_TOKEN_ID),
                    (KIND_PROVIDER_KEY, "a"),
                    (KIND_PROVIDER_KEY, "locked")
                ]
            );
            assert_eq!(listed[1].value_encrypted, "enc-a2");
            assert_eq!(listed[2].value_encrypted, "enc-old-key");

            assert_eq!(delete_all(conn)?, 3);
            assert!(list(conn)?.is_empty());
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn kind_is_constrained() {
        let db = Database::open_in_memory().unwrap();
        let err = db
            .with_conn(|conn| upsert_all(conn, &[row("other", "x", "e", 0)]))
            .await;
        assert!(err.is_err(), "unknown kinds must be refused by the schema");
    }
}
