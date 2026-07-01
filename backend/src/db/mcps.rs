use anyhow::Result;
use rusqlite::{params, Connection};
use std::collections::HashMap;

use crate::models::*;
use crate::core::crypto;

// ─── MCP Servers ─────────────────────────────────────────────────────────────

pub fn list_servers(conn: &Connection) -> Result<Vec<McpServer>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, transport, command, args_json, url, source, api_spec_json
         FROM mcp_servers ORDER BY name"
    )?;

    let servers = stmt.query_map([], |row| {
        let transport_type: String = row.get(3)?;
        let command: Option<String> = row.get(4)?;
        let args_json: String = row.get(5)?;
        let url: Option<String> = row.get(6)?;
        let source_str: String = row.get(7)?;
        let api_spec_json: Option<String> = row.get(8)?;

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
            "api_only" => McpTransport::ApiOnly,
            _ => McpTransport::Stdio {
                command: command.unwrap_or_default(),
                args: serde_json::from_str(&args_json).unwrap_or_default(),
            },
        };

        let source = match source_str.as_str() {
            "registry" => McpSource::Registry,
            "manual" => McpSource::Manual,
            "host_imported" => McpSource::HostImported,
            _ => McpSource::Detected,
        };

        // Parse api_spec_json — silent fallback to None on malformed JSON so
        // one corrupt row doesn't break the whole plugin list load.
        let api_spec = api_spec_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<crate::models::ApiSpec>(raw).ok());

        Ok(McpServer {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            transport,
            source,
            api_spec,
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
        McpTransport::ApiOnly => ("api_only", None, "[]".to_string(), None),
    };

    let source_str = match server.source {
        McpSource::Registry => "registry",
        McpSource::Detected => "detected",
        McpSource::Manual => "manual",
        McpSource::HostImported => "host_imported",
    };

    let api_spec_json = server.api_spec.as_ref()
        .map(serde_json::to_string)
        .transpose()?;

    conn.execute(
        "INSERT INTO mcp_servers (id, name, description, transport, command, args_json, url, source, api_spec_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
           name = excluded.name,
           description = excluded.description,
           transport = excluded.transport,
           command = excluded.command,
           args_json = excluded.args_json,
           url = excluded.url,
           source = excluded.source,
           api_spec_json = excluded.api_spec_json",
        params![
            server.id,
            server.name,
            server.description,
            transport_type,
            command,
            args_json,
            url,
            source_str,
            api_spec_json,
        ],
    )?;
    Ok(())
}

#[allow(dead_code)]
pub fn delete_server(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM mcp_servers WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

/// Re-sync registry-derived fields (`api_spec`, `description`, `transport`)
/// onto every existing DB row whose id matches a builtin definition.
///
/// **Why this exists**: when the registry gains a new field on a definition
/// (e.g. we added `api_spec` to `mcp-github` so it could power ApiCall
/// steps), users who configured GitHub *before* the registry change still
/// had the old DB row with `api_spec: None` — so the workflow wizard
/// silently filtered GitHub out of the API plugin picker. This function,
/// called on backend startup, makes the registry the source of truth for
/// system-managed fields without touching anything user-managed (env
/// secrets, labels, project links).
///
/// Only updates EXISTING rows. New registry entries the user hasn't yet
/// added stay registry-only — they appear in `mcps/registry` but not in
/// `mcps` until the user explicitly creates a config.
pub fn sync_registry_servers_to_db(
    conn: &Connection,
    registry: &[crate::models::McpDefinition],
) -> Result<usize> {
    let mut updated = 0;
    let existing = list_servers(conn)?;
    let existing_ids: std::collections::HashSet<String> =
        existing.iter().map(|s| s.id.clone()).collect();

    for def in registry {
        if !existing_ids.contains(&def.id) {
            continue;
        }
        let server = crate::models::McpServer {
            id: def.id.clone(),
            name: def.name.clone(),
            description: def.description.clone(),
            transport: def.transport.clone(),
            source: crate::models::McpSource::Registry,
            api_spec: def.api_spec.clone(),
        };
        upsert_server(conn, &server)?;
        updated += 1;
    }
    Ok(updated)
}

// ─── MCP Configs ─────────────────────────────────────────────────────────────

pub fn list_configs(conn: &Connection) -> Result<Vec<McpConfig>> {
    let mut stmt = conn.prepare(
        "SELECT id, server_id, label, env_encrypted, env_keys_json, args_override, is_global, config_hash, include_general, host_sync
         FROM mcp_configs ORDER BY label"
    )?;

    let configs: Vec<McpConfig> = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let env_keys_json: String = row.get(4)?;
        let args_str: Option<String> = row.get(5)?;
        let host_sync_str: String = row.get::<_, Option<String>>(9)?
            .unwrap_or_else(|| "None".to_string());

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
            host_sync: parse_host_sync(&host_sync_str),
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
        "INSERT INTO mcp_configs (id, server_id, label, env_encrypted, env_keys_json, args_override, is_global, config_hash, include_general, host_sync)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
            host_sync_to_str(&config.host_sync),
        ],
    )?;

    // Insert project linkages
    for pid in &config.project_ids {
        link_config_project(conn, &config.id, pid)?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn update_config(conn: &Connection, id: &str, label: Option<&str>, env_encrypted: Option<&str>, env_keys: Option<&[String]>, args_override: Option<&Vec<String>>, is_global: Option<bool>, config_hash: Option<&str>, include_general: Option<bool>, host_sync: Option<HostSyncMode>) -> Result<bool> {
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
    let new_host_sync = host_sync.unwrap_or(existing.host_sync.clone());

    let env_keys_json = serde_json::to_string(&new_keys)?;
    let args_json = new_args.as_ref()
        .map(serde_json::to_string)
        .transpose()?;

    let affected = conn.execute(
        "UPDATE mcp_configs SET label = ?1, env_encrypted = ?2, env_keys_json = ?3, args_override = ?4, is_global = ?5, config_hash = ?6, include_general = ?7, host_sync = ?8 WHERE id = ?9",
        params![new_label, new_enc, env_keys_json, args_json, new_global as i32, new_hash, new_include_general as i32, host_sync_to_str(&new_host_sync), id],
    )?;
    Ok(affected > 0)
}

// ─── HostSyncMode <-> string helpers ─────────────────────────────────────────

/// Serialize HostSyncMode → SQL TEXT. Stable wire format independent of
/// serde so a future enum rename doesn't break the migration.
pub(crate) fn host_sync_to_str(mode: &HostSyncMode) -> &'static str {
    match mode {
        HostSyncMode::None => "None",
        HostSyncMode::GlobalOnly => "GlobalOnly",
        HostSyncMode::MirrorAll => "MirrorAll",
    }
}

/// Parse SQL TEXT → HostSyncMode. Unknown values fall back to `None`
/// (safer than failing — preserves data, never auto-syncs).
pub(crate) fn parse_host_sync(s: &str) -> HostSyncMode {
    match s {
        "GlobalOnly" => HostSyncMode::GlobalOnly,
        "MirrorAll" => HostSyncMode::MirrorAll,
        _ => HostSyncMode::None,
    }
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

/// Pick THIS instance's config id for a given plugin/server, used when a
/// workflow is imported/cloned and its ApiCall steps carry the source
/// instance's `api_config_id` (a UUID meaningless here). Preference order:
/// a config scoped to `project_id`, then a global one, then any config for
/// that server. `None` when this instance has no config for the plugin (the
/// caller leaves the step's id as-is so the user picks one). Server ids
/// (`api_plugin_slug`) are stable across instances; config ids are not.
pub fn find_config_for_server(
    conn: &Connection,
    server_id: &str,
    project_id: Option<&str>,
) -> Result<Option<String>> {
    let all = list_configs(conn)?;
    if let Some(pid) = project_id {
        if let Some(c) = all.iter().find(|c|
            c.server_id == server_id && c.project_ids.iter().any(|p| p == pid))
        {
            return Ok(Some(c.id.clone()));
        }
    }
    if let Some(c) = all.iter().find(|c| c.server_id == server_id && c.is_global) {
        return Ok(Some(c.id.clone()));
    }
    Ok(all.into_iter().find(|c| c.server_id == server_id).map(|c| c.id))
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
            host_sync: c.host_sync,
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
        // API-only plugins use the server id as identity — they can't
        // collide with MCP-transport configs because the env keys differ.
        McpTransport::ApiOnly => parts.push(format!("api_only:{}", server.id)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{HostSyncMode, McpServer, McpSource, McpTransport};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    fn mk_server(id: &str) -> McpServer {
        McpServer {
            id: id.into(),
            name: format!("Server-{id}"),
            description: "Test".into(),
            transport: McpTransport::Stdio { command: "echo".into(), args: vec![] },
            source: McpSource::Registry,
            api_spec: None,
        }
    }

    #[test]
    fn list_servers_empty_returns_empty_vec() {
        let conn = test_conn();
        assert!(list_servers(&conn).unwrap().is_empty());
    }

    #[test]
    fn upsert_then_list_returns_one_row() {
        let conn = test_conn();
        upsert_server(&conn, &mk_server("s-1")).unwrap();
        let list = list_servers(&conn).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "s-1");
    }

    #[test]
    fn upsert_is_idempotent_on_same_id() {
        let conn = test_conn();
        upsert_server(&conn, &mk_server("s-x")).unwrap();
        let mut server = mk_server("s-x");
        server.description = "Updated".into();
        upsert_server(&conn, &server).unwrap();
        let list = list_servers(&conn).unwrap();
        assert_eq!(list.len(), 1, "idempotent upsert must not duplicate");
        assert_eq!(list[0].description, "Updated");
    }

    #[test]
    fn delete_server_unknown_returns_false() {
        let conn = test_conn();
        let removed = delete_server(&conn, "nope").unwrap();
        assert!(!removed);
    }

    #[test]
    fn delete_server_existing_returns_true_and_removes_row() {
        let conn = test_conn();
        upsert_server(&conn, &mk_server("s-del")).unwrap();
        let removed = delete_server(&conn, "s-del").unwrap();
        assert!(removed);
        assert!(list_servers(&conn).unwrap().is_empty());
    }

    #[test]
    fn list_configs_empty_returns_empty_vec() {
        let conn = test_conn();
        assert!(list_configs(&conn).unwrap().is_empty());
    }

    #[test]
    fn get_config_unknown_returns_none() {
        let conn = test_conn();
        assert!(get_config(&conn, "nope").unwrap().is_none());
    }

    #[test]
    fn find_config_by_hash_unknown_returns_none() {
        let conn = test_conn();
        assert!(find_config_by_hash(&conn, "deadbeef").unwrap().is_none());
    }

    #[test]
    fn insert_config_then_find_by_hash() {
        let conn = test_conn();
        upsert_server(&conn, &mk_server("s-cfg")).unwrap();
        let cfg = crate::models::McpConfig {
            id: "cfg-1".into(),
            server_id: "s-cfg".into(),
            label: "Prod".into(),
            env_keys: vec!["TOKEN".into()],
            env_encrypted: "x".into(),
            args_override: None,
            is_global: false,
            config_hash: "abcdef".into(),
            project_ids: vec![],
            host_sync: HostSyncMode::GlobalOnly,
            include_general: true,
        };
        insert_config(&conn, &cfg).unwrap();

        let found = find_config_by_hash(&conn, "abcdef").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().label, "Prod");
    }

    #[test]
    fn find_config_for_server_prefers_project_then_global_then_any() {
        let conn = test_conn();
        upsert_server(&conn, &mk_server("gh")).unwrap();
        upsert_server(&conn, &mk_server("jira")).unwrap();
        let mk = |id: &str, server: &str, global: bool| crate::models::McpConfig {
            id: id.into(), server_id: server.into(), label: "L".into(),
            env_keys: vec![], env_encrypted: "x".into(), args_override: None,
            is_global: global, config_hash: id.into(), project_ids: vec![],
            host_sync: HostSyncMode::GlobalOnly, include_general: true,
        };
        conn.execute("INSERT INTO projects (id, name, path, created_at, updated_at) VALUES ('proj-1','Test','/tmp/proj1','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')", []).unwrap();
        insert_config(&conn, &mk("gh-global", "gh", true)).unwrap();
        insert_config(&conn, &mk("gh-proj", "gh", false)).unwrap();
        link_config_project(&conn, "gh-proj", "proj-1").unwrap();
        insert_config(&conn, &mk("jira-1", "jira", true)).unwrap();

        // Project-scoped config wins for that project.
        assert_eq!(find_config_for_server(&conn, "gh", Some("proj-1")).unwrap().as_deref(), Some("gh-proj"));
        // No project → prefer the global config for the server.
        assert_eq!(find_config_for_server(&conn, "gh", None).unwrap().as_deref(), Some("gh-global"));
        // Unknown project → falls back to the global config.
        assert_eq!(find_config_for_server(&conn, "gh", Some("other")).unwrap().as_deref(), Some("gh-global"));
        // A different server resolves to its own config.
        assert_eq!(find_config_for_server(&conn, "jira", None).unwrap().as_deref(), Some("jira-1"));
        // No config for the plugin → None (caller leaves the step id untouched).
        assert!(find_config_for_server(&conn, "slack", None).unwrap().is_none());
    }

    #[test]
    fn update_config_unknown_id_returns_false() {
        let conn = test_conn();
        let changed = update_config(&conn, "nope", Some("X"),
            None, None, None, None, None, None, None).unwrap();
        assert!(!changed);
    }

    #[test]
    fn delete_config_unknown_returns_false() {
        let conn = test_conn();
        assert!(!delete_config(&conn, "nope").unwrap());
    }

    /// Helper: seed a server + one config whose env is encrypted under `secret`.
    fn seed_encrypted_config(conn: &Connection, secret: &str, id: &str) {
        upsert_server(conn, &mk_server("srv")).ok();
        let mut env = HashMap::new();
        env.insert("TOKEN".to_string(), "s3cr3t".to_string());
        let enc = encrypt_env(&env, secret).unwrap();
        let cfg = crate::models::McpConfig {
            id: id.into(),
            server_id: "srv".into(),
            label: "L".into(),
            env_keys: vec!["TOKEN".into()],
            env_encrypted: enc,
            args_override: None,
            is_global: false,
            config_hash: id.into(),
            project_ids: vec![],
            host_sync: HostSyncMode::GlobalOnly,
            include_general: true,
        };
        insert_config(conn, &cfg).unwrap();
    }

    #[test]
    fn encrypt_env_empty_map_returns_empty_string() {
        // Empty env → "" so the reconciler/locked-state treat it as "no secret".
        let secret = crypto::generate_secret();
        assert_eq!(encrypt_env(&HashMap::new(), &secret).unwrap(), "");
    }

    #[test]
    fn encrypt_then_decrypt_env_roundtrips() {
        let secret = crypto::generate_secret();
        let mut env = HashMap::new();
        env.insert("A".to_string(), "1".to_string());
        env.insert("B".to_string(), "two".to_string());
        let enc = encrypt_env(&env, &secret).unwrap();
        assert_eq!(decrypt_env(&enc, &secret).unwrap(), env);
    }

    /// The locked-state AUTHORITY: `secrets_broken` must be TRUE exactly when the
    /// active key cannot decrypt a config that has secrets — this is what the UI
    /// renders as "🔒 needs re-key" and what proves nothing was silently lost.
    #[test]
    fn list_display_flags_broken_only_for_undecryptable_rows() {
        let conn = test_conn();
        let good = crypto::generate_secret();
        let wrong = crypto::generate_secret();
        seed_encrypted_config(&conn, &good, "cfg-good");

        // Right key → not broken.
        let disp = list_configs_display(&conn, Some(&good)).unwrap();
        assert_eq!(disp.len(), 1);
        assert!(!disp[0].secrets_broken, "correct key must NOT flag broken");

        // Wrong key → broken (locked): the row is present, ciphertext intact,
        // just not decryptable — exactly the recoverable-by-re-key state.
        let disp_wrong = list_configs_display(&conn, Some(&wrong)).unwrap();
        assert!(disp_wrong[0].secrets_broken, "wrong key MUST flag broken/locked");
    }

    #[test]
    fn list_display_not_broken_when_no_secrets_present() {
        let conn = test_conn();
        upsert_server(&conn, &mk_server("srv")).unwrap();
        // A config with NO env_keys / empty ciphertext is never "broken".
        let cfg = crate::models::McpConfig {
            id: "cfg-empty".into(), server_id: "srv".into(), label: "L".into(),
            env_keys: vec![], env_encrypted: String::new(), args_override: None,
            is_global: false, config_hash: "h".into(), project_ids: vec![],
            host_sync: HostSyncMode::GlobalOnly, include_general: true,
        };
        insert_config(&conn, &cfg).unwrap();
        let disp = list_configs_display(&conn, Some(&crypto::generate_secret())).unwrap();
        assert!(!disp[0].secrets_broken, "a keyless config is never broken");
    }

    #[test]
    fn list_display_no_active_key_does_not_false_flag_broken() {
        // When no key is available (None), we can't prove a row is broken, so we
        // must NOT mark it broken (avoids a scary false "locked" with no key).
        let conn = test_conn();
        let good = crypto::generate_secret();
        seed_encrypted_config(&conn, &good, "cfg-x");
        let disp = list_configs_display(&conn, None).unwrap();
        assert!(!disp[0].secrets_broken, "no active key → do not false-flag broken");
    }

    #[test]
    fn decrypt_env_empty_returns_empty_hashmap() {
        let secret = crypto::generate_secret();
        let result = decrypt_env("", &secret).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn decrypt_env_garbage_ciphertext_returns_err() {
        let secret = crypto::generate_secret();
        let result = decrypt_env("not-base64-and-not-ciphertext", &secret);
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_env_with_bogus_secret_returns_err() {
        let result = decrypt_env("some-ciphertext", "not-a-hex-secret");
        assert!(result.is_err());
    }

    #[test]
    fn encrypt_decrypt_env_roundtrip() {
        let secret = crypto::generate_secret();
        let key = crypto::parse_secret(&secret).unwrap();

        let mut env = HashMap::new();
        env.insert("TOKEN".to_string(), "secret-value-42".to_string());
        env.insert("API_KEY".to_string(), "another-secret".to_string());

        let json = serde_json::to_string(&env).unwrap();
        let encrypted = crypto::encrypt(&json, &key).unwrap();

        let decrypted = decrypt_env(&encrypted, &secret).unwrap();
        assert_eq!(decrypted, env);
    }
}
