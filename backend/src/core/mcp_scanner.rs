use std::collections::HashMap;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

use crate::models::McpTransport;

// ─── Codex config.toml format ────────────────────────────────────────────────

/// Codex config.toml — only the mcp_servers section.
/// We preserve everything else by doing a partial read/write.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CodexMcpEntry {
    command: String,
    args: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    env: HashMap<String, String>,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    enabled: bool,
    /// Codex default is 10s which is too short when many MCPs start in parallel.
    /// Always written explicitly so Codex reads it.
    #[serde(default = "default_startup_timeout")]
    startup_timeout_sec: u32,
}

pub(crate) fn default_startup_timeout() -> u32 { 30 }

fn default_true() -> bool { true }
fn is_true(v: &bool) -> bool { *v }

// ─── Vibe config.toml format ─────────────────────────────────────────────────

/// Vibe config.toml `[[mcp_servers]]` entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct VibeMcpEntry {
    pub(crate) name: String,
    pub(crate) transport: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) env: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct VibeConfig {
    #[serde(default)]
    pub(crate) mcp_servers: Vec<VibeMcpEntry>,
}

// ─── .mcp.json file format (Claude Code) ─────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpJsonFile {
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: HashMap<String, McpServerEntry>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct McpServerEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
}

/// Custom Debug impl that masks env values (may contain secrets like API keys).
impl std::fmt::Debug for McpServerEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerEntry")
            .field("command", &self.command)
            .field("args", &self.args)
            .field("url", &self.url)
            .field("env", &format!("[{} keys]", self.env.len()))
            .finish()
    }
}

// ─── Read .mcp.json ──────────────────────────────────────────────────────────

/// Read and parse the raw .mcp.json for a project path.
/// Returns the full file including secrets — NOT for API responses.
pub fn read_mcp_json(project_path: &str) -> Option<McpJsonFile> {
    let resolved = resolve_host_path(project_path);
    let file = Path::new(&resolved).join(".mcp.json");
    let content = std::fs::read_to_string(&file).ok()?;
    serde_json::from_str(&content).ok()
}

/// Write a McpJsonFile to the project's .mcp.json.
pub fn write_mcp_json(project_path: &str, data: &McpJsonFile) -> Result<(), String> {
    write_mcp_json_to_subpath(project_path, ".mcp.json", data)
}

/// Write a McpJsonFile to an arbitrary subpath within a project directory.
/// Creates parent directories if needed. Used for Claude (.mcp.json),
/// Kiro (.kiro/settings/mcp.json + .ai/mcp/mcp.json), and Gemini (.gemini/settings.json).
pub fn write_mcp_json_to_subpath(project_path: &str, subpath: &str, data: &McpJsonFile) -> Result<(), String> {
    let resolved = resolve_host_path(project_path);
    let file = Path::new(&resolved).join(subpath);
    // Create parent directories (e.g., .kiro/settings/)
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create dir {}: {}", parent.display(), e))?;
    }
    let content = serde_json::to_string_pretty(data)
        .map_err(|e| format!("JSON serialize error: {}", e))?;
    // Atomic write: write to temp file then rename, so agents never read partial JSON
    atomic_write(&file, &content)
}

/// Write content to a file atomically: write to a temp sibling then rename.
/// This prevents agents from reading a partially-written config file.
pub(crate) fn atomic_write(target: &Path, content: &str) -> Result<(), String> {
    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, content)
        .map_err(|e| format!("Failed to write temp {}: {}", tmp.display(), e))?;
    std::fs::rename(&tmp, target)
        .map_err(|e| {
            // Clean up temp file on rename failure
            let _ = std::fs::remove_file(&tmp);
            format!("Failed to rename {} → {}: {}", tmp.display(), target.display(), e)
        })
}

/// Ensure Claude Code's settings.local.json has all MCP server names in enabledMcpjsonServers.
/// Claude Code uses this list as a whitelist — MCPs not listed are silently ignored,
/// even when enableAllProjectMcpServers is true (known bug #24657).
/// This function only ADDS missing entries, never removes user-configured ones.
/// Sync `enabledMcpjsonServers` in `.claude/settings.local.json` to match
/// the current `.mcp.json` keys exactly. This fixes the naming migration
/// issue (TD-20260403-mcp-naming-migration) where old keys (`server.name`)
/// stayed in the whitelist after we switched to `config.label` as the key.
///
/// Strategy: REPLACE the whitelist with exactly the current `.mcp.json` keys.
/// Old stale entries (from renamed MCPs, deleted configs, etc.) are removed.
/// Claude Code only loads MCPs that are BOTH in `.mcp.json` AND whitelisted,
/// so the whitelist must be a superset of `.mcp.json` keys.
pub(crate) fn sync_claude_enabled_servers(project_path: &str, mcp_servers: &HashMap<String, McpServerEntry>) {
    let resolved = resolve_host_path(project_path);
    let settings_dir = Path::new(&resolved).join(".claude");
    let settings_file = settings_dir.join("settings.local.json");

    if !settings_file.exists() {
        return; // No settings.local.json → Claude Code loads all MCPs by default
    }

    let content = match std::fs::read_to_string(&settings_file) {
        Ok(c) => c,
        Err(_) => return,
    };

    let mut settings: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Only act if enabledMcpjsonServers exists (don't create it if absent)
    if settings.get("enabledMcpjsonServers").and_then(|v| v.as_array()).is_none() {
        return;
    }

    // Build the new whitelist: exactly the keys from the current .mcp.json.
    // This removes stale entries from renamed/deleted MCPs and adds new ones.
    let new_enabled: Vec<serde_json::Value> = mcp_servers.keys()
        .map(|k| serde_json::Value::String(k.clone()))
        .collect();

    let old_enabled: Vec<String> = settings["enabledMcpjsonServers"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    settings["enabledMcpjsonServers"] = serde_json::Value::Array(new_enabled);

    // Log what changed for debugging
    let new_set: std::collections::HashSet<&str> = mcp_servers.keys().map(|s| s.as_str()).collect();
    let old_set: std::collections::HashSet<&str> = old_enabled.iter().map(|s| s.as_str()).collect();
    let added: Vec<&&str> = new_set.difference(&old_set).collect();
    let removed: Vec<&&str> = old_set.difference(&new_set).collect();

    if added.is_empty() && removed.is_empty() {
        return; // No changes needed
    }

    if let Ok(json) = serde_json::to_string_pretty(&settings) {
        let _ = atomic_write(&settings_file, &json);
        if !added.is_empty() {
            tracing::info!("Claude enabledMcpjsonServers: added {:?}", added);
        }
        if !removed.is_empty() {
            tracing::info!("Claude enabledMcpjsonServers: removed stale {:?}", removed);
        }
    }
}

/// Write a .mcp.json to `target_dir` with all MCP configs that have `include_general` set.
/// Used for general discussions (no project) so agents still have access to global MCPs.
pub fn write_general_mcp_json(
    conn: &rusqlite::Connection,
    secret: &str,
    target_dir: &str,
) -> Result<(), String> {
    use crate::db;

    let configs = db::mcps::list_configs(conn).map_err(|e| e.to_string())?;
    let general_configs: Vec<_> = configs.into_iter().filter(|c| c.include_general).collect();
    if general_configs.is_empty() { return Ok(()); }

    let servers = db::mcps::list_servers(conn).map_err(|e| e.to_string())?;
    let server_map: HashMap<String, &crate::models::McpServer> = servers.iter()
        .map(|s| (s.id.clone(), s)).collect();

    let mut mcp_servers = HashMap::new();
    for config in &general_configs {
        let server = match server_map.get(&config.server_id) {
            Some(s) => s,
            None => continue,
        };
        let env = db::mcps::decrypt_env(&config.env_encrypted, secret).unwrap_or_default();

        let entry = match &server.transport {
            McpTransport::Stdio { command, args } => {
                if !is_command_available(command) { continue; }
                let final_args = config.args_override.clone().unwrap_or_else(|| args.clone());
                McpServerEntry { command: Some(command.clone()), args: Some(final_args), url: None, env }
            }
            McpTransport::Sse { url } | McpTransport::Streamable { url } => {
                McpServerEntry { command: None, args: None, url: Some(url.clone()), env }
            }
            // API-only plugins never get written to .mcp.json — their
            // capability is surfaced to the agent via prompt injection in
            // `build_api_context_block` (see api_context.rs). Skip silently.
            McpTransport::ApiOnly => continue,
        };
        let key = config.label.clone();
        mcp_servers.insert(key, entry);
    }

    if !mcp_servers.is_empty() {
        // ── Claude Code: .mcp.json (stdio only) ──
        let stdio_only: HashMap<String, McpServerEntry> = mcp_servers.iter()
            .filter(|(_, e)| e.command.is_some())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !stdio_only.is_empty() {
            let data = McpJsonFile { mcp_servers: stdio_only };
            write_mcp_json(target_dir, &data)?;
        }

        // ── Kiro: .kiro/settings/mcp.json + .ai/mcp/mcp.json (filter incompatible) ──
        let kiro_servers: HashMap<String, McpServerEntry> = mcp_servers.iter()
            .filter(|(key, _)| {
                !general_configs.iter().any(|cfg| {
                    if let Some(srv) = server_map.get(&cfg.server_id) {
                            cfg.label.as_str() == key.as_str() && check_incompatibility(srv, &AgentType::Kiro).is_some()
                    } else { false }
                })
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let kiro_data = McpJsonFile { mcp_servers: kiro_servers };
        let _ = write_mcp_json_to_subpath(target_dir, ".kiro/settings/mcp.json", &kiro_data);
        let _ = write_mcp_json_to_subpath(target_dir, ".ai/mcp/mcp.json", &kiro_data);

        // ── Gemini: .gemini/settings.json (full, no localhost filter for desktop) ──
        let full_data = McpJsonFile { mcp_servers: mcp_servers.clone() };
        let _ = write_mcp_json_to_subpath(target_dir, ".gemini/settings.json", &full_data);

        // ── Vibe: .vibe/config.toml ──
        let server_map_owned: HashMap<String, &crate::models::McpServer> = server_map;
        sync_vibe_project_config(target_dir, &general_configs, &server_map_owned, secret);
    }
    Ok(())
}

// ─── Sync DB → disk ──────────────────────────────────────────────────────

/// Rebuild a project's .mcp.json from the DB state.
/// Collects all applicable MCP configs (direct + global), decrypts env,
/// and writes the result to disk.
pub fn sync_project_mcps_to_disk(
    conn: &rusqlite::Connection,
    project_id: &str,
    secret: &str,
) -> Result<(), String> {
    use crate::db;

    // Get project path
    let project = db::projects::list_projects(conn)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|p| p.id == project_id)
        .ok_or_else(|| format!("Project {} not found", project_id))?;

    // Get all configs for this project (direct + global)
    let configs = db::mcps::configs_for_project(conn, project_id)
        .map_err(|e| e.to_string())?;

    // Get all servers
    let servers = db::mcps::list_servers(conn)
        .map_err(|e| e.to_string())?;
    let server_map: HashMap<String, &crate::models::McpServer> = servers.iter()
        .map(|s| (s.id.clone(), s))
        .collect();

    // Count configs per server to decide naming strategy:
    // - Single config for a server → use server.name (clean technical name)
    // - Multiple configs for same server → use config.label (to differentiate)
    // Build the McpJsonFile
    let mut mcp_servers = HashMap::new();
    let mut synced_config_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for config in &configs {
        let server = match server_map.get(&config.server_id) {
            Some(s) => s,
            None => continue,
        };

        // Decrypt env — skip MCP if decryption fails and keys are expected
        let env = match db::mcps::decrypt_env(&config.env_encrypted, secret) {
            Ok(e) => e,
            Err(e) => {
                if !config.env_keys.is_empty() {
                    tracing::warn!(
                        "MCP '{}' has {} env keys but decryption failed ({}) — writing without secrets",
                        config.label, config.env_keys.len(), e
                    );
                }
                HashMap::new()
            }
        };

        let entry = match &server.transport {
            McpTransport::Stdio { command, args } => {
                // Validate that the MCP command is available in PATH
                if !is_command_available(command) {
                    tracing::warn!(
                        "MCP server '{}' command '{}' not found in PATH — skipping",
                        server.name, command
                    );
                    continue;
                }
                let final_args = config.args_override.clone()
                    .unwrap_or_else(|| args.clone());
                McpServerEntry {
                    command: Some(command.clone()),
                    args: Some(final_args),
                    url: None,
                    env,
                }
            }
            McpTransport::Sse { url } | McpTransport::Streamable { url } => {
                McpServerEntry {
                    command: None,
                    args: None,
                    url: Some(url.clone()),
                    env,
                }
            }
            // API-only plugins are not written to `.mcp.json` — their
            // capability is surfaced to agents via prompt injection. Skip
            // silently so the rest of the sync (other MCPs on the same
            // project) continues normally.
            McpTransport::ApiOnly => continue,
        };

        // Always use config label as key — avoids case mismatch between
        // server.name (e.g. "Fastly") and lowercased variants ("fastly")
        let key = config.label.clone();
        mcp_servers.insert(key, entry);
        synced_config_ids.insert(config.id.clone());
    }

    if mcp_servers.is_empty() {
        // Remove config files if no MCPs
        let resolved = resolve_host_path(&project.path);
        for filename in &[".mcp.json", ".vibe/config.toml", ".kiro/settings/mcp.json", ".gemini/settings.json", ".ai/mcp/mcp.json"] {
            let file = std::path::Path::new(&resolved).join(filename);
            if file.exists() {
                let _ = std::fs::remove_file(&file);
                tracing::info!("Removed {} from {} (no MCPs)", filename, project.path);
            }
        }
    } else {
        // ── Claude Code: .mcp.json ──
        // Claude Code only supports stdio servers in .mcp.json.
        // SSE/Streamable entries (with only "url", no "command") break the schema
        // validation and cause Claude Code to reject the ENTIRE file → no MCPs at all.
        let stdio_only: HashMap<String, McpServerEntry> = mcp_servers.iter()
            .filter(|(_, entry)| entry.command.is_some())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let claude_data = McpJsonFile { mcp_servers: stdio_only };
        write_mcp_json(&project.path, &claude_data)?;
        ensure_gitignore(&project.path, ".mcp.json");
        tracing::info!("Synced .mcp.json for {} ({} stdio MCPs)", project.path, claude_data.mcp_servers.len());

        // ── Claude Code settings.local.json: keep enabledMcpjsonServers in sync ──
        sync_claude_enabled_servers(&project.path, &claude_data.mcp_servers);

        // Full data — but filter out localhost SSE/Streamable (unreachable in Docker)
        let docker_safe: HashMap<String, McpServerEntry> = mcp_servers.into_iter()
            .filter(|(_, entry)| {
                if let Some(ref url) = entry.url {
                    !(url.contains("localhost") || url.contains("127.0.0.1") || url.contains("[::1]"))
                } else {
                    true
                }
            })
            .collect();
        let data = McpJsonFile { mcp_servers: docker_safe };

        // ── Vibe: .vibe/config.toml ──
        sync_vibe_project_config(&project.path, &configs, &server_map, secret);

        // ── Kiro: filter out incompatible servers ──
        let kiro_servers: HashMap<String, McpServerEntry> = {
            let mut filtered = data.mcp_servers.clone();
            let to_remove: Vec<String> = filtered.keys()
                .filter(|key| {
                    // Find matching server in server_map by checking config labels/names
                    configs.iter().any(|cfg| {
                        if let Some(srv) = server_map.get(&cfg.server_id) {
                            cfg.label.as_str() == key.as_str() && check_incompatibility(srv, &AgentType::Kiro).is_some()
                        } else {
                            false
                        }
                    })
                })
                .cloned()
                .collect();
            for key in &to_remove {
                tracing::info!("Excluding '{}' from Kiro config (incompatible)", key);
                filtered.remove(key);
            }
            filtered
        };
        let kiro_data = McpJsonFile { mcp_servers: kiro_servers };

        // ── Kiro: .kiro/settings/mcp.json ──
        if let Err(e) = write_mcp_json_to_subpath(&project.path, ".kiro/settings/mcp.json", &kiro_data) {
            tracing::warn!("Failed to sync Kiro MCP config: {}", e);
        } else {
            ensure_gitignore(&project.path, ".kiro/settings/");
            tracing::info!("Synced .kiro/settings/mcp.json for {} ({} servers, {} excluded)",
                project.path, kiro_data.mcp_servers.len(),
                data.mcp_servers.len() - kiro_data.mcp_servers.len());
        }

        // ── Gemini CLI: .gemini/settings.json (same JSON format as Claude) ──
        if let Err(e) = write_mcp_json_to_subpath(&project.path, ".gemini/settings.json", &data) {
            tracing::warn!("Failed to sync Gemini MCP config: {}", e);
        } else {
            ensure_gitignore(&project.path, ".gemini/");
            tracing::info!("Synced .gemini/settings.json for {}", project.path);
        }

        // ── Kiro (new format): .ai/mcp/mcp.json ──
        if let Err(e) = write_mcp_json_to_subpath(&project.path, ".ai/mcp/mcp.json", &kiro_data) {
            tracing::warn!("Failed to sync Kiro .ai/mcp config: {}", e);
        } else {
            ensure_gitignore(&project.path, ".ai/mcp/");
            tracing::info!("Synced .ai/mcp/mcp.json for {}", project.path);
        }

        // Auto-create MCP context files from registry defaults (if available).
        // Writes when: (1) the config was synced (command available) OR the
        // plugin is API-only (nothing to sync to .mcp.json — its capability
        // surfaces via prompt injection instead); AND (2) the registry
        // provides a default_context; AND (3) no file exists yet.
        // This prevents creating context for a registry server (e.g. Go
        // binary) when the actually synced server is a different variant
        // (e.g. npm package), while still seeding API plugins like
        // Chartbeat whose context is equally valuable to the agent.
        {
            let registry = crate::core::registry::builtin_registry();
            let servers_for_kind = crate::db::mcps::list_servers(conn).unwrap_or_default();
            for config in &configs {
                let is_api_only = servers_for_kind.iter()
                    .find(|s| s.id == config.server_id)
                    .map(|s| matches!(s.transport, crate::models::McpTransport::ApiOnly))
                    .unwrap_or(false);
                if !synced_config_ids.contains(&config.id) && !is_api_only {
                    continue;
                }
                let slug = slugify_label(&config.label);
                if read_mcp_context(&project.path, &slug).is_none() {
                    if let Some(def) = registry.iter().find(|d| d.id == config.server_id) {
                        if let Some(ref ctx) = def.default_context {
                            let _ = write_mcp_context(&project.path, &slug, ctx);
                            tracing::info!("Created default MCP context for '{}' in {}", config.label, project.path);
                        }
                    }
                }
            }
        }
    }

    // ── Native skill & profile files (SKILL.md, agent files) ──
    // Full sync with cleanup: removes stale files from deselected skills/profiles.
    // Safe here because this runs at startup / project config change, not per-discussion.
    {
        let profile_ids: Vec<String> = project.default_profile_id.iter().cloned().collect();
        if let Err(e) = crate::core::native_files::sync_project_native_files_full(
            &project.path, &project.default_skill_ids, &profile_ids,
        ) {
            tracing::warn!("Failed to sync native files for {}: {}", project.path, e);
        }
    }

    // ── Ensure redirector files exist (auto-update for projects with ai/) ──
    ensure_redirectors(&project.path);

    Ok(())
}

/// Public wrapper for tests.
pub fn ensure_redirectors_public(project_path: &str) {
    ensure_redirectors(project_path);
}

/// Ensure all agent redirector files exist in a project that has an ai/ directory.
/// Non-destructive: only creates missing files, never overwrites existing ones.
/// Called during MCP sync to keep redirectors up-to-date when Kronn adds new agent support.
fn ensure_redirectors(project_path: &str) {
    let resolved = resolve_host_path(project_path);
    let project_dir = Path::new(&resolved);

    // Only for projects that have an ai/ directory
    if !project_dir.join("ai").is_dir() {
        return;
    }

    let template_dir = std::env::var("KRONN_TEMPLATES_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("templates"));

    if !template_dir.is_dir() {
        return;
    }

    // Simple redirector files (flat)
    let redirectors = [
        "CLAUDE.md", "GEMINI.md", "AGENTS.md",
        ".cursorrules", ".windsurfrules", ".clinerules",
    ];

    for filename in &redirectors {
        let src = template_dir.join(filename);
        let dst = project_dir.join(filename);
        if src.exists() && !dst.exists() {
            let _ = std::fs::copy(&src, &dst);
        }
    }

    // Nested redirectors (need parent dir creation)
    let nested = [
        ".github/copilot-instructions.md",
        ".kiro/steering/instructions.md",
    ];

    for subpath in &nested {
        let src = template_dir.join(subpath);
        let dst = project_dir.join(subpath);
        if src.exists() && !dst.exists() {
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(&src, &dst);
        }
    }
}

// ─── Vibe per-project sync ────────────────────────────────────────────────────

/// Write .vibe/config.toml for a project with its MCP servers.
fn sync_vibe_project_config(
    project_path: &str,
    configs: &[crate::models::McpConfig],
    server_map: &HashMap<String, &crate::models::McpServer>,
    secret: &str,
) {
    let mut entries = Vec::new();

    for config in configs {
        let server = match server_map.get(&config.server_id) {
            Some(s) => s,
            None => continue,
        };

        let env = crate::db::mcps::decrypt_env(&config.env_encrypted, secret)
            .unwrap_or_default();

        let name = config.label.clone();

        let entry = match &server.transport {
            McpTransport::Stdio { command, args } => {
                let final_args = config.args_override.clone()
                    .unwrap_or_else(|| args.clone());
                VibeMcpEntry {
                    name,
                    transport: "stdio".into(),
                    command: Some(command.clone()),
                    args: Some(final_args),
                    url: None,
                    env,
                }
            }
            McpTransport::Sse { url } => VibeMcpEntry {
                name,
                transport: "http".into(),
                command: None,
                args: None,
                url: Some(url.clone()),
                env,
            },
            McpTransport::Streamable { url } => VibeMcpEntry {
                name,
                transport: "streamable-http".into(),
                command: None,
                args: None,
                url: Some(url.clone()),
                env,
            },
            // API-only plugins don't appear in Vibe's MCP config — they're
            // surfaced via agent prompt injection. Skip silently.
            McpTransport::ApiOnly => continue,
        };
        entries.push(entry);
    }

    let resolved = resolve_host_path(project_path);
    let vibe_dir = Path::new(&resolved).join(".vibe");
    let vibe_config = vibe_dir.join("config.toml");

    if entries.is_empty() {
        if vibe_config.exists() {
            let _ = std::fs::remove_file(&vibe_config);
        }
        return;
    }

    if let Err(e) = std::fs::create_dir_all(&vibe_dir) {
        tracing::warn!("Failed to create .vibe dir at {}: {}", vibe_dir.display(), e);
        return;
    }

    let vibe_cfg = VibeConfig { mcp_servers: entries };
    match toml::to_string_pretty(&vibe_cfg) {
        Ok(content) => {
            let header = "# Vibe MCP config — auto-generated by Kronn\n# Do not edit manually; changes will be overwritten on next sync.\n\n";
            if let Err(e) = atomic_write(&vibe_config, &format!("{}{}", header, content)) {
                tracing::warn!("Failed to write Vibe config {}: {}", vibe_config.display(), e);
            } else {
                ensure_gitignore(project_path, ".vibe/");
                tracing::info!("Synced .vibe/config.toml for {}", project_path);
            }
        }
        Err(e) => tracing::warn!("Failed to serialize Vibe config: {}", e),
    }
}

// ─── Codex global sync ───────────────────────────────────────────────────────

/// Warn once if a host-native Codex config also exists in a different
/// directory than the one Kronn is about to write to.
///
/// Kronn syncs MCPs into a single `~/.codex/config.toml`. When run from
/// Docker / WSL the path is derived from `KRONN_HOST_HOME` or `HOME` inside
/// the container, while a host-native Codex install lives at `$USERPROFILE`
/// (Windows) or `$HOME` (Unix native). If the user installs Codex on the host
/// AND uses Kronn from Docker/WSL, the two configs diverge silently.
fn detect_codex_config_drift(active_dir: &Path) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    if WARNED.load(Ordering::Relaxed) {
        return;
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(host_home) = std::env::var("KRONN_HOST_HOME") {
        candidates.push(PathBuf::from(format!("{}/.codex", host_home)));
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(format!("{}/.codex", home)));
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        candidates.push(PathBuf::from(format!("{}/.codex", profile)));
    }

    let active_canon = active_dir.canonicalize().ok();
    for cand in &candidates {
        if !cand.join("config.toml").exists() {
            continue;
        }
        let cand_canon = cand.canonicalize().ok();
        let same = match (&active_canon, &cand_canon) {
            (Some(a), Some(b)) => a == b,
            _ => active_dir == cand,
        };
        if !same {
            tracing::warn!(
                "Codex config drift detected: Kronn writes to {} but another \
                 config also exists at {}. The two will diverge — pick a single \
                 source of truth (either run Codex inside Kronn's environment \
                 or set KRONN_HOST_HOME to point at the host install).",
                active_dir.display(),
                cand.display()
            );
            WARNED.store(true, Ordering::Relaxed);
            return;
        }
    }
}

/// Outcome of attempting to load an existing Codex `config.toml` for merge.
///
/// `Loaded(table)` — file parsed cleanly, returns the table to merge into.
/// `Empty`         — file does not exist or could not be read; start fresh.
/// `Aborted`       — file exists but is corrupt/non-table; the caller MUST
///                   abandon the sync without writing to avoid clobbering
///                   the user's existing provider config.
#[derive(Debug)]
pub(crate) enum CodexLoadOutcome {
    Loaded(toml::value::Table),
    Empty,
    Aborted,
}

/// Read an existing Codex `config.toml`, returning a parsed table or signalling
/// that the caller must abort. On parse failure we copy the file to
/// `config.toml.kronn-backup` so the user can recover their original.
///
/// Extracted from `sync_codex_global_config` so the data-preservation logic
/// is unit-testable in isolation.
pub(crate) fn load_codex_config_for_merge(codex_config: &Path) -> CodexLoadOutcome {
    if !codex_config.exists() {
        return CodexLoadOutcome::Empty;
    }
    let content = match std::fs::read_to_string(codex_config) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "Cannot read Codex config {}: {} — starting fresh",
                codex_config.display(),
                e
            );
            return CodexLoadOutcome::Empty;
        }
    };
    match content.parse::<toml::Value>() {
        Ok(v) => match v.as_table() {
            Some(t) => CodexLoadOutcome::Loaded(t.clone()),
            None => {
                tracing::error!(
                    "Codex config {} is not a TOML table — aborting sync to avoid data loss",
                    codex_config.display()
                );
                CodexLoadOutcome::Aborted
            }
        },
        Err(e) => {
            let backup = codex_config.with_extension("toml.kronn-backup");
            if let Err(copy_err) = std::fs::copy(codex_config, &backup) {
                tracing::error!(
                    "Failed to back up corrupt Codex config {} to {}: {}",
                    codex_config.display(),
                    backup.display(),
                    copy_err
                );
            }
            tracing::error!(
                "Failed to parse Codex config {} ({}). Backed up to {} and aborting sync to preserve user data.",
                codex_config.display(),
                e,
                backup.display()
            );
            CodexLoadOutcome::Aborted
        }
    }
}

/// Rebuild ~/.codex/config.toml with ALL configured MCP servers.
/// Codex uses a single global config file — no per-project support.
/// We merge MCP entries into the existing config, preserving non-MCP settings.
fn sync_codex_global_config(
    conn: &rusqlite::Connection,
    secret: &str,
) {
    use crate::db;

    // Gather ALL configs across all projects
    let all_configs = match db::mcps::list_configs(conn) {
        Ok(c) => c,
        Err(e) => { tracing::warn!("Failed to list configs for Codex sync: {}", e); return; }
    };
    let servers = match db::mcps::list_servers(conn) {
        Ok(s) => s,
        Err(e) => { tracing::warn!("Failed to list servers for Codex sync: {}", e); return; }
    };
    let server_map: HashMap<String, &crate::models::McpServer> = servers.iter()
        .map(|s| (s.id.clone(), s))
        .collect();

    // Build MCP entries (Codex only supports stdio transport)
    let mut mcp_entries: HashMap<String, CodexMcpEntry> = HashMap::new();
    for config in &all_configs {
        let server = match server_map.get(&config.server_id) {
            Some(s) => s,
            None => continue,
        };

        // Codex only supports stdio transport
        let (command, args) = match &server.transport {
            McpTransport::Stdio { command, args } => {
                let final_args = config.args_override.clone()
                    .unwrap_or_else(|| args.clone());
                (command.clone(), final_args)
            }
            _ => {
                tracing::debug!("Skipping non-stdio MCP '{}' for Codex (unsupported)", server.name);
                continue;
            }
        };

        let env = db::mcps::decrypt_env(&config.env_encrypted, secret)
            .unwrap_or_default();

        // Codex requires names matching ^[a-zA-Z0-9_-]+$ — slugify
        let raw_key = config.label.clone();
        let key = slugify_label(&raw_key);

        // npx/uvx MCPs need longer timeout for initial package download (cold start)
        let timeout = if command == "npx" || command == "uvx" { 60 } else { 30 };

        mcp_entries.insert(key, CodexMcpEntry {
            command,
            args,
            env,
            enabled: true,
            startup_timeout_sec: timeout,
        });
    }

    // Read existing config.toml and preserve non-MCP settings.
    // Inside Docker the host home is mounted at /root, but we use KRONN_HOST_HOME
    // to support native Linux/macOS execution where /root is not the user's home.
    let codex_dir = if let Ok(host_home) = std::env::var("KRONN_HOST_HOME") {
        PathBuf::from(format!("{}/.codex", host_home))
    } else {
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map(|h| PathBuf::from(format!("{}/.codex", h)))
            .unwrap_or_else(|_| directories::BaseDirs::new()
                .map(|d| d.home_dir().join(".codex"))
                .unwrap_or_else(|| {
                    tracing::warn!("Cannot determine home directory for Codex config — using /tmp/.codex");
                    PathBuf::from("/tmp/.codex")
                }))
    };
    let codex_config = codex_dir.join("config.toml");

    // Drift detection: if both KRONN_HOST_HOME/.codex and the local user's
    // ~/.codex exist *and* point at different filesystems, the user is likely
    // alternating between Kronn (Docker / WSL) and a host-native Codex install.
    // The two configs will silently diverge as Kronn syncs only one of them.
    // We can't pick a winner safely — just warn once so the user knows to
    // pick a single source of truth.
    detect_codex_config_drift(&codex_dir);

    // Parse existing config as a TOML table to preserve other settings
    // (e.g. [model_providers], [profiles], custom user keys).
    //
    // SAFETY: data preservation is delegated to load_codex_config_for_merge,
    // which backs up corrupt files before signalling Aborted so we never
    // silently overwrite the user's provider config.
    let mut doc: toml::value::Table = match load_codex_config_for_merge(&codex_config) {
        CodexLoadOutcome::Loaded(t) => t,
        CodexLoadOutcome::Empty => toml::value::Table::new(),
        CodexLoadOutcome::Aborted => return,
    };

    // Replace mcp_servers section
    if mcp_entries.is_empty() {
        doc.remove("mcp_servers");
    } else {
        let mut mcp_table = toml::value::Table::new();
        for (key, entry) in &mcp_entries {
            match toml::Value::try_from(entry) {
                Ok(v) => { mcp_table.insert(key.clone(), v); }
                Err(e) => tracing::warn!("Failed to serialize Codex MCP entry '{}': {}", key, e),
            }
        }
        doc.insert("mcp_servers".into(), toml::Value::Table(mcp_table));
    }

    // Write back. create_dir_all can fail for surprising reasons on real
    // user machines: ~/.codex moved to iCloud Drive (read-only sync), FileVault
    // not yet unlocked, parent owned by another user, or a Linux mount point
    // re-mounted read-only. Surface enough context that the user can act on
    // the message instead of staring at "Failed to create Codex config dir".
    if let Err(e) = std::fs::create_dir_all(&codex_dir) {
        tracing::error!(
            "Codex MCPs will NOT be synced — cannot create config dir {}: {} \
             (kind: {:?}). Common causes: ~/.codex moved to iCloud Drive, FileVault locked, \
             parent dir owned by another user, or read-only filesystem.",
            codex_dir.display(),
            e,
            e.kind()
        );
        return;
    }

    match toml::to_string_pretty(&doc) {
        Ok(content) => {
            if let Err(e) = atomic_write(&codex_config, &content) {
                tracing::error!(
                    "Failed to write Codex config to {}: {} — \
                     check disk space and that the path is writable.",
                    codex_config.display(), e
                );
            } else {
                tracing::info!("Synced Codex global config ({} MCP servers)", mcp_entries.len());
            }
        }
        Err(e) => tracing::warn!("Failed to serialize Codex config: {}", e),
    }
}

/// Sync ~/.copilot/mcp-config.json — global config, same JSON format as Claude (.mcp.json).
fn sync_copilot_global_config(
    conn: &rusqlite::Connection,
    secret: &str,
) {
    use crate::db;

    let all_configs = match db::mcps::list_configs(conn) {
        Ok(c) => c,
        Err(e) => { tracing::warn!("Failed to list configs for Copilot sync: {}", e); return; }
    };
    let servers = match db::mcps::list_servers(conn) {
        Ok(s) => s,
        Err(e) => { tracing::warn!("Failed to list servers for Copilot sync: {}", e); return; }
    };
    let server_map: HashMap<String, &crate::models::McpServer> = servers.iter()
        .map(|s| (s.id.clone(), s))
        .collect();

    // Build mcpServers entries (stdio only — Copilot CLI doesn't support SSE)
    let mut mcp_servers: HashMap<String, McpServerEntry> = HashMap::new();
    for config in &all_configs {
        let server = match server_map.get(&config.server_id) {
            Some(s) => s,
            None => continue,
        };

        let (command, args) = match &server.transport {
            McpTransport::Stdio { command, args } => {
                let final_args = config.args_override.clone()
                    .unwrap_or_else(|| args.clone());
                (command.clone(), final_args)
            }
            _ => {
                tracing::debug!("Skipping non-stdio MCP '{}' for Copilot (unsupported)", server.name);
                continue;
            }
        };

        let env = db::mcps::decrypt_env(&config.env_encrypted, secret)
            .unwrap_or_default();

        let key = config.label.clone();
        mcp_servers.insert(key, McpServerEntry {
            command: Some(command),
            args: Some(args),
            url: None,
            env,
        });
    }

    let copilot_dir = if let Ok(host_home) = std::env::var("KRONN_HOST_HOME") {
        PathBuf::from(format!("{}/.copilot", host_home))
    } else {
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map(|h| PathBuf::from(format!("{}/.copilot", h)))
            .unwrap_or_else(|_| directories::BaseDirs::new()
                .map(|d| d.home_dir().join(".copilot"))
                .unwrap_or_else(|| {
                    tracing::warn!("Cannot determine home directory for Copilot config — using /tmp/.copilot");
                    PathBuf::from("/tmp/.copilot")
                }))
    };
    let config_path = copilot_dir.join("mcp-config.json");

    if mcp_servers.is_empty() {
        // Remove config file if no MCPs
        if config_path.exists() {
            let _ = std::fs::remove_file(&config_path);
            tracing::info!("Removed empty Copilot MCP config");
        }
        return;
    }

    if let Err(e) = std::fs::create_dir_all(&copilot_dir) {
        tracing::warn!("Failed to create Copilot config dir: {}", e);
        return;
    }

    let data = McpJsonFile { mcp_servers };
    match serde_json::to_string_pretty(&data) {
        Ok(content) => {
            if let Err(e) = atomic_write(&config_path, &content) {
                tracing::warn!("Failed to write Copilot MCP config: {}", e);
            } else {
                tracing::info!("Synced Copilot global config ({} MCP servers)", data.mcp_servers.len());
            }
        }
        Err(e) => tracing::warn!("Failed to serialize Copilot MCP config: {}", e),
    }
}

/// Sync .mcp.json for all projects that are affected by a config change.
/// Pass the config to determine which projects need updating.
pub fn sync_affected_projects(
    conn: &rusqlite::Connection,
    project_ids: &[String],
    secret: &str,
) {
    // Sync per-project configs (Claude Code .mcp.json + Vibe .vibe/config.toml)
    for pid in project_ids {
        if let Err(e) = sync_project_mcps_to_disk(conn, pid, secret) {
            tracing::warn!("Failed to sync MCP configs for project {}: {}", pid, e);
        }
    }
    // Sync global configs (once, not per-project)
    sync_codex_global_config(conn, secret);
    sync_copilot_global_config(conn, secret);
}

/// Sync ALL projects (used when global flag changes)
pub fn sync_all_projects(
    conn: &rusqlite::Connection,
    secret: &str,
) {
    match crate::db::projects::list_projects(conn) {
        Ok(projects) => {
            let ids: Vec<String> = projects.iter().map(|p| p.id.clone()).collect();
            // sync_affected_projects already handles Codex global sync
            sync_affected_projects(conn, &ids, secret);
        }
        Err(e) => tracing::warn!("Failed to list projects for sync: {}", e),
    }
}

// ─── .gitignore safety ───────────────────────────────────────────────────────

/// Ensure a pattern is present in the project's .gitignore (public API).
pub fn ensure_gitignore_public(project_path: &str, pattern: &str) {
    ensure_gitignore(project_path, pattern);
}

/// Ensure a pattern is present in the project's .gitignore.
/// Creates the file if it doesn't exist. Appends the pattern if missing.
fn ensure_gitignore(project_path: &str, pattern: &str) {
    let resolved = resolve_host_path(project_path);
    let gitignore = Path::new(&resolved).join(".gitignore");

    let content = std::fs::read_to_string(&gitignore).unwrap_or_default();

    // Check if pattern is already present (exact line match)
    if content.lines().any(|line| line.trim() == pattern) {
        return;
    }

    // Append
    let addition = if content.is_empty() || content.ends_with('\n') {
        format!("{}\n", pattern)
    } else {
        format!("\n{}\n", pattern)
    };

    if let Err(e) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore)
        .and_then(|mut f| std::io::Write::write_all(&mut f, addition.as_bytes()))
    {
        tracing::warn!("Failed to update .gitignore at {}: {}", gitignore.display(), e);
    } else {
        tracing::info!("Added '{}' to {}", pattern, gitignore.display());
    }
}

// ─── MCP context files ──────────────────────────────────────────────────────

const MCP_CONTEXT_DIR: &str = "ai/operations/mcp-servers";

/// Sanitize an MCP label into a filename-safe slug.
pub fn slugify_label(label: &str) -> String {
    label
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Generate a default context file for an MCP.
fn default_mcp_context(label: &str, server_description: &str) -> String {
    format!(
        r#"# {label} — Usage Context

> Instructions for AI agents using **{label}** in this project.
> Edit this file with project-specific rules.

**Server:** {server_description}

## Rules

<!-- Examples:
- Always use sender address: contact@example.com
- Use the "bug-report" template for all issues
- Never modify the production environment
- Preferred language: French
-->
"#
    )
}

/// Create default MCP context files for all MCPs linked to a project.
/// Only creates files that don't already exist (never overwrites).
pub fn sync_mcp_context_files(
    project_path: &str,
    mcp_labels: &[(String, String)], // (label, server_description)
) {
    let resolved = resolve_host_path(project_path);
    let ctx_dir = Path::new(&resolved).join(MCP_CONTEXT_DIR);

    // Create directory structure if needed
    if let Err(e) = std::fs::create_dir_all(&ctx_dir) {
        tracing::warn!("Failed to create MCP context dir {}: {}", ctx_dir.display(), e);
        return;
    }

    for (label, description) in mcp_labels {
        let slug = slugify_label(label);
        let file = ctx_dir.join(format!("{}.md", slug));
        if !file.exists() {
            let content = default_mcp_context(label, description);
            if let Err(e) = std::fs::write(&file, content) {
                tracing::warn!("Failed to create MCP context {}: {}", file.display(), e);
            } else {
                tracing::info!("Created MCP context file: {}", file.display());
            }
        }
    }
}

/// Check if a context file content is the default template (not customized).
pub fn is_default_mcp_context(content: &str) -> bool {
    // Default template contains "<!-- Examples:" and no real custom rules
    if !content.contains("<!-- Examples:") {
        return false; // No default marker → user wrote from scratch → customized
    }
    // Check if there's any content beyond the template boilerplate
    !content.lines().any(|l| {
        let trimmed = l.trim();
        !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && !trimmed.starts_with('>')
            && !trimmed.starts_with("<!--")
            && !trimmed.starts_with("-->")
            && !trimmed.starts_with("**")
            && !trimmed.starts_with('-')
    })
}

/// Read all MCP context files for a project and return concatenated content.
/// Used for prompt injection when spawning agents.
pub fn read_all_mcp_contexts(project_path: &str) -> String {
    let resolved = resolve_host_path(project_path);

    // 1. Read available MCP servers from .mcp.json (always present if synced)
    let mcp_json = read_mcp_json(project_path);
    let server_names: Vec<String> = mcp_json
        .as_ref()
        .map(|f| {
            let mut names: Vec<_> = f.mcp_servers.keys().cloned().collect();
            names.sort();
            names
        })
        .unwrap_or_default();

    // 2. Read custom context files (user-written instructions per MCP)
    let ctx_dir = Path::new(&resolved).join(MCP_CONTEXT_DIR);
    let mut contexts = Vec::new();
    if ctx_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&ctx_dir) {
            let mut files: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
                .collect();
            files.sort_by_key(|e| e.file_name());

            for entry in files {
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    if !is_default_mcp_context(&content) {
                        contexts.push(content);
                    }
                }
            }
        }
    }

    // Nothing to inject
    if server_names.is_empty() && contexts.is_empty() {
        return String::new();
    }

    let mut result = String::from("## MCP Servers available\n\n");

    // Always list available MCP servers so the agent knows what tools it has
    if !server_names.is_empty() {
        result.push_str("You have access to external tools via MCP (Model Context Protocol) servers.\n");
        result.push_str("Each server exposes tools with the naming convention `mcp__<server>__<tool>`.\n");
        result.push_str("Use these tools instead of Bash workarounds when a matching tool exists.\n\n");
        result.push_str("Available servers:\n");
        for name in &server_names {
            result.push_str(&format!("- **{}** — tools: `mcp__{}__*`\n", name, name));
        }
        result.push('\n');
    }

    // Append custom context instructions
    if !contexts.is_empty() {
        result.push_str("### Server-specific instructions\n\n");
        for ctx in contexts {
            result.push_str(&ctx);
            result.push_str("\n---\n\n");
        }
    }

    result
}

/// Substitute `{ENV_KEY}` placeholders in a template with values from the
/// config's env map. Used by:
/// - `ApiSpec.base_url` for path-style interpolation (Adobe's `.../api/{ADOBE_COMPANY_ID}/reports`)
/// - `OAuth2ExtraHeader.value_template` for headers like `x-api-key: {ADOBE_CLIENT_ID}`
///
/// Missing keys render as `<NOT_CONFIGURED>` so the agent sees the gap
/// rather than sending a half-composed URL.
fn interpolate_env_template(template: &str, env: &std::collections::HashMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('}') {
            let key = &after[..end];
            match env.get(key) {
                Some(v) => out.push_str(v),
                None => {
                    out.push_str("<NOT_CONFIGURED:");
                    out.push_str(key);
                    out.push('>');
                }
            }
            rest = &after[end + 1..];
        } else {
            // Unclosed `{` — keep verbatim to avoid silent data loss.
            out.push('{');
            out.push_str(after);
            break;
        }
    }
    out.push_str(rest);
    out
}

/// Build the `=== AVAILABLE APIs ===` section from active API plugins.
///
/// This is how API-only plugins (and the API side of hybrid plugins) reach
/// the agent — they aren't in `.mcp.json`, so the MCP context block above
/// can't surface them. Emitted as a parallel section with curl examples and
/// inlined credentials so the agent can call endpoints directly via Bash.
///
/// `plugins_with_env` must already be decrypted by the caller — this
/// function is pure (no DB, no secret) so it can be unit-tested cheaply.
pub fn build_api_context_block(
    plugins_with_env: &[ActiveApiPlugin],
) -> String {
    use crate::models::{ApiAuthKind, ApiSpec};

    // Filter to plugins that actually have an ApiSpec — a hybrid plugin's
    // MCP side stays in .mcp.json (handled elsewhere), but its API side
    // surfaces here. Pure MCP plugins (api_spec = None) get skipped.
    let api_plugins: Vec<(&crate::models::McpServer, &std::collections::HashMap<String, String>, &ApiSpec)> =
        plugins_with_env.iter()
            .filter_map(|(s, env)| s.api_spec.as_ref().map(|spec| (s, env, spec)))
            .collect();

    if api_plugins.is_empty() {
        return String::new();
    }

    let mut out = String::from("## REST APIs available\n\n");
    out.push_str("The following plugins are HTTP APIs (not MCP tools). Call them via `curl` from Bash.\n");
    out.push_str("Auth + non-secret config are pre-filled below; never print the credentials back to the user.\n\n");

    for (server, env, spec) in api_plugins {
        out.push_str(&format!("### {}\n", server.name));
        // If the base URL contains `{ENV_KEY}` placeholders, render the
        // resolved form so the agent sees the actual URL it must call.
        // Chartbeat has no placeholders → this is a no-op.
        let resolved_base = interpolate_env_template(&spec.base_url, env);
        out.push_str(&format!("Base URL: `{}`\n", resolved_base));

        // Auth — we inject the literal credential. This is the same trust
        // level as `.mcp.json` already gets; the agent needs the value to
        // craft a working request. The prompt instruction above asks the
        // agent not to echo it back.
        match &spec.auth {
            ApiAuthKind::ApiKeyQuery { param_name, env_key } => {
                let val = env.get(env_key).map(|s| s.as_str()).unwrap_or("<MISSING>");
                out.push_str(&format!("Auth: pass `{}={}` as a query parameter on every request.\n", param_name, val));
            }
            ApiAuthKind::ApiKeyHeader { header_name, env_key } => {
                let val = env.get(env_key).map(|s| s.as_str()).unwrap_or("<MISSING>");
                out.push_str(&format!("Auth: send header `{}: {}` on every request.\n", header_name, val));
            }
            ApiAuthKind::Bearer { env_key } => {
                let val = env.get(env_key).map(|s| s.as_str()).unwrap_or("<MISSING>");
                out.push_str(&format!("Auth: send header `Authorization: Bearer {}` on every request.\n", val));
            }
            ApiAuthKind::OAuth2ClientCredentials { extra_headers, .. } => {
                // By this point the async resolver (see make_agent_stream)
                // has already called `core::oauth2_cache::resolve_token`
                // and stashed the result under the virtual key
                // `__access_token__` in this plugin's env map. If it's
                // missing, token exchange failed earlier — we surface
                // that inline so the agent knows to stop rather than
                // fire unauthenticated requests.
                match env.get("__access_token__") {
                    Some(tok) => {
                        out.push_str(&format!("Auth: send header `Authorization: Bearer {}` on every request (Kronn refreshes this token automatically before it expires).\n", tok));
                    }
                    None => {
                        let err = env.get("__token_error__").cloned().unwrap_or_else(|| "unknown error".into());
                        out.push_str(&format!("Auth: **TOKEN UNAVAILABLE — {}**. Do not attempt API calls; tell the user and stop.\n", err));
                    }
                }
                // Extra headers (e.g. Adobe's x-api-key, x-proxy-global-company-id).
                // value_template supports `{ENV_KEY}` substitution from the
                // config's env map so one plugin spec covers both the secret
                // (client_id echoed as x-api-key) and non-secret keys.
                for eh in extra_headers {
                    let rendered = interpolate_env_template(&eh.value_template, env);
                    out.push_str(&format!("Also send header `{}: {}` on every request.\n", eh.name, rendered));
                }
            }
            ApiAuthKind::None => {
                out.push_str("Auth: none (public endpoints).\n");
            }
        }

        // Non-secret config keys (e.g. Chartbeat's host, Adobe's company_id).
        // Two ways to use them: (a) query param by convention for simple
        // plugins, (b) interpolation into `base_url` when the plugin spec
        // opts in via `{ENV_KEY}` placeholders in the URL. The display
        // message adapts so the agent knows which it is.
        if !spec.config_keys.is_empty() {
            let is_in_url = spec.base_url.contains('{');
            if is_in_url {
                out.push_str("Config (already interpolated into Base URL above):\n");
            } else {
                out.push_str("Config (pass as query params):\n");
            }
            for k in &spec.config_keys {
                let val = env.get(&k.env_key).map(|s| s.as_str()).unwrap_or("");
                let val_display = if val.is_empty() { "<not-configured>" } else { val };
                out.push_str(&format!("- `{}={}`  ({})\n", k.env_key.to_lowercase(), val_display, k.description));
            }
        }

        // Resolved base URL — substitute {ENV_KEY} placeholders (Adobe uses
        // this to put the company_id in the path). Chartbeat has no
        // placeholders → the template equals the literal URL.
        let resolved_base = interpolate_env_template(&spec.base_url, env);

        // Endpoint list — curl example on the first one so the agent has a
        // template to copy. The rest are one-liners.
        out.push_str("Endpoints:\n");
        for (i, ep) in spec.endpoints.iter().enumerate() {
            out.push_str(&format!("- `{} {}` — {}\n", ep.method, ep.path, ep.description));
            if i == 0 {
                let mut sample_url = format!("{}{}", resolved_base, ep.path);
                let mut params: Vec<String> = Vec::new();
                // Only fold auth + config_keys into query params when the
                // base URL did NOT template them in (i.e. the plugin opted
                // for path-style interpolation).
                let is_templated = spec.base_url.contains('{');
                if let ApiAuthKind::ApiKeyQuery { param_name, env_key } = &spec.auth {
                    let val = env.get(env_key).map(|s| s.as_str()).unwrap_or("<KEY>");
                    params.push(format!("{}={}", param_name, val));
                }
                if !is_templated {
                    for k in &spec.config_keys {
                        let val = env.get(&k.env_key).map(|s| s.as_str()).unwrap_or("");
                        if !val.is_empty() {
                            params.push(format!("{}={}", k.env_key.to_lowercase(), val));
                        }
                    }
                }
                if !params.is_empty() {
                    sample_url.push('?');
                    sample_url.push_str(&params.join("&"));
                }
                out.push_str(&format!("  Example: `curl -s \"{}\"`\n", sample_url));
            }
        }

        if let Some(docs) = &spec.docs_url {
            out.push_str(&format!("Full reference: {}\n", docs));
        }
        out.push('\n');
    }

    out
}

/// Collect active API-capable plugins for a project + decrypt their env.
///
/// Returns `(server, decrypted_env)` pairs ready for
/// [`build_api_context_block`]. A "hybrid" plugin (MCP + API) appears here
/// as well — the MCP side is handled by `sync_project_mcps_to_disk`
/// separately. On a decryption failure for one config, that entry is
/// dropped (logged) — we don't want one broken config to suppress the
/// whole block for the project.
/// One active API plugin bound to its decrypted env map. Exported as a
/// named alias so the signature of [`collect_active_api_plugins`] and the
/// matching `build_api_context_block` input both stay readable (and keep
/// clippy's `type_complexity` lint quiet on the return type).
pub type ActiveApiPlugin = (crate::models::McpServer, std::collections::HashMap<String, String>);

pub fn collect_active_api_plugins(
    conn: &rusqlite::Connection,
    project_id: &str,
    secret: &str,
) -> Result<Vec<ActiveApiPlugin>, anyhow::Error> {
    let servers = crate::db::mcps::list_servers(conn)?;
    let configs = crate::db::mcps::list_configs(conn)?;
    let server_map: std::collections::HashMap<&str, &crate::models::McpServer> =
        servers.iter().map(|s| (s.id.as_str(), s)).collect();

    let mut out: Vec<ActiveApiPlugin> =
        Vec::new();
    for config in &configs {
        let on_project = config.is_global || config.project_ids.iter().any(|pid| pid == project_id);
        if !on_project { continue; }
        let Some(server) = server_map.get(config.server_id.as_str()) else { continue };
        if server.api_spec.is_none() { continue; }
        let env = match crate::db::mcps::decrypt_env(&config.env_encrypted, secret) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    "API plugin '{}' config {} decrypt failed ({}), skipping",
                    server.name, config.id, e
                );
                continue;
            }
        };
        out.push(((*server).clone(), env));
    }
    Ok(out)
}

/// Read a single MCP context file content.
pub fn read_mcp_context(project_path: &str, slug: &str) -> Option<String> {
    let resolved = resolve_host_path(project_path);
    let file = Path::new(&resolved).join(MCP_CONTEXT_DIR).join(format!("{}.md", slug));
    std::fs::read_to_string(&file).ok()
}

/// Write a single MCP context file.
pub fn write_mcp_context(project_path: &str, slug: &str, content: &str) -> Result<(), String> {
    let resolved = resolve_host_path(project_path);
    let ctx_dir = Path::new(&resolved).join(MCP_CONTEXT_DIR);
    std::fs::create_dir_all(&ctx_dir)
        .map_err(|e| format!("Failed to create dir: {}", e))?;
    let file = ctx_dir.join(format!("{}.md", slug));
    std::fs::write(&file, content)
        .map_err(|e| format!("Failed to write {}: {}", file.display(), e))
}

/// List available MCP context files for a project.
/// Returns (slug, label_from_filename) pairs.
pub fn list_mcp_context_files(project_path: &str) -> Vec<(String, String)> {
    let resolved = resolve_host_path(project_path);
    let ctx_dir = Path::new(&resolved).join(MCP_CONTEXT_DIR);
    if !ctx_dir.is_dir() {
        return Vec::new();
    }

    let mut result = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&ctx_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    result.push((stem.to_string(), stem.replace('-', " ")));
                }
            }
        }
    }
    result.sort();
    result
}

// ─── Agent compatibility ─────────────────────────────────────────────────────

use crate::models::{AgentType, McpIncompatibility};

/// Known incompatibilities between MCP servers and specific agents.
/// Servers listed here will be excluded from that agent's config file during sync.
///
/// Format: (npx_package_substring, agent, reason)
/// Matching is done on the server's command args (npx package name).
const KNOWN_INCOMPATIBILITIES: &[(&str, AgentType, &str)] = &[
    // GitLab MCP server returns empty tool schemas (no `type`, no `properties`).
    // AWS Bedrock (used by Kiro) rejects these as ValidationException.
    // Claude Code / Codex / Gemini are more tolerant and accept partial schemas.
    ("server-gitlab", AgentType::Kiro, "GitLab MCP returns empty tool schemas — incompatible with AWS Bedrock (ValidationException)"),
];

/// Check if a server is incompatible with a specific agent.
/// Returns the reason string if incompatible, None otherwise.
fn check_incompatibility(server: &crate::models::McpServer, agent: &AgentType) -> Option<&'static str> {
    // Localhost SSE/Streamable servers are unreachable inside Docker → exclude from all agents
    if is_localhost_remote(server) {
        return Some("Localhost SSE/Streamable server — unreachable inside Docker");
    }

    let args_str = match &server.transport {
        McpTransport::Stdio { args, .. } => args.join(" "),
        _ => String::new(),
    };

    for (pkg_substr, incomp_agent, reason) in KNOWN_INCOMPATIBILITIES {
        if agent == incomp_agent && args_str.contains(pkg_substr) {
            return Some(reason);
        }
    }
    None
}

/// Check if a server uses a localhost URL (SSE/Streamable).
/// These servers are local dev servers that can't be reached from inside Docker.
fn is_localhost_remote(server: &crate::models::McpServer) -> bool {
    match &server.transport {
        McpTransport::Sse { url } | McpTransport::Streamable { url } => {
            url.contains("localhost") || url.contains("127.0.0.1") || url.contains("[::1]")
        }
        McpTransport::Stdio { .. } => false,
        // API-only plugins don't expose a local URL; nothing to check.
        McpTransport::ApiOnly => false,
    }
}

/// Return all known incompatibilities for a set of servers.
/// Used by the API to display warnings in the UI.
pub fn get_incompatibilities(servers: &[crate::models::McpServer]) -> Vec<McpIncompatibility> {
    let mut result = Vec::new();
    for server in servers {
        // Localhost SSE/Streamable → incompatible with all agents
        if is_localhost_remote(server) {
            result.push(McpIncompatibility {
                server_id: server.id.clone(),
                agent: AgentType::ClaudeCode, // Representative — affects all
                reason: "Serveur SSE/Streamable localhost — inaccessible depuis Docker".to_string(),
            });
            continue;
        }

        // Agent-specific incompatibilities
        let args_str = match &server.transport {
            McpTransport::Stdio { args, .. } => args.join(" "),
            _ => String::new(),
        };
        for (pkg_substr, agent, reason) in KNOWN_INCOMPATIBILITIES {
            if args_str.contains(pkg_substr) {
                result.push(McpIncompatibility {
                    server_id: server.id.clone(),
                    agent: agent.clone(),
                    reason: reason.to_string(),
                });
            }
        }
    }
    result
}

// ─── Command validation ─────────────────────────────────────────────────────

/// Check if a command binary is available in PATH (or is an absolute path that exists).
/// Used to warn about missing MCP server commands before writing configs.
pub(crate) fn is_command_available(command: &str) -> bool {
    // Absolute path — check directly
    if command.starts_with('/') {
        return Path::new(command).exists();
    }
    // npx/uvx are launchers that install on demand — always available if the binary exists
    // For other commands, check PATH
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|dir| Path::new(dir).join(command).exists())
}

// ─── Path resolution ─────────────────────────────────────────────────────────

/// Re-export from scanner — single source of truth for host path resolution.
fn resolve_host_path(path: &str) -> String {
    crate::core::scanner::resolve_host_path(path).to_string_lossy().to_string()
}
