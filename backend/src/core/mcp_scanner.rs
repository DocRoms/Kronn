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
}

fn default_true() -> bool { true }
fn is_true(v: &bool) -> bool { *v }

// ─── Vibe config.toml format ─────────────────────────────────────────────────

/// Vibe config.toml `[[mcp_servers]]` entry
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VibeMcpEntry {
    name: String,
    transport: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    env: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct VibeConfig {
    #[serde(default)]
    mcp_servers: Vec<VibeMcpEntry>,
}

// ─── .mcp.json file format (Claude Code) ─────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpJsonFile {
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: HashMap<String, McpServerEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    let resolved = resolve_host_path(project_path);
    let file = Path::new(&resolved).join(".mcp.json");
    let content = serde_json::to_string_pretty(data)
        .map_err(|e| format!("JSON serialize error: {}", e))?;
    std::fs::write(&file, content)
        .map_err(|e| format!("Failed to write {}: {}", file.display(), e))
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
    let mut configs_per_server: HashMap<String, usize> = HashMap::new();
    for config in &configs {
        *configs_per_server.entry(config.server_id.clone()).or_insert(0) += 1;
    }

    // Build the McpJsonFile
    let mut mcp_servers = HashMap::new();
    for config in &configs {
        let server = match server_map.get(&config.server_id) {
            Some(s) => s,
            None => continue,
        };

        // Decrypt env
        let env = db::mcps::decrypt_env(&config.env_encrypted, secret)
            .unwrap_or_default();

        let entry = match &server.transport {
            McpTransport::Stdio { command, args } => {
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
        };

        // Use server name when there's only one config, label when multiple
        let key = if configs_per_server.get(&config.server_id).copied().unwrap_or(0) > 1 {
            config.label.clone()
        } else {
            server.name.to_lowercase()
        };
        mcp_servers.insert(key, entry);
    }

    if mcp_servers.is_empty() {
        // Remove config files if no MCPs
        let resolved = resolve_host_path(&project.path);
        for filename in &[".mcp.json", ".vibe/config.toml"] {
            let file = std::path::Path::new(&resolved).join(filename);
            if file.exists() {
                let _ = std::fs::remove_file(&file);
                tracing::info!("Removed {} from {} (no MCPs)", filename, project.path);
            }
        }
    } else {
        // ── Claude Code: .mcp.json ──
        let data = McpJsonFile { mcp_servers };
        write_mcp_json(&project.path, &data)?;
        ensure_gitignore(&project.path, ".mcp.json");
        tracing::info!("Synced .mcp.json for {} ({} MCPs)", project.path, configs.len());

        // ── Vibe: .vibe/config.toml ──
        sync_vibe_project_config(&project.path, &configs, &server_map, secret);

        // MCP context files are only created when the user explicitly writes
        // custom instructions via the UI (write_mcp_context). No auto-creation
        // of empty/template files — they add no value and pollute the project.
    }

    Ok(())
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
    // Count configs per server for naming
    let mut configs_per_server: HashMap<String, usize> = HashMap::new();
    for config in configs {
        *configs_per_server.entry(config.server_id.clone()).or_insert(0) += 1;
    }

    for config in configs {
        let server = match server_map.get(&config.server_id) {
            Some(s) => s,
            None => continue,
        };

        let env = crate::db::mcps::decrypt_env(&config.env_encrypted, secret)
            .unwrap_or_default();

        let name = if configs_per_server.get(&config.server_id).copied().unwrap_or(0) > 1 {
            config.label.clone()
        } else {
            server.name.to_lowercase()
        };

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
            if let Err(e) = std::fs::write(&vibe_config, format!("{}{}", header, content)) {
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

    // Count configs per server for naming
    let mut configs_per_server: HashMap<String, usize> = HashMap::new();
    for config in &all_configs {
        *configs_per_server.entry(config.server_id.clone()).or_insert(0) += 1;
    }

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
        let raw_key = if configs_per_server.get(&config.server_id).copied().unwrap_or(0) > 1 {
            config.label.clone()
        } else {
            server.name.to_lowercase()
        };
        let key = slugify_label(&raw_key);

        mcp_entries.insert(key, CodexMcpEntry {
            command,
            args,
            env,
            enabled: true,
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
                .unwrap_or_else(|| PathBuf::from("/home/kronn/.codex")))
    };
    let codex_config = codex_dir.join("config.toml");

    // Parse existing config as a TOML table to preserve other settings
    let mut doc: toml::value::Table = if codex_config.exists() {
        match std::fs::read_to_string(&codex_config) {
            Ok(content) => content.parse::<toml::Value>()
                .ok()
                .and_then(|v| v.as_table().cloned())
                .unwrap_or_default(),
            Err(_) => toml::value::Table::new(),
        }
    } else {
        toml::value::Table::new()
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

    // Write back
    if let Err(e) = std::fs::create_dir_all(codex_dir) {
        tracing::warn!("Failed to create Codex config dir: {}", e);
        return;
    }

    match toml::to_string_pretty(&doc) {
        Ok(content) => {
            if let Err(e) = std::fs::write(&codex_config, content) {
                tracing::warn!("Failed to write Codex config: {}", e);
            } else {
                tracing::info!("Synced Codex global config ({} MCP servers)", mcp_entries.len());
            }
        }
        Err(e) => tracing::warn!("Failed to serialize Codex config: {}", e),
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
    // Sync Codex global config (once, not per-project)
    sync_codex_global_config(conn, secret);
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
        result.push_str("You have access to the following MCP servers in this project. ");
        result.push_str("Use their tools (prefixed `mcp__<server>__<tool>`) instead of Bash workarounds.\n\n");
        result.push_str("Available servers:\n");
        for name in &server_names {
            result.push_str(&format!("- **{}**\n", name));
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

// ─── Path resolution ─────────────────────────────────────────────────────────

/// Resolve a project path that might be a host path (when running in Docker).
/// Inside Docker, host $HOME is mounted at /host-home.
/// KRONN_HOST_HOME contains the actual host home path (e.g. /home/priol).
pub fn resolve_host_path(path: &str) -> String {
    if Path::new(path).exists() {
        return path.to_string();
    }
    if let Ok(host_home) = std::env::var("KRONN_HOST_HOME") {
        if let Some(relative) = path.strip_prefix(&host_home) {
            let mapped = format!("/host-home{}", relative);
            if Path::new(&mapped).exists() {
                return mapped;
            }
        }
    }
    path.to_string()
}
