use anyhow::Result;
use rusqlite::{params, Connection};
use std::collections::HashMap;

use crate::models::*;
use crate::core::crypto;

// ─── MCP Servers ─────────────────────────────────────────────────────────────

pub fn list_servers(conn: &Connection) -> Result<Vec<McpServer>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, transport, command, args_json, url, source
         FROM mcp_servers ORDER BY name"
    )?;

    let servers = stmt.query_map([], |row| {
        let transport_type: String = row.get(3)?;
        let command: Option<String> = row.get(4)?;
        let args_json: String = row.get(5)?;
        let url: Option<String> = row.get(6)?;
        let source_str: String = row.get(7)?;

        let transport = match transport_type.as_str() {
            "stdio" => McpTransport::Stdio {
                command: command.unwrap_or_default(),
                args: serde_json::from_str(&args_json).unwrap_or_default(),
            },
            "sse" => McpTransport::Sse {
                url: url.unwrap_or_default(),
            },
            "streamable" => McpTransport::Streamable {
                url: url.unwrap_or_default(),
            },
            _ => McpTransport::Stdio {
                command: command.unwrap_or_default(),
                args: serde_json::from_str(&args_json).unwrap_or_default(),
            },
        };

        let source = match source_str.as_str() {
            "registry" => McpSource::Registry,
            "manual" => McpSource::Manual,
            _ => McpSource::Detected,
        };

        Ok(McpServer {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            transport,
            source,
        })
    })?.filter_map(|r| r.ok()).collect();

    Ok(servers)
}

pub fn upsert_server(conn: &Connection, server: &McpServer) -> Result<()> {
    let (transport_type, command, args_json, url) = match &server.transport {
        McpTransport::Stdio { command, args } => (
            "stdio",
            Some(command.clone()),
            serde_json::to_string(args)?,
            None,
        ),
        McpTransport::Sse { url } => ("sse", None, "[]".to_string(), Some(url.clone())),
        McpTransport::Streamable { url } => ("streamable", None, "[]".to_string(), Some(url.clone())),
    };

    let source_str = match server.source {
        McpSource::Registry => "registry",
        McpSource::Detected => "detected",
        McpSource::Manual => "manual",
    };

    conn.execute(
        "INSERT INTO mcp_servers (id, name, description, transport, command, args_json, url, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
           name = excluded.name,
           description = excluded.description,
           transport = excluded.transport,
           command = excluded.command,
           args_json = excluded.args_json,
           url = excluded.url,
           source = excluded.source",
        params![
            server.id,
            server.name,
            server.description,
            transport_type,
            command,
            args_json,
            url,
            source_str,
        ],
    )?;
    Ok(())
}

#[allow(dead_code)]
pub fn delete_server(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM mcp_servers WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

// ─── MCP Configs ─────────────────────────────────────────────────────────────

pub fn list_configs(conn: &Connection) -> Result<Vec<McpConfig>> {
    let mut stmt = conn.prepare(
        "SELECT id, server_id, label, env_encrypted, env_keys_json, args_override, is_global, config_hash, include_general
         FROM mcp_configs ORDER BY label"
    )?;

    let configs: Vec<McpConfig> = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let env_keys_json: String = row.get(4)?;
        let args_str: Option<String> = row.get(5)?;

        Ok((id.clone(), McpConfig {
            id,
            server_id: row.get(1)?,
            label: row.get(2)?,
            env_keys: serde_json::from_str(&env_keys_json).unwrap_or_default(),
            env_encrypted: row.get(3)?,
            args_override: args_str.and_then(|s| serde_json::from_str(&s).ok()),
            is_global: row.get::<_, i32>(6)? != 0,
            config_hash: row.get(7)?,
            include_general: row.get::<_, i32>(8).unwrap_or(1) != 0,
            project_ids: vec![], // loaded below
        }))
    })?.filter_map(|r| r.ok())
    .map(|(id, mut config)| {
        config.project_ids = list_config_project_ids(conn, &id).unwrap_or_default();
        config
    })
    .collect();

    Ok(configs)
}

pub fn get_config(conn: &Connection, id: &str) -> Result<Option<McpConfig>> {
    let configs = list_configs(conn)?;
    Ok(configs.into_iter().find(|c| c.id == id))
}

pub fn find_config_by_hash(conn: &Connection, hash: &str) -> Result<Option<McpConfig>> {
    let configs = list_configs(conn)?;
    Ok(configs.into_iter().find(|c| c.config_hash == hash))
}

pub fn insert_config(conn: &Connection, config: &McpConfig) -> Result<()> {
    let env_keys_json = serde_json::to_string(&config.env_keys)?;
    let args_json = config.args_override.as_ref()
        .map(serde_json::to_string)
        .transpose()?;

    conn.execute(
        "INSERT INTO mcp_configs (id, server_id, label, env_encrypted, env_keys_json, args_override, is_global, config_hash, include_general)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            config.id,
            config.server_id,
            config.label,
            config.env_encrypted,
            env_keys_json,
            args_json,
            config.is_global as i32,
            config.config_hash,
            config.include_general as i32,
        ],
    )?;

    // Insert project linkages
    for pid in &config.project_ids {
        link_config_project(conn, &config.id, pid)?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn update_config(conn: &Connection, id: &str, label: Option<&str>, env_encrypted: Option<&str>, env_keys: Option<&[String]>, args_override: Option<&Vec<String>>, is_global: Option<bool>, config_hash: Option<&str>, include_general: Option<bool>) -> Result<bool> {
    // Load existing, apply changes, write back
    let existing = match get_config(conn, id)? {
        Some(c) => c,
        None => return Ok(false),
    };

    let new_label = label.unwrap_or(&existing.label);
    let new_enc = env_encrypted.unwrap_or(&existing.env_encrypted);
    let new_keys = env_keys.map(|k| k.to_vec()).unwrap_or(existing.env_keys.clone());
    let new_args = args_override.cloned().or(existing.args_override.clone());
    let new_global = is_global.unwrap_or(existing.is_global);
    let new_include_general = include_general.unwrap_or(existing.include_general);
    let new_hash = config_hash.unwrap_or(&existing.config_hash);

    let env_keys_json = serde_json::to_string(&new_keys)?;
    let args_json = new_args.as_ref()
        .map(serde_json::to_string)
        .transpose()?;

    let affected = conn.execute(
        "UPDATE mcp_configs SET label = ?1, env_encrypted = ?2, env_keys_json = ?3, args_override = ?4, is_global = ?5, config_hash = ?6, include_general = ?7 WHERE id = ?8",
        params![new_label, new_enc, env_keys_json, args_json, new_global as i32, new_hash, new_include_general as i32, id],
    )?;
    Ok(affected > 0)
}

pub fn delete_config(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM mcp_configs WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

// ─── Config ↔ Project linkage ────────────────────────────────────────────────

fn list_config_project_ids(conn: &Connection, config_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT project_id FROM mcp_config_projects WHERE config_id = ?1"
    )?;
    let ids = stmt.query_map(params![config_id], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(ids)
}

pub fn link_config_project(conn: &Connection, config_id: &str, project_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO mcp_config_projects (config_id, project_id) VALUES (?1, ?2)",
        params![config_id, project_id],
    )?;
    Ok(())
}

#[allow(dead_code)]
pub fn unlink_config_project(conn: &Connection, config_id: &str, project_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM mcp_config_projects WHERE config_id = ?1 AND project_id = ?2",
        params![config_id, project_id],
    )?;
    Ok(())
}

pub fn set_config_projects(conn: &Connection, config_id: &str, project_ids: &[String]) -> Result<()> {
    conn.execute(
        "DELETE FROM mcp_config_projects WHERE config_id = ?1",
        params![config_id],
    )?;
    for pid in project_ids {
        link_config_project(conn, config_id, pid)?;
    }
    Ok(())
}

/// Get all configs linked to a specific project (including global ones)
pub fn configs_for_project(conn: &Connection, project_id: &str) -> Result<Vec<McpConfig>> {
    let all = list_configs(conn)?;
    Ok(all.into_iter().filter(|c| {
        c.is_global || c.project_ids.contains(&project_id.to_string())
    }).collect())
}

// ─── Display helpers ─────────────────────────────────────────────────────────

/// Build McpConfigDisplay list with masked secrets and server names.
/// Pass `secret` to detect broken encryption (secrets_broken flag).
pub fn list_configs_display(conn: &Connection, secret: Option<&str>) -> Result<Vec<McpConfigDisplay>> {
    let configs = list_configs(conn)?;
    let servers = list_servers(conn)?;

    let projects = crate::db::projects::list_projects(conn)?;
    let project_map: HashMap<String, String> = projects.into_iter()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect();

    let server_map: HashMap<String, String> = servers.iter()
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();

    Ok(configs.into_iter().map(|c| {
        let env_masked: Vec<McpEnvEntry> = c.env_keys.iter()
            .map(|k| McpEnvEntry {
                key: k.clone(),
                masked_value: "••••••".to_string(),
            })
            .collect();

        let project_names: Vec<String> = c.project_ids.iter()
            .filter_map(|pid| project_map.get(pid).cloned())
            .collect();

        // Detect broken encryption: env_keys exist but decryption fails
        let secrets_broken = if !c.env_keys.is_empty() && !c.env_encrypted.is_empty() {
            if let Some(s) = secret {
                decrypt_env(&c.env_encrypted, s).is_err()
            } else {
                false
            }
        } else {
            false
        };

        McpConfigDisplay {
            id: c.id,
            server_id: c.server_id.clone(),
            server_name: server_map.get(&c.server_id).cloned().unwrap_or_default(),
            label: c.label,
            env_keys: c.env_keys,
            env_masked,
            args_override: c.args_override,
            is_global: c.is_global,
            include_general: c.include_general,
            config_hash: c.config_hash,
            project_ids: c.project_ids,
            project_names,
            secrets_broken,
        }
    }).collect())
}

// ─── Config hash computation ─────────────────────────────────────────────────

/// Compute a deduplication hash from the server command+args+env values.
/// Two configs with the same hash are functionally identical.
pub fn compute_config_hash(server: &McpServer, env: &HashMap<String, String>, args_override: Option<&Vec<String>>) -> String {
    use std::collections::BTreeMap;
    let mut parts = vec![];

    // Transport identity
    match &server.transport {
        McpTransport::Stdio { command, args } => {
            parts.push(format!("stdio:{}:{}", command, args.join(",")));
        }
        McpTransport::Sse { url } => parts.push(format!("sse:{}", url)),
        McpTransport::Streamable { url } => parts.push(format!("streamable:{}", url)),
    }

    // Args override
    if let Some(args) = args_override {
        parts.push(format!("args:{}", args.join(",")));
    }

    // Env values sorted by key for determinism
    let sorted: BTreeMap<_, _> = env.iter().collect();
    for (k, v) in sorted {
        parts.push(format!("{}={}", k, v));
    }

    // Simple hash: use first 16 chars of hex-encoded digest
    let input = parts.join("|");
    simple_hash(&input)
}

/// Simple string hash (not cryptographic, just for dedup grouping)
fn simple_hash(input: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV-1a prime
    }
    format!("{:016x}", hash)
}

// ─── Encrypt/decrypt env helpers ─────────────────────────────────────────────

pub fn encrypt_env(env: &HashMap<String, String>, secret: &str) -> Result<String, String> {
    if env.is_empty() {
        return Ok(String::new());
    }
    let json = serde_json::to_string(env).map_err(|e| e.to_string())?;
    let key = crypto::parse_secret(secret)?;
    crypto::encrypt(&json, &key)
}

pub fn decrypt_env(encrypted: &str, secret: &str) -> Result<HashMap<String, String>, String> {
    if encrypted.is_empty() {
        return Ok(HashMap::new());
    }
    let key = crypto::parse_secret(secret)?;
    let json = crypto::decrypt(encrypted, &key)?;
    serde_json::from_str(&json).map_err(|e| format!("Invalid env JSON: {}", e))
}
