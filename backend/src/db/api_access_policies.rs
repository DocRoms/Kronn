//! Storage for plugin access policies (KT-1026). Written only by the human
//! settings route; read by the broker on every call.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{ApiAccessPolicy, ApiAccessPolicyEntry};

pub fn get(conn: &Connection, server_id: &str) -> Result<Option<ApiAccessPolicy>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT policy_json FROM api_access_policies WHERE server_id = ?1",
            params![server_id],
            |row| row.get(0),
        )
        .optional()?;
    // An unreadable policy must not silently reopen the plugin.
    raw.map(|json| {
        serde_json::from_str(&json)
            .map_err(|e| anyhow::anyhow!("unreadable access policy for `{server_id}`: {e}"))
    })
    .transpose()
}

pub fn list(conn: &Connection) -> Result<Vec<ApiAccessPolicyEntry>> {
    let mut stmt =
        conn.prepare("SELECT server_id, policy_json FROM api_access_policies ORDER BY server_id")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (server_id, json) = row?;
        let policy = serde_json::from_str(&json)
            .map_err(|e| anyhow::anyhow!("unreadable access policy for `{server_id}`: {e}"))?;
        out.push(ApiAccessPolicyEntry { server_id, policy });
    }
    Ok(out)
}

pub fn set(conn: &Connection, server_id: &str, policy: &ApiAccessPolicy) -> Result<()> {
    conn.execute(
        "INSERT INTO api_access_policies(server_id, policy_json, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(server_id) DO UPDATE SET policy_json = excluded.policy_json,
                                              updated_at = excluded.updated_at",
        params![
            server_id,
            serde_json::to_string(policy)?,
            chrono::Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, server_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM api_access_policies WHERE server_id = ?1",
        params![server_id],
    )?;
    Ok(())
}
