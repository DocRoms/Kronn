use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::core::{crypto, execution_variables::VariableProvenance};

#[derive(Debug, serde::Serialize)]
pub struct SnapshotMetadata {
    pub id: String,
    pub resolved_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub provenance: Vec<VariableProvenance>,
    pub purged: bool,
}

pub fn snapshot_id_for_run(
    conn: &Connection,
    run_kind: &str,
    run_id: &str,
) -> Result<Option<String>> {
    conn.query_row(
        "SELECT id FROM execution_variable_snapshots WHERE run_kind=?1 AND run_id=?2",
        params![run_kind, run_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub struct NewSnapshot<'a> {
    pub run_kind: &'a str,
    pub run_id: &'a str,
    pub project_id: Option<&'a str>,
    pub environment_ref: &'a str,
    pub resolved_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub values: &'a HashMap<String, String>,
    pub provenance: &'a [VariableProvenance],
}

pub fn insert(conn: &Connection, snapshot: NewSnapshot<'_>, key: &[u8; 32]) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let plaintext = serde_json::to_string(snapshot.values)?;
    let encrypted = crypto::encrypt(&plaintext, key).map_err(anyhow::Error::msg)?;
    let fingerprint = crypto::keyed_digest(
        key,
        b"kronn.execution-variable-snapshot.v1",
        plaintext.as_bytes(),
    );
    let provenance = serde_json::to_string(snapshot.provenance)?;
    conn.execute("INSERT INTO execution_variable_snapshots (id, run_kind, run_id, project_id, environment_ref, resolved_at, expires_at, values_encrypted, fingerprint, provenance_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![id, snapshot.run_kind, snapshot.run_id, snapshot.project_id, snapshot.environment_ref, snapshot.resolved_at.to_rfc3339(), snapshot.expires_at.map(|v| v.to_rfc3339()), encrypted, fingerprint, provenance])?;
    Ok(id)
}

pub fn metadata(
    conn: &Connection,
    run_kind: &str,
    run_id: &str,
) -> Result<Option<SnapshotMetadata>> {
    conn.query_row("SELECT id,resolved_at,expires_at,provenance_json,values_encrypted IS NULL FROM execution_variable_snapshots WHERE run_kind=?1 AND run_id=?2", params![run_kind, run_id], |row| {
        let resolved: String = row.get(1)?; let expires: Option<String> = row.get(2)?; let provenance: String = row.get(3)?;
        Ok(SnapshotMetadata { id: row.get(0)?, resolved_at: DateTime::parse_from_rfc3339(&resolved).map(|v| v.with_timezone(&Utc)).map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?, expires_at: expires.and_then(|v| DateTime::parse_from_rfc3339(&v).ok()).map(|v| v.with_timezone(&Utc)), provenance: serde_json::from_str(&provenance).unwrap_or_default(), purged: row.get(4)? })
    }).optional().map_err(Into::into)
}

pub fn reveal(
    conn: &Connection,
    snapshot_id: &str,
    variable: &str,
    actor: &str,
    key: &[u8; 32],
    now: DateTime<Utc>,
) -> Result<Option<String>> {
    let encrypted: Option<Option<String>> = conn.query_row("SELECT values_encrypted FROM execution_variable_snapshots WHERE id=?1 AND (expires_at IS NULL OR expires_at>?2)", params![snapshot_id, now.to_rfc3339()], |row| row.get(0)).optional()?;
    let Some(Some(encrypted)) = encrypted else {
        return Ok(None);
    };
    let plaintext = crypto::decrypt(&encrypted, key).map_err(anyhow::Error::msg)?;
    let values: HashMap<String, String> =
        serde_json::from_str(&plaintext).context("invalid snapshot payload")?;
    let value = values.get(variable).cloned();
    if value.is_some() {
        conn.execute("INSERT INTO execution_variable_reveal_audit (id,snapshot_id,variable_name,actor,revealed_at) VALUES (?1,?2,?3,?4,?5)", params![Uuid::new_v4().to_string(), snapshot_id, variable, actor, now.to_rfc3339()])?;
    }
    Ok(value)
}

/// Decrypt the immutable values for execution. Unlike `reveal`, this internal
/// path does not create a human reveal audit row and never returns metadata to
/// an API response.
pub fn load_values(
    conn: &Connection,
    run_kind: &str,
    run_id: &str,
    key: &[u8; 32],
    now: DateTime<Utc>,
) -> Result<Option<HashMap<String, String>>> {
    let encrypted: Option<Option<String>> = conn
        .query_row(
            "SELECT values_encrypted FROM execution_variable_snapshots WHERE run_kind=?1 AND run_id=?2 AND (expires_at IS NULL OR expires_at>?3)",
            params![run_kind, run_id, now.to_rfc3339()],
            |row| row.get(0),
        )
        .optional()?;
    let Some(Some(encrypted)) = encrypted else {
        return Ok(None);
    };
    let plaintext = crypto::decrypt(&encrypted, key).map_err(anyhow::Error::msg)?;
    serde_json::from_str(&plaintext)
        .context("invalid snapshot payload")
        .map(Some)
}

pub fn purge_expired(conn: &Connection, now: DateTime<Utc>) -> Result<usize> {
    Ok(conn.execute("UPDATE execution_variable_snapshots SET values_encrypted=NULL,purged_at=?1 WHERE values_encrypted IS NOT NULL AND expires_at IS NOT NULL AND expires_at<=?1", [now.to_rfc3339()])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use sha2::Digest;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    fn sample<'a>(values: &'a HashMap<String, String>, now: DateTime<Utc>) -> NewSnapshot<'a> {
        NewSnapshot {
            run_kind: "workflow",
            run_id: "run-1",
            project_id: None,
            environment_ref: "project_mcp_configs",
            resolved_at: now,
            expires_at: Some(now + Duration::days(30)),
            values,
            provenance: &[],
        }
    }

    #[test]
    fn encrypted_snapshot_reveals_with_audit_then_purges_without_leaking_value() {
        let conn = conn();
        let now = Utc::now();
        let values = HashMap::from([("token".into(), "small-secret".into())]);
        let key = [7u8; 32];
        let id = insert(&conn, sample(&values, now), &key).unwrap();
        let (ciphertext, fingerprint): (String, String) = conn
            .query_row(
                "SELECT values_encrypted,fingerprint FROM execution_variable_snapshots WHERE id=?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(!ciphertext.contains("small-secret"));
        let bare_hash = sha2::Sha256::digest(b"{\"token\":\"small-secret\"}")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_ne!(fingerprint, bare_hash);
        assert_eq!(
            reveal(&conn, &id, "token", "tester", &key, now)
                .unwrap()
                .as_deref(),
            Some("small-secret")
        );
        assert_eq!(
            load_values(&conn, "workflow", "run-1", &key, now)
                .unwrap()
                .unwrap()["token"],
            "small-secret"
        );
        let audit: String = conn.query_row(
            "SELECT variable_name || ':' || actor FROM execution_variable_reveal_audit WHERE snapshot_id=?1",
            [&id], |row| row.get(0),
        ).unwrap();
        assert_eq!(audit, "token:tester");
        assert!(!audit.contains("small-secret"));
        assert_eq!(purge_expired(&conn, now + Duration::days(31)).unwrap(), 1);
        assert!(reveal(
            &conn,
            &id,
            "token",
            "tester",
            &key,
            now + Duration::days(31)
        )
        .unwrap()
        .is_none());
        assert!(
            metadata(&conn, "workflow", "run-1")
                .unwrap()
                .unwrap()
                .purged
        );
    }

    #[test]
    fn fingerprint_changes_with_active_key_policy() {
        let now = Utc::now();
        let values = HashMap::from([("token".into(), "guessable".into())]);
        let first = conn();
        insert(&first, sample(&values, now), &[1u8; 32]).unwrap();
        let a: String = first
            .query_row(
                "SELECT fingerprint FROM execution_variable_snapshots",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let second = conn();
        insert(&second, sample(&values, now), &[2u8; 32]).unwrap();
        let b: String = second
            .query_row(
                "SELECT fingerprint FROM execution_variable_snapshots",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(a, b);
    }
}
