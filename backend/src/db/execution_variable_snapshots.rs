use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::core::{crypto, execution_variables::VariableProvenance};

pub struct SnapshotMetadata {
    pub id: String,
    pub resolved_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub provenance: Vec<VariableProvenance>,
    pub purged: bool,
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
    let fingerprint = Sha256::digest(plaintext.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let provenance = serde_json::to_string(snapshot.provenance)?;
    conn.execute("INSERT INTO execution_variable_snapshots (id, run_kind, run_id, project_id, environment_ref, resolved_at, expires_at, values_encrypted, fingerprint, provenance_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![id, snapshot.run_kind, snapshot.run_id, snapshot.project_id, snapshot.environment_ref, snapshot.resolved_at.to_rfc3339(), snapshot.expires_at.map(|v| v.to_rfc3339()), encrypted, fingerprint, provenance])?;
    Ok(id)
}

pub fn metadata(conn: &Connection, run_kind: &str, run_id: &str) -> Result<Option<SnapshotMetadata>> {
    conn.query_row("SELECT id,resolved_at,expires_at,provenance_json,values_encrypted IS NULL FROM execution_variable_snapshots WHERE run_kind=?1 AND run_id=?2", params![run_kind, run_id], |row| {
        let resolved: String = row.get(1)?; let expires: Option<String> = row.get(2)?; let provenance: String = row.get(3)?;
        Ok(SnapshotMetadata { id: row.get(0)?, resolved_at: DateTime::parse_from_rfc3339(&resolved).map(|v| v.with_timezone(&Utc)).map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?, expires_at: expires.and_then(|v| DateTime::parse_from_rfc3339(&v).ok()).map(|v| v.with_timezone(&Utc)), provenance: serde_json::from_str(&provenance).unwrap_or_default(), purged: row.get(4)? })
    }).optional().map_err(Into::into)
}

pub fn reveal(conn: &Connection, snapshot_id: &str, variable: &str, actor: &str, key: &[u8; 32], now: DateTime<Utc>) -> Result<Option<String>> {
    let encrypted: Option<Option<String>> = conn.query_row("SELECT values_encrypted FROM execution_variable_snapshots WHERE id=?1 AND (expires_at IS NULL OR expires_at>?2)", params![snapshot_id, now.to_rfc3339()], |row| row.get(0)).optional()?;
    let Some(Some(encrypted)) = encrypted else { return Ok(None) };
    let plaintext = crypto::decrypt(&encrypted, key).map_err(anyhow::Error::msg)?;
    let values: HashMap<String, String> = serde_json::from_str(&plaintext).context("invalid snapshot payload")?;
    let value = values.get(variable).cloned();
    if value.is_some() { conn.execute("INSERT INTO execution_variable_reveal_audit (id,snapshot_id,variable_name,actor,revealed_at) VALUES (?1,?2,?3,?4,?5)", params![Uuid::new_v4().to_string(), snapshot_id, variable, actor, now.to_rfc3339()])?; }
    Ok(value)
}

pub fn purge_expired(conn: &Connection, now: DateTime<Utc>) -> Result<usize> {
    Ok(conn.execute("UPDATE execution_variable_snapshots SET values_encrypted=NULL,purged_at=?1 WHERE values_encrypted IS NOT NULL AND expires_at IS NOT NULL AND expires_at<=?1", [now.to_rfc3339()])?)
}
