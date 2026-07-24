//! Inbound discovery of MCP entries declared in host-level CLI config files.
//!
//! Phase 1 of the inbound/outbound feature: scans `~/.claude.json`,
//! `~/.gemini/settings.json`, `~/.codex/config.toml`, and
//! `~/.copilot/mcp-config.json` to surface MCPs the user configured outside
//! Kronn. **Read-only**: no DB writes, no file mutations.
//!
//! Ownership detection works two ways:
//! - **Marker** (`_kronn.config_id` field on the entry) — strongest signal,
//!   set by Kronn itself when it writes the entry (Phase 3, not yet shipped).
//! - **Hash match** — recompute the dedup hash from the discovered entry's
//!   transport+env values and look it up in `mcp_configs.config_hash`.
//!
//! Path resolution mirrors `mcp_scanner.rs::sync_codex_global_config`: prefer
//! `KRONN_HOST_HOME` (Docker → host bind) then `HOME`/`USERPROFILE`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::db;
use crate::models::{McpServer, McpSource, McpTransport};

// ─── Public types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq, Hash)]
#[ts(export)]
#[serde(tag = "kind", content = "value")]
pub enum HostScope {
    /// `~/.claude.json` top-level `mcpServers` (Claude `--scope user`).
    ClaudeUser,
    /// `~/.claude.json` `projects[<abs-path>].mcpServers` (Claude `--scope local`).
    ClaudeLocal { project_path: String },
    /// `~/.gemini/settings.json` `mcpServers`.
    Gemini,
    /// `~/.codex/config.toml` `[mcp_servers.*]`.
    Codex,
    /// `~/.copilot/mcp-config.json` `mcpServers`.
    Copilot,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
#[serde(tag = "type", content = "config_id")]
pub enum KronnOwnership {
    /// Entry has no `_kronn` marker and its hash does not match any
    /// `mcp_configs.config_hash`.
    NotManaged,
    /// `_kronn.config_id` field present on the entry. Strongest signal.
    ManagedByMarker(String),
    /// Hash matches an existing `mcp_configs` row (entry written before
    /// markers were introduced, or by a different Kronn instance).
    ManagedByHash(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscoveredHostMcp {
    pub source_file: String,
    pub scope: HostScope,
    pub name: String,
    pub transport: McpTransport,
    /// Env variable **names** declared by the entry. Values are never
    /// surfaced through this struct (even though the file already exposes
    /// them locally) so the API response is safe to log.
    pub env_keys: Vec<String>,
    pub managed_by_kronn: KronnOwnership,
}

// ─── Path resolution ─────────────────────────────────────────────────────────

/// Resolve the user's home directory the same way `mcp_scanner.rs` does.
/// Returns `None` only when no env var is set AND `directories` cannot infer
/// it (extremely unusual — would only happen in heavily sandboxed runtimes).
pub fn resolve_home() -> Option<PathBuf> {
    if let Ok(host_home) = std::env::var("KRONN_HOST_HOME") {
        if !host_home.is_empty() {
            return Some(PathBuf::from(host_home));
        }
    }
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return Some(PathBuf::from(h));
        }
    }
    if let Ok(p) = std::env::var("USERPROFILE") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

// ─── Ownership index ─────────────────────────────────────────────────────────

/// Pre-computed lookup tables built once per scan. Avoids N×M comparison.
pub(crate) struct OwnershipIndex {
    /// `mcp_configs.config_hash → config_id`.
    by_hash: HashMap<String, String>,
}

impl OwnershipIndex {
    fn from_db(conn: &Connection) -> Result<Self> {
        let configs = db::mcps::list_configs(conn)?;
        let by_hash = configs.into_iter().map(|c| (c.config_hash, c.id)).collect();
        Ok(Self { by_hash })
    }

    /// Resolve ownership: marker wins over hash.
    fn classify(&self, marker_id: Option<&str>, hash: &str) -> KronnOwnership {
        if let Some(id) = marker_id {
            return KronnOwnership::ManagedByMarker(id.to_string());
        }
        if let Some(id) = self.by_hash.get(hash) {
            return KronnOwnership::ManagedByHash(id.clone());
        }
        KronnOwnership::NotManaged
    }
}

// ─── Public entry point ──────────────────────────────────────────────────────

/// Scan all 4 host config files. Returns an empty vec when the home dir
/// cannot be resolved — never errors (best-effort discovery).
pub fn scan_all_host_mcps(conn: &Connection) -> Vec<DiscoveredHostMcp> {
    let home = match resolve_home() {
        Some(h) => h,
        None => {
            tracing::warn!("host_mcp_discovery: cannot resolve home dir");
            return Vec::new();
        }
    };
    let index = match OwnershipIndex::from_db(conn) {
        Ok(idx) => idx,
        Err(e) => {
            tracing::warn!("host_mcp_discovery: failed to build ownership index: {}", e);
            OwnershipIndex {
                by_hash: HashMap::new(),
            }
        }
    };
    scan_all_with_home(&home, &index)
}

/// Test-friendly variant. Production should call `scan_all_host_mcps`.
pub(crate) fn scan_all_with_home(home: &Path, index: &OwnershipIndex) -> Vec<DiscoveredHostMcp> {
    let mut out = Vec::new();
    out.extend(scan_claude(home, index));
    out.extend(scan_gemini(home, index));
    out.extend(scan_codex(home, index));
    out.extend(scan_copilot(home, index));
    out
}

// ─── Claude Code: ~/.claude.json ─────────────────────────────────────────────

/// Scan both top-level `mcpServers` (scope user) AND `projects[<abs>].mcpServers`
/// (scope local). The file is monolithic (~40 KB on the author's machine)
/// and contains a lot of state we must not touch — we only read.
fn scan_claude(home: &Path, index: &OwnershipIndex) -> Vec<DiscoveredHostMcp> {
    let path = home.join(".claude.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("host_mcp_discovery: cannot parse {}: {}", path.display(), e);
            return Vec::new();
        }
    };
    let path_str = path.to_string_lossy().to_string();
    let mut out = Vec::new();

    // Top-level mcpServers (scope user)
    if let Some(servers) = value.get("mcpServers").and_then(|v| v.as_object()) {
        for (name, entry) in servers {
            if let Some(disc) =
                build_from_json_entry(&path_str, HostScope::ClaudeUser, name, entry, index)
            {
                out.push(disc);
            }
        }
    }

    // projects[<abs-path>].mcpServers (scope local)
    if let Some(projects) = value.get("projects").and_then(|v| v.as_object()) {
        for (project_path, project_obj) in projects {
            let servers = match project_obj.get("mcpServers").and_then(|v| v.as_object()) {
                Some(s) => s,
                None => continue,
            };
            for (name, entry) in servers {
                if let Some(disc) = build_from_json_entry(
                    &path_str,
                    HostScope::ClaudeLocal {
                        project_path: project_path.clone(),
                    },
                    name,
                    entry,
                    index,
                ) {
                    out.push(disc);
                }
            }
        }
    }

    out
}

// ─── Gemini CLI: ~/.gemini/settings.json ─────────────────────────────────────

fn scan_gemini(home: &Path, index: &OwnershipIndex) -> Vec<DiscoveredHostMcp> {
    let path = home.join(".gemini/settings.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("host_mcp_discovery: cannot parse {}: {}", path.display(), e);
            return Vec::new();
        }
    };
    let path_str = path.to_string_lossy().to_string();
    let mut out = Vec::new();
    if let Some(servers) = value.get("mcpServers").and_then(|v| v.as_object()) {
        for (name, entry) in servers {
            if let Some(disc) =
                build_from_json_entry(&path_str, HostScope::Gemini, name, entry, index)
            {
                out.push(disc);
            }
        }
    }
    out
}

// ─── Copilot CLI: ~/.copilot/mcp-config.json ─────────────────────────────────

fn scan_copilot(home: &Path, index: &OwnershipIndex) -> Vec<DiscoveredHostMcp> {
    let path = home.join(".copilot/mcp-config.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("host_mcp_discovery: cannot parse {}: {}", path.display(), e);
            return Vec::new();
        }
    };
    let path_str = path.to_string_lossy().to_string();
    let mut out = Vec::new();
    if let Some(servers) = value.get("mcpServers").and_then(|v| v.as_object()) {
        for (name, entry) in servers {
            if let Some(disc) =
                build_from_json_entry(&path_str, HostScope::Copilot, name, entry, index)
            {
                out.push(disc);
            }
        }
    }
    out
}

// ─── Codex CLI: ~/.codex/config.toml ─────────────────────────────────────────

fn scan_codex(home: &Path, index: &OwnershipIndex) -> Vec<DiscoveredHostMcp> {
    let path = home.join(".codex/config.toml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    // toml 1.x: parse into Table directly (parse::<toml::Value> no
    // longer accepts a full TOML document — it's only for primitive
    // values now).
    let value: toml::Table = match raw.parse() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("host_mcp_discovery: cannot parse {}: {}", path.display(), e);
            return Vec::new();
        }
    };
    let path_str = path.to_string_lossy().to_string();
    let mut out = Vec::new();
    let servers = match value.get("mcp_servers").and_then(|v| v.as_table()) {
        Some(t) => t,
        None => return Vec::new(),
    };
    for (name, entry) in servers {
        let table = match entry.as_table() {
            Some(t) => t,
            None => continue,
        };
        let command = match table.get("command").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => continue, // Codex requires command (stdio only)
        };
        let args: Vec<String> = table
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let env_map: HashMap<String, String> = table
            .get("env")
            .and_then(|v| v.as_table())
            .map(|t| {
                t.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let env_keys: Vec<String> = env_map.keys().cloned().collect();
        let marker_id = table
            .get("_kronn")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("config_id"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let transport = McpTransport::Stdio { command, args };
        let hash = compute_hash(&transport, &env_map);
        let ownership = index.classify(marker_id.as_deref(), &hash);

        out.push(DiscoveredHostMcp {
            source_file: path_str.clone(),
            scope: HostScope::Codex,
            name: name.clone(),
            transport,
            env_keys: sort_keys(env_keys),
            managed_by_kronn: ownership,
        });
    }
    out
}

// ─── JSON entry → DiscoveredHostMcp ──────────────────────────────────────────

/// Common path for Claude/Gemini/Copilot which all share the same JSON shape:
/// `{ "command": ..., "args": [...], "url": ..., "env": {...} }`.
fn build_from_json_entry(
    source_file: &str,
    scope: HostScope,
    name: &str,
    entry: &serde_json::Value,
    index: &OwnershipIndex,
) -> Option<DiscoveredHostMcp> {
    let entry_obj = entry.as_object()?;
    let command = entry_obj
        .get("command")
        .and_then(|v| v.as_str())
        .map(String::from);
    let url = entry_obj
        .get("url")
        .and_then(|v| v.as_str())
        .map(String::from);
    let http_url = entry_obj
        .get("httpUrl")
        .and_then(|v| v.as_str())
        .map(String::from);
    let entry_type = entry_obj.get("type").and_then(|v| v.as_str());
    let args: Vec<String> = entry_obj
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let env_map: HashMap<String, String> = entry_obj
        .get("env")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    // _kronn marker (future-proof — Phase 3 will write this)
    let marker_id = entry_obj
        .get("_kronn")
        .and_then(|v| v.as_object())
        .and_then(|m| m.get("config_id"))
        .and_then(|v| v.as_str())
        .map(String::from);

    let transport = if let Some(cmd) = command {
        McpTransport::Stdio { command: cmd, args }
    } else {
        let u = http_url.or_else(|| url.clone())?;
        // Gemini uses `httpUrl` for Streamable; Claude uses `type: "http"`.
        // We map to Streamable when `type` is `http` or `httpUrl` is set;
        // otherwise default to SSE (legacy default).
        if entry_type == Some("http") || entry_obj.contains_key("httpUrl") {
            McpTransport::Streamable { url: u }
        } else {
            McpTransport::Sse { url: u }
        }
    };

    let env_keys: Vec<String> = env_map.keys().cloned().collect();
    let hash = compute_hash(&transport, &env_map);
    let ownership = index.classify(marker_id.as_deref(), &hash);

    Some(DiscoveredHostMcp {
        source_file: source_file.to_string(),
        scope,
        name: name.to_string(),
        transport,
        env_keys: sort_keys(env_keys),
        managed_by_kronn: ownership,
    })
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Compute the same hash `db::mcps::compute_config_hash` would on the
/// equivalent McpConfig — we synthesize an ephemeral McpServer just for
/// the transport identity. `name`/`description`/`source` are irrelevant
/// to the hash function.
fn compute_hash(transport: &McpTransport, env: &HashMap<String, String>) -> String {
    let phantom = McpServer {
        id: String::new(),
        name: String::new(),
        description: String::new(),
        transport: transport.clone(),
        source: McpSource::Detected,
        api_spec: None,
    };
    db::mcps::compute_config_hash(&phantom, env, None)
}

fn sort_keys(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::collections::HashSet;
    use std::fs;
    use tempfile::TempDir;

    fn empty_index() -> OwnershipIndex {
        OwnershipIndex {
            by_hash: HashMap::new(),
        }
    }

    fn index_with_hash(hash: &str, config_id: &str) -> OwnershipIndex {
        let mut by_hash = HashMap::new();
        by_hash.insert(hash.to_string(), config_id.to_string());
        OwnershipIndex { by_hash }
    }

    #[test]
    fn empty_home_returns_no_results() {
        let tmp = TempDir::new().unwrap();
        let result = scan_all_with_home(tmp.path(), &empty_index());
        assert!(result.is_empty());
    }

    #[test]
    fn scan_claude_user_scope() {
        let tmp = TempDir::new().unwrap();
        let claude = r#"{
            "numStartups": 42,
            "theme": "dark",
            "mcpServers": {
                "linear": {
                    "command": "npx",
                    "args": ["-y", "@linear/mcp-server"],
                    "env": { "LINEAR_API_KEY": "lin_xxx" }
                }
            }
        }"#;
        fs::write(tmp.path().join(".claude.json"), claude).unwrap();

        let result = scan_claude(tmp.path(), &empty_index());
        assert_eq!(result.len(), 1);
        let entry = &result[0];
        assert_eq!(entry.name, "linear");
        assert_eq!(entry.scope, HostScope::ClaudeUser);
        assert_eq!(entry.env_keys, vec!["LINEAR_API_KEY"]);
        assert!(matches!(entry.transport, McpTransport::Stdio { .. }));
        assert_eq!(entry.managed_by_kronn, KronnOwnership::NotManaged);
    }

    #[test]
    fn scan_claude_local_scope_per_project() {
        let tmp = TempDir::new().unwrap();
        let claude = r#"{
            "projects": {
                "/home/me/repo-a": {
                    "mcpServers": {
                        "github": {
                            "command": "uvx",
                            "args": ["mcp-server-github"],
                            "env": { "GITHUB_TOKEN": "ghp_xxx" }
                        }
                    }
                },
                "/home/me/repo-b": {
                    "mcpServers": {
                        "fs": { "command": "node", "args": ["fs-server.js"] }
                    }
                }
            }
        }"#;
        fs::write(tmp.path().join(".claude.json"), claude).unwrap();

        let result = scan_claude(tmp.path(), &empty_index());
        assert_eq!(result.len(), 2);

        let scopes: HashSet<HostScope> = result.iter().map(|d| d.scope.clone()).collect();
        assert!(scopes.contains(&HostScope::ClaudeLocal {
            project_path: "/home/me/repo-a".into()
        }));
        assert!(scopes.contains(&HostScope::ClaudeLocal {
            project_path: "/home/me/repo-b".into()
        }));
    }

    #[test]
    fn scan_claude_both_scopes_simultaneously() {
        let tmp = TempDir::new().unwrap();
        let claude = r#"{
            "mcpServers": {
                "global-one": { "command": "npx", "args": ["pkg-a"] }
            },
            "projects": {
                "/p": {
                    "mcpServers": {
                        "scoped": { "command": "npx", "args": ["pkg-b"] }
                    }
                }
            }
        }"#;
        fs::write(tmp.path().join(".claude.json"), claude).unwrap();
        let result = scan_claude(tmp.path(), &empty_index());
        assert_eq!(result.len(), 2);
        assert!(result
            .iter()
            .any(|d| d.name == "global-one" && d.scope == HostScope::ClaudeUser));
        assert!(result.iter().any(
            |d| matches!(&d.scope, HostScope::ClaudeLocal { project_path } if project_path == "/p")
        ));
    }

    #[test]
    fn parse_failure_is_silent_no_panic() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(".claude.json"), "{not valid json").unwrap();
        let result = scan_claude(tmp.path(), &empty_index());
        assert!(result.is_empty()); // logs warn, returns empty
    }

    #[test]
    fn missing_files_return_empty() {
        let tmp = TempDir::new().unwrap();
        // No files written — all 4 should produce zero results
        let result = scan_all_with_home(tmp.path(), &empty_index());
        assert!(result.is_empty());
    }

    #[test]
    fn scan_gemini_with_streamable_http_url() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".gemini")).unwrap();
        let gemini = r#"{
            "apiKey": "ai_xxx",
            "mcpServers": {
                "remote": { "httpUrl": "https://example.com/mcp" }
            }
        }"#;
        fs::write(tmp.path().join(".gemini/settings.json"), gemini).unwrap();
        let result = scan_gemini(tmp.path(), &empty_index());
        assert_eq!(result.len(), 1);
        assert!(matches!(
            result[0].transport,
            McpTransport::Streamable { .. }
        ));
    }

    #[test]
    fn scan_gemini_with_sse_url() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".gemini")).unwrap();
        let gemini = r#"{
            "mcpServers": {
                "old-sse": { "url": "https://example.com/sse" }
            }
        }"#;
        fs::write(tmp.path().join(".gemini/settings.json"), gemini).unwrap();
        let result = scan_gemini(tmp.path(), &empty_index());
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].transport, McpTransport::Sse { .. }));
    }

    #[test]
    fn scan_codex_extracts_stdio() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        let codex = r#"
model = "gpt-4"
[mcp_servers.atlassian]
command = "uvx"
args = ["mcp-atlassian"]
startup_timeout_sec = 60
[mcp_servers.atlassian.env]
ATLASSIAN_TOKEN = "x"
"#;
        fs::write(tmp.path().join(".codex/config.toml"), codex).unwrap();
        let result = scan_codex(tmp.path(), &empty_index());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "atlassian");
        assert_eq!(result[0].env_keys, vec!["ATLASSIAN_TOKEN"]);
        assert_eq!(result[0].scope, HostScope::Codex);
    }

    #[test]
    fn scan_copilot_basic() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".copilot")).unwrap();
        let copilot = r#"{
            "mcpServers": {
                "linear": { "command": "npx", "args": ["pkg"], "env": { "K": "v" } }
            }
        }"#;
        fs::write(tmp.path().join(".copilot/mcp-config.json"), copilot).unwrap();
        let result = scan_copilot(tmp.path(), &empty_index());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].scope, HostScope::Copilot);
    }

    #[test]
    fn ownership_marker_wins_over_hash() {
        let tmp = TempDir::new().unwrap();
        let claude = r#"{
            "mcpServers": {
                "linear": {
                    "command": "npx",
                    "args": ["-y", "pkg"],
                    "_kronn": { "managed": true, "config_id": "uuid-from-marker" }
                }
            }
        }"#;
        fs::write(tmp.path().join(".claude.json"), claude).unwrap();

        // Build a phantom hash that would match — we want to verify the marker
        // wins even when both hash and marker resolve.
        let env = HashMap::new();
        let transport = McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "pkg".into()],
        };
        let hash = compute_hash(&transport, &env);
        let idx = index_with_hash(&hash, "uuid-from-hash");

        let result = scan_claude(tmp.path(), &idx);
        assert_eq!(result.len(), 1);
        match &result[0].managed_by_kronn {
            KronnOwnership::ManagedByMarker(id) => assert_eq!(id, "uuid-from-marker"),
            other => panic!("Expected ManagedByMarker, got {:?}", other),
        }
    }

    #[test]
    fn ownership_hash_match_when_no_marker() {
        let tmp = TempDir::new().unwrap();
        let claude = r#"{
            "mcpServers": {
                "linear": {
                    "command": "npx",
                    "args": ["-y", "pkg"]
                }
            }
        }"#;
        fs::write(tmp.path().join(".claude.json"), claude).unwrap();

        let env = HashMap::new();
        let transport = McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "pkg".into()],
        };
        let hash = compute_hash(&transport, &env);
        let idx = index_with_hash(&hash, "uuid-known");

        let result = scan_claude(tmp.path(), &idx);
        match &result[0].managed_by_kronn {
            KronnOwnership::ManagedByHash(id) => assert_eq!(id, "uuid-known"),
            other => panic!("Expected ManagedByHash, got {:?}", other),
        }
    }

    #[test]
    fn ownership_unknown_when_no_match() {
        let tmp = TempDir::new().unwrap();
        let claude = r#"{
            "mcpServers": {
                "rare": { "command": "weird", "args": ["one-off"] }
            }
        }"#;
        fs::write(tmp.path().join(".claude.json"), claude).unwrap();
        let result = scan_claude(tmp.path(), &empty_index());
        assert_eq!(result[0].managed_by_kronn, KronnOwnership::NotManaged);
    }

    #[test]
    fn env_values_never_appear_in_output() {
        let tmp = TempDir::new().unwrap();
        let claude = r#"{
            "mcpServers": {
                "leaky": {
                    "command": "npx",
                    "env": { "SECRET_TOKEN": "very-secret-do-not-expose" }
                }
            }
        }"#;
        fs::write(tmp.path().join(".claude.json"), claude).unwrap();
        let result = scan_claude(tmp.path(), &empty_index());
        let serialized = serde_json::to_string(&result).unwrap();
        assert!(
            !serialized.contains("very-secret-do-not-expose"),
            "env value leaked in serialized output: {}",
            serialized
        );
    }

    #[test]
    fn multi_cli_idempotent_scan() {
        // Run scan twice — same input must produce same output.
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".gemini")).unwrap();
        fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        fs::write(
            tmp.path().join(".claude.json"),
            r#"{"mcpServers":{"a":{"command":"x"}}}"#,
        )
        .unwrap();
        fs::write(
            tmp.path().join(".gemini/settings.json"),
            r#"{"mcpServers":{"b":{"command":"y"}}}"#,
        )
        .unwrap();
        fs::write(
            tmp.path().join(".codex/config.toml"),
            "[mcp_servers.c]\ncommand = \"z\"\n",
        )
        .unwrap();

        let r1 = scan_all_with_home(tmp.path(), &empty_index());
        let r2 = scan_all_with_home(tmp.path(), &empty_index());
        assert_eq!(r1.len(), 3);
        assert_eq!(r1.len(), r2.len());
        let names1: HashSet<&str> = r1.iter().map(|d| d.name.as_str()).collect();
        let names2: HashSet<&str> = r2.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names1, names2);
    }

    #[test]
    #[serial]
    fn entry_without_transport_is_skipped() {
        let tmp = TempDir::new().unwrap();
        let claude = r#"{
            "mcpServers": {
                "broken": { "env": { "K": "v" } }
            }
        }"#;
        fs::write(tmp.path().join(".claude.json"), claude).unwrap();
        let result = scan_claude(tmp.path(), &empty_index());
        assert!(result.is_empty());
    }

    /// Env-mutating test — single-threaded by serial_test convention is
    /// not used here because Kronn doesn't pull `serial_test` as a dep.
    /// Risk is bounded: only KRONN_HOST_HOME is mutated and other tests
    /// don't read it. If flake surfaces, gate behind `--test-threads=1`.
    #[test]
    #[serial]
    fn resolve_home_prefers_kronn_host_home() {
        let original = std::env::var("KRONN_HOST_HOME").ok();
        std::env::set_var("KRONN_HOST_HOME", "/host/path");
        let resolved = resolve_home();
        assert_eq!(resolved, Some(PathBuf::from("/host/path")));
        match original {
            Some(v) => std::env::set_var("KRONN_HOST_HOME", v),
            None => std::env::remove_var("KRONN_HOST_HOME"),
        }
    }
}
