use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

use crate::models::McpTransport;

const MCP_OWNERSHIP_STATE: &str = ".kronn/mcp-managed.json";
static MCP_PROJECT_CONFIG_SYNC_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_project_config_sync() -> std::sync::MutexGuard<'static, ()> {
    MCP_PROJECT_CONFIG_SYNC_LOCK.lock().unwrap_or_else(|poisoned| {
        tracing::error!("MCP project config sync lock was poisoned; recovering serialized access");
        poisoned.into_inner()
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpOwnershipState {
    version: u8,
    #[serde(default)]
    files: BTreeMap<String, BTreeSet<String>>,
}

impl Default for McpOwnershipState {
    fn default() -> Self {
        Self { version: 1, files: BTreeMap::new() }
    }
}

fn refuse_symlink(path: &Path, label: &str) -> Result<(), String> {
    if std::fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!("{label} {} is a symlink; refusing mutation", path.display()));
    }
    Ok(())
}

fn refuse_project_symlink_chain(project_path: &str, target: &Path, label: &str) -> Result<(), String> {
    let resolved = resolve_host_path(project_path);
    let root = Path::new(&resolved);
    let relative = target.strip_prefix(root)
        .map_err(|_| format!("{label} {} escapes project root {}; refusing mutation", target.display(), root.display()))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        refuse_symlink(&current, label)?;
    }
    Ok(())
}

fn ensure_safe_kronn_state_dir(project_path: &str) -> Result<PathBuf, String> {
    let resolved = resolve_host_path(project_path);
    let root = Path::new(&resolved);
    let state_dir = root.join(".kronn");
    refuse_symlink(&state_dir, "Kronn state directory")?;
    std::fs::create_dir_all(&state_dir)
        .map_err(|e| format!("Failed to create Kronn state dir {}: {e}", state_dir.display()))?;
    Ok(state_dir)
}

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

/// Inject the `kronn-internal` MCP server entry into an existing
/// `McpJsonFile`. Idempotent: replaces an existing `kronn-internal`
/// entry, leaves all other entries untouched. The actual disc id is
/// passed via the agent process env (`KRONN_DISCUSSION_ID`) rather
/// than the MCP entry's own env, so a single .mcp.json works across
/// runs of different discussions in the same project workspace.
///
/// Returns `true` when an entry was injected (the caller can decide
/// whether to re-write the file). When the bridge script can't be
/// located at a path that's valid on **both sides** (Kronn-spawn
/// in-container AND host-CLI), returns `false` and removes any
/// previously-injected stale entry — better than leaving a broken
/// command that breaks every host CLI invocation with `Broken pipe`.
pub fn inject_kronn_internal(file: &mut McpJsonFile) -> bool {
    let script = match crate::agents::runner::disc_introspection_mcp_path_for_shared_config() {
        Some(p) => p,
        None => {
            // Stale entry from a previous Kronn build that wrote the
            // container path? Clean it up so host CLIs stop choking.
            file.mcp_servers.remove("kronn-internal");
            return false;
        }
    };
    let entry = McpServerEntry {
        command: Some("python3".into()),
        args: Some(vec![script]),
        url: None,
        env: HashMap::new(),
    };
    file.mcp_servers.insert("kronn-internal".into(), entry);
    true
}

/// Codex variant of `inject_kronn_internal` — same script, same
/// behaviour, but written into a `HashMap<String, CodexMcpEntry>`
/// (the shape Codex's `~/.codex/config.toml` `[mcp_servers]` uses).
///
/// `~/.codex/config.toml` lives in the user's home and is read by
/// **both** the Kronn-spawned-in-container Codex AND the user's
/// host `codex` CLI — same shared-path constraint as
/// [`inject_kronn_internal`].
fn inject_kronn_internal_codex(entries: &mut HashMap<String, CodexMcpEntry>) -> bool {
    let script = match crate::agents::runner::disc_introspection_mcp_path_for_shared_config() {
        Some(p) => p,
        None => {
            entries.remove("kronn-internal");
            return false;
        }
    };
    entries.insert("kronn-internal".into(), CodexMcpEntry {
        command: "python3".into(),
        args: vec![script],
        env: HashMap::new(),
        enabled: true,
        startup_timeout_sec: default_startup_timeout(),
    });
    true
}

/// Write `.mcp.json` (+ Kiro `.kiro/settings/mcp.json` & `.ai/mcp/mcp.json`,
/// Gemini `.gemini/settings.json`) containing ONLY the built-in `kronn-internal`
/// introspection bridge — for a project that has no user-linked MCPs.
///
/// Without this, `sync_project_mcps_to_disk` used to *delete* those files when a
/// project had no other MCPs, so project-bound discussions lost kronn-internal
/// entirely (reported 2026-06-26). Mirrors `write_general_mcp_json`.
///
/// Returns `false` when the bridge path can't be resolved (host-CLI safety, cf.
/// [`disc_introspection_mcp_path_for_shared_config`]); the caller then removes
/// any stale files instead.
pub(crate) fn write_kronn_internal_only(project_path: &str) -> Result<bool, String> {
    let mut only_internal = McpJsonFile { mcp_servers: HashMap::new() };
    if !inject_kronn_internal(&mut only_internal) {
        return Ok(false);
    }
    write_owned_mcp_json_to_subpath(project_path, ".mcp.json", &only_internal)?;
    ensure_gitignore(project_path, ".mcp.json");
    let merged = read_mcp_json(project_path).unwrap_or_else(|| only_internal.clone());
    sync_claude_enabled_servers(project_path, &merged.mcp_servers);
    for (subpath, ignore) in [
        (".kiro/settings/mcp.json", ".kiro/settings/"),
        (".ai/mcp/mcp.json", ".ai/mcp/"),
        (".gemini/settings.json", ".gemini/"),
    ] {
        write_owned_mcp_json_to_subpath(project_path, subpath, &only_internal)?;
        ensure_gitignore(project_path, ignore);
    }
    Ok(true)
}

fn backup_before_refusal(project_path: &str, file: &Path) -> Result<(), String> {
    if !file.exists() {
        return Ok(());
    }
    refuse_project_symlink_chain(project_path, file, "MCP config path")?;
    let resolved = resolve_host_path(project_path);
    let root = Path::new(&resolved);
    let relative = file.strip_prefix(root).unwrap_or(file);
    let safe_name = relative.to_string_lossy().replace(['/', '\\'], "__");
    let state_dir = ensure_safe_kronn_state_dir(project_path)?;
    let backups_dir = state_dir.join("backups");
    refuse_symlink(&backups_dir, "MCP backup directory")?;
    std::fs::create_dir_all(&backups_dir)
        .map_err(|e| format!("Failed to create backup dir {}: {e}", backups_dir.display()))?;
    let configs_dir = backups_dir.join("mcp-configs");
    refuse_symlink(&configs_dir, "MCP backup directory")?;
    std::fs::create_dir_all(&configs_dir)
        .map_err(|e| format!("Failed to create backup dir {}: {e}", configs_dir.display()))?;
    let backup = configs_dir.join(format!("{safe_name}.backup"));
    refuse_symlink(&backup, "MCP backup file")?;
    if backup.exists() {
        return Ok(());
    }
    ensure_gitignore(project_path, ".kronn/");
    std::fs::copy(file, &backup)
        .map_err(|e| format!("Failed to back up {} to {}: {e}", file.display(), backup.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Failed to secure backup {}: {e}", backup.display()))?;
    }
    Ok(())
}

fn secure_file_0600(file: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Failed to secure MCP config {}: {e}", file.display()))?;
    }
    Ok(())
}

fn ownership_state_path(project_path: &str) -> PathBuf {
    let resolved = resolve_host_path(project_path);
    Path::new(&resolved).join(MCP_OWNERSHIP_STATE)
}

fn load_mcp_ownership(project_path: &str) -> Result<McpOwnershipState, String> {
    let path = ownership_state_path(project_path);
    refuse_project_symlink_chain(project_path, &path, "MCP ownership state")?;
    if !path.exists() {
        return Ok(McpOwnershipState::default());
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read MCP ownership state {}: {e}", path.display()))?;
    match serde_json::from_str::<McpOwnershipState>(&raw) {
        Ok(state) if state.version == 1 => Ok(state),
        Ok(state) => {
            backup_before_refusal(project_path, &path)?;
            Err(format!("Unsupported MCP ownership state version {} in {}; refusing config mutation", state.version, path.display()))
        }
        Err(e) => {
            backup_before_refusal(project_path, &path)?;
            Err(format!("Invalid MCP ownership state {}: {e}; refusing config mutation", path.display()))
        }
    }
}

fn save_mcp_ownership(project_path: &str, state: &McpOwnershipState) -> Result<(), String> {
    let path = ownership_state_path(project_path);
    ensure_safe_kronn_state_dir(project_path)?;
    refuse_project_symlink_chain(project_path, &path, "MCP ownership state")?;
    ensure_gitignore(project_path, ".kronn/");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create MCP ownership dir {}: {e}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(state)
        .map_err(|e| format!("Failed to serialize MCP ownership state: {e}"))?;
    atomic_write(&path, &format!("{content}\n"))?;
    secure_file_0600(&path)
}

fn merge_mcp_json_file(
    project_path: &str,
    file: &Path,
    data: &McpJsonFile,
    previously_owned: &BTreeSet<String>,
) -> Result<BTreeSet<String>, String> {
    refuse_project_symlink_chain(project_path, file, "MCP JSON config path")?;
    let existed = file.exists();
    if !existed && data.mcp_servers.is_empty() {
        return Ok(BTreeSet::new());
    }
    let observed_content;
    let mut root = if existed {
        let raw = std::fs::read_to_string(file)
            .map_err(|e| format!("Failed to read existing config {}: {e}", file.display()))?;
        observed_content = Some(raw.as_bytes().to_vec());
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(serde_json::Value::Object(object)) => object,
            Ok(_) => {
                backup_before_refusal(project_path, file)?;
                return Err(format!("Existing config {} is not a JSON object; refusing overwrite", file.display()));
            }
            Err(e) => {
                backup_before_refusal(project_path, file)?;
                return Err(format!("Invalid existing JSON config {}: {e}; refusing overwrite", file.display()));
            }
        }
    } else {
        observed_content = None;
        serde_json::Map::new()
    };

    let servers = root
        .entry("mcpServers".to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let serde_json::Value::Object(existing) = servers else {
        backup_before_refusal(project_path, file)?;
        return Err(format!("Existing config {} has a non-object mcpServers field; refusing overwrite", file.display()));
    };

    let desired: BTreeMap<String, serde_json::Value> = data.mcp_servers.iter()
        .map(|(name, entry)| {
            serde_json::to_value(entry)
                .map(|value| (name.clone(), value))
                .map_err(|e| format!("Failed to serialize MCP entry {name}: {e}"))
        })
        .collect::<Result<_, _>>()?;

    for name in previously_owned {
        if !desired.contains_key(name) {
            existing.remove(name);
        }
    }

    let mut now_owned = BTreeSet::new();
    for (name, wanted) in desired {
        match existing.get(&name) {
            Some(current) if previously_owned.contains(&name) || current == &wanted => {
                existing.insert(name.clone(), wanted);
                now_owned.insert(name);
            }
            Some(_) => {
                tracing::warn!(
                    "MCP config collision in {} for '{}': preserving user-owned entry",
                    file.display(), name
                );
            }
            None => {
                existing.insert(name.clone(), wanted);
                now_owned.insert(name);
            }
        }
    }

    if existed {
        backup_before_refusal(project_path, file)?;
    }
    let content = serde_json::to_string_pretty(&serde_json::Value::Object(root))
        .map_err(|e| format!("JSON serialize error for {}: {e}", file.display()))?;
    atomic_write_if_unchanged(file, &format!("{content}\n"), observed_content.as_deref())?;
    secure_file_0600(file)?;
    Ok(now_owned)
}

/// Merge an MCP section into an arbitrary project-local JSON config. Existing
/// top-level settings and colliding user-owned MCP entries are preserved.
pub fn write_mcp_json_to_subpath(project_path: &str, subpath: &str, data: &McpJsonFile) -> Result<(), String> {
    let resolved = resolve_host_path(project_path);
    let file = Path::new(&resolved).join(subpath);
    // Validate the complete chain before creating a missing nested parent.
    // Otherwise `create_dir_all(.kiro/settings)` would follow an existing
    // `.kiro` symlink and mutate a directory outside the project even though
    // the later merge correctly refuses the final file write.
    refuse_project_symlink_chain(project_path, &file, "MCP JSON config path")?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create dir {}: {}", parent.display(), e))?;
    }
    merge_mcp_json_file(project_path, &file, data, &BTreeSet::new()).map(|_| ())
}

pub(crate) fn write_owned_mcp_json_to_subpath(
    project_path: &str,
    subpath: &str,
    data: &McpJsonFile,
) -> Result<(), String> {
    let mut state = load_mcp_ownership(project_path)?;
    let previous = state.files.get(subpath).cloned().unwrap_or_default();
    let resolved = resolve_host_path(project_path);
    let file = Path::new(&resolved).join(subpath);
    refuse_project_symlink_chain(project_path, &file, "MCP JSON config path")?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create dir {}: {e}", parent.display()))?;
    }
    let now_owned = merge_mcp_json_file(project_path, &file, data, &previous)?;
    if now_owned.is_empty() {
        state.files.remove(subpath);
    } else {
        state.files.insert(subpath.to_string(), now_owned);
    }
    save_mcp_ownership(project_path, &state)
}

/// Write content to a file atomically: write to a temp sibling then rename.
/// This prevents agents from reading a partially-written config file.
fn write_atomic_temp(target: &Path, content: &[u8]) -> Result<PathBuf, String> {
    // A fixed `<target>.tmp` name is unsafe in a user-controlled project: a
    // pre-created symlink at that path makes `fs::write` follow it and mutate
    // an arbitrary file outside the project. Use an unpredictable sibling and
    // `create_new` so an existing filesystem object is never followed.
    let tmp = target.with_extension(format!("kronn-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut temp_file = options
        .open(&tmp)
        .map_err(|e| format!("Failed to create temp {}: {e}", tmp.display()))?;
    if let Err(e) = std::io::Write::write_all(&mut temp_file, content) {
        drop(temp_file);
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("Failed to write temp {}: {e}", tmp.display()));
    }
    drop(temp_file);
    Ok(tmp)
}

fn commit_atomic_temp(tmp: &Path, target: &Path) -> Result<(), String> {
    std::fs::rename(tmp, target)
        .map_err(|e| {
            // Clean up temp file on rename failure
            let _ = std::fs::remove_file(tmp);
            format!("Failed to rename {} → {}: {}", tmp.display(), target.display(), e)
        })
}

pub(crate) fn atomic_write(target: &Path, content: &str) -> Result<(), String> {
    atomic_write_bytes(target, content.as_bytes())
}

pub(crate) fn atomic_write_bytes(target: &Path, content: &[u8]) -> Result<(), String> {
    let tmp = write_atomic_temp(target, content)?;
    commit_atomic_temp(&tmp, target)
}

/// Atomically replace a file only if it is still byte-identical to what the
/// merge parsed (`Some`) or is still absent (`None`). The temp is fully written
/// first, keeping the compare→rename window as small as the portable filesystem
/// API allows. This prevents a normal concurrent CLI/user edit from being
/// silently lost while Kronn is serializing a merged config.
pub(crate) fn atomic_write_if_unchanged(
    target: &Path,
    content: &str,
    observed: Option<&[u8]>,
) -> Result<(), String> {
    let tmp = write_atomic_temp(target, content.as_bytes())?;
    let unchanged = match (observed, std::fs::read(target)) {
        (Some(expected), Ok(current)) => current == expected,
        (None, Err(e)) if e.kind() == std::io::ErrorKind::NotFound => true,
        _ => false,
    };
    if !unchanged {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "{} changed concurrently while its MCP config was being merged; refusing overwrite",
            target.display()
        ));
    }
    commit_atomic_temp(&tmp, target)
}

/// File mtime as of `read_target_mtime` invocation. Returned as
/// `Option<SystemTime>` because the file may not exist yet (legitimate
/// for a first-time sync) — `None` means "no prior version to defend
/// against", and `atomic_write_checked` will not abort on absent files.
pub(crate) fn read_target_mtime(target: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(target).ok().and_then(|m| m.modified().ok())
}

/// Why an `atomic_write_checked` call failed.
#[derive(Debug)]
pub enum AtomicWriteCheckedError {
    /// Target was modified by a concurrent writer between the caller's
    /// read and our pre-rename re-check. Caller should NOT clobber —
    /// log a warning, drop the sync, and let the next tick retry.
    /// Real-world trigger: Claude Code itself rewriting `~/.claude.json`
    /// (cache, recents, mcpContextUris) while Kronn is mid-sync (cf.
    /// TD-20260427-host-sync-flock).
    ConcurrentWrite,
    /// Standard IO failure (permission, disk full, …).
    Io(String),
}

impl std::fmt::Display for AtomicWriteCheckedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AtomicWriteCheckedError::ConcurrentWrite => f.write_str(
                "concurrent writer detected (mtime moved) — sync aborted to preserve external edits",
            ),
            AtomicWriteCheckedError::Io(s) => write!(f, "io: {}", s),
        }
    }
}

// ─── HostMcpSync trait (TD-20260427-host-sync-trait) ────────────────────────
//
// Each Kronn-supported host CLI (Claude Code, Codex, Copilot, Gemini)
// has its own MCP config file with a different format and merge rule.
// The 4 `sync_*_global_config` functions used to duplicate the common
// orchestration (mtime snapshot, parent-dir creation, atomic write,
// success log, post-write chmod). The trait below centralises that
// orchestration so each CLI only owns its config-specific bits
// (path resolution, format-specific build, optional post-write).
//
// Adding a 5th CLI = one struct + one trait impl + one call in
// `sync_affected_projects`. No more 4× drift on cross-cutting
// concerns (workflow-run gate, mtime guard, write helper, log shape).

/// Plan returned by a `HostMcpSync::prepare` call. Carries everything
/// the generic driver needs to commit the write.
pub(crate) struct HostSyncPlan {
    /// Absolute path of the host config file on disk.
    pub path: PathBuf,
    /// Serialised file content, ready for `atomic_write_checked`.
    pub content: String,
    /// One-shot success log line ("Synced Codex global config (3 MCP servers)").
    pub summary: String,
    /// Mtime captured BEFORE the impl read the existing file. Used by
    /// the driver's `atomic_write_checked` to abort on concurrent edits.
    pub pre_mtime: Option<std::time::SystemTime>,
}

/// A Kronn-managed outbound sync to a host CLI's global MCP config file.
///
/// Implementations encapsulate the format (TOML for Codex,
/// scope-aware JSON for Claude, flat JSON for Copilot/Gemini) and any
/// special handling of the "no Kronn entries" case (Codex clears
/// `mcp_servers`; Copilot deletes the file; Claude/Gemini wipe Kronn
/// entries and write the rest back). The driver `run_host_sync`
/// handles the cross-cutting concerns.
pub(crate) trait HostMcpSync: Sync {
    /// Human-readable label for log lines and the workflow-run gate.
    fn label(&self) -> &'static str;

    /// Build the next file content from the current DB state and the
    /// existing on-disk config (which the impl loads itself).
    ///
    /// Returns `Some(plan)` when there is something to write.
    /// Returns `None` when:
    /// - there's nothing to sync (empty Kronn state + file absent);
    /// - the impl already handled the change itself (e.g. Copilot
    ///   removes the file when no MCPs remain);
    /// - the existing file is corrupt (already backed up + warn-logged
    ///   by the impl's loader).
    ///
    /// Impls log their own warn / error lines for parsing / serialisation
    /// failures — the driver doesn't try to second-guess them.
    fn prepare(
        &self,
        conn: &rusqlite::Connection,
        secret: &str,
    ) -> Option<HostSyncPlan>;

    /// Hook to run after a successful atomic write (e.g. `chmod 0600`).
    /// Default: no-op.
    fn post_write(&self, _path: &Path) {}
}

/// Generic orchestrator for `HostMcpSync` impls. Centralises:
/// 1. parent-dir creation
/// 2. mtime-checked atomic write (TD-host-sync-flock)
/// 3. standardised log shape (helped by `write_host_config_checked`)
/// 4. post-write hook on success
///
/// The workflow-run gate (TD-host-sync-workflow-race) is handled at
/// `sync_affected_projects` entry, OUT of this loop, so all four CLIs
/// back off in lockstep.
pub(crate) fn run_host_sync(
    t: &dyn HostMcpSync,
    conn: &rusqlite::Connection,
    secret: &str,
) {
    let label = t.label();
    let plan = match t.prepare(conn, secret) {
        Some(p) => p,
        None => return,
    };
    if let Some(parent) = plan.path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::error!(
                "{} sync: cannot create dir {}: {}",
                label,
                parent.display(),
                e
            );
            return;
        }
    }
    if write_host_config_checked(
        &plan.path,
        &plan.content,
        plan.pre_mtime,
        label,
        &plan.summary,
    ) {
        t.post_write(&plan.path);
    }
}

/// One-shot helper that wraps `atomic_write_checked` with the
/// standardised log-line shape used by every host-sync function
/// (`sync_codex_global_config`, `sync_copilot_global_config`,
/// `sync_claude_global_config`, `sync_gemini_global_config`).
///
/// Centralising the post-write reporting means a future change (e.g.
/// adding `flock`, swapping to a metric counter, etc.) lives in one
/// place instead of four.
///
/// `success_msg` is logged at `info` on `Ok` — caller passes the
/// already-formatted line ("Synced Codex global config (3 MCP servers)").
/// `target_label` is used in the deferred / IO error lines so the
/// operator can tell which sync is whining.
///
/// Returns `true` iff the write actually committed — callers that need
/// to do post-success work (e.g. chmod 0600 the new file) can branch
/// without re-stating the file. A `false` return doesn't differentiate
/// concurrent-write from IO failure — the log line already did.
pub(crate) fn write_host_config_checked(
    target: &Path,
    content: &str,
    expected_mtime: Option<std::time::SystemTime>,
    target_label: &str,
    success_msg: &str,
) -> bool {
    match atomic_write_checked(target, content, expected_mtime) {
        Ok(()) => {
            tracing::info!("{}", success_msg);
            true
        }
        Err(AtomicWriteCheckedError::ConcurrentWrite) => {
            tracing::warn!(
                "{} sync aborted: {} was modified concurrently. Skipping this tick.",
                target_label,
                target.display()
            );
            false
        }
        Err(AtomicWriteCheckedError::Io(e)) => {
            tracing::error!(
                "{} sync: atomic_write {}: {}",
                target_label,
                target.display(),
                e
            );
            false
        }
    }
}

/// Atomic write with a CAS-style mtime guard.
///
/// `expected_mtime`:
/// - `None`         → no prior version (first install) → write unconditionally.
/// - `Some(stamp)`  → caller observed `stamp` BEFORE reading the file.
///   We re-stat the target right before the rename; if its mtime moved
///   past `stamp`, abort with `ConcurrentWrite`. The caller's pending
///   content is dropped — we'd rather miss a sync tick than overwrite
///   the user's running CLI state (cf. TD-20260427-host-sync-flock).
///
/// Pure mtime check (no `flock`): the documented threat is Claude Code /
/// Gemini CLI racing Kronn over their own config files. Both write via
/// rename, both bump mtime. mtime monotonicity is enough to catch the
/// race; flock would only help against concurrent *Kronn* writers,
/// which don't happen (sync is sequential).
pub(crate) fn atomic_write_checked(
    target: &Path,
    content: &str,
    expected_mtime: Option<std::time::SystemTime>,
) -> Result<(), AtomicWriteCheckedError> {
    if let Some(prev) = expected_mtime {
        if let Some(curr) = read_target_mtime(target) {
            // Strictly-greater on purpose: equality means "untouched";
            // some filesystems clamp mtime resolution to 1 s, so the
            // common "user touched it, then we read, then we'd rename
            // 100 ms later" still catches because curr > prev when
            // they actually edited.
            if curr > prev {
                return Err(AtomicWriteCheckedError::ConcurrentWrite);
            }
        }
        // Target disappeared since the read → unusual but not a clobber.
        // Treat as concurrent write: a deletion is also a state change
        // we should not paper over.
        else {
            return Err(AtomicWriteCheckedError::ConcurrentWrite);
        }
    }
    atomic_write(target, content).map_err(AtomicWriteCheckedError::Io)
}

/// Ensure Claude Code's settings.local.json has all MCP server names in enabledMcpjsonServers.
/// Claude Code uses this list as a whitelist — MCPs not listed are silently ignored,
/// even when enableAllProjectMcpServers is true (known bug #24657).
/// Sync `enabledMcpjsonServers` in `.claude/settings.local.json` to match
/// the fully merged `.mcp.json` keys exactly. Callers must pass the post-merge
/// file, including manual user MCPs; otherwise this whitelist would silently
/// disable entries that the safe config merge deliberately preserved.
/// This fixes the naming migration
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
/// Decrypt a config's env for writing into an on-disk agent config file.
///
/// `Err` = the config EXPECTS secrets (`env_keys` non-empty) but decryption
/// failed — the caller MUST abort the whole file write, leaving the existing
/// on-disk file untouched. Both alternatives clobber previously-good secrets
/// (the 2026-06-30 incident class): writing the entry with `env: {}` strips
/// them, and skipping just the entry deletes it from the regenerated file.
/// A config with no expected keys degrades to an empty map.
pub(crate) fn decrypt_env_strict(
    config: &crate::models::McpConfig,
    secret: &str,
) -> Result<HashMap<String, String>, String> {
    match crate::db::mcps::decrypt_env(&config.env_encrypted, secret) {
        Ok(e) => Ok(e),
        Err(_) if config.env_keys.is_empty() => Ok(HashMap::new()),
        Err(e) => {
            let msg = format!(
                "MCP '{}': {} env key(s) configured but decryption failed ({}) — ABORTING this \
                 config-file write so existing on-disk secrets are not clobbered. Fix the \
                 encryption key (Settings → Security) or re-enter the MCP's env values.",
                config.label,
                config.env_keys.len(),
                e
            );
            tracing::error!("{msg}");
            Err(msg)
        }
    }
}

/// Availability of `command` for a HOST CLI. Codex/Copilot read their global
/// config on the host — natively the backend PATH IS the host PATH, but under
/// Docker it isn't: check the mounted host bins (KRONN_HOST_BIN), accepting
/// symlink entries (brew Cellar links dangle inside the container — see
/// rtk_detect). Absolute paths and an unset KRONN_HOST_BIN are unverifiable
/// from the container → keep the entry (a startup warning on the host beats
/// silently dropping a working MCP).
pub(crate) fn host_mcp_command_available(command: &str) -> bool {
    if !crate::core::env::is_docker() {
        return is_command_available(command);
    }
    if command.starts_with('/') || command.starts_with('.') {
        return true;
    }
    match std::env::var("KRONN_HOST_BIN") {
        Ok(hb) => std::env::split_paths(&hb).any(|dir| {
            let p = dir.join(command);
            p.exists() || p.symlink_metadata().is_ok()
        }),
        Err(_) => true,
    }
}

pub fn write_general_mcp_json(
    conn: &rusqlite::Connection,
    secret: &str,
    target_dir: &str,
) -> Result<(), String> {
    let _sync_guard = lock_project_config_sync();
    use crate::db;

    let configs = db::mcps::list_configs(conn).map_err(|e| e.to_string())?;
    let general_configs: Vec<_> = configs.into_iter().filter(|c| c.include_general).collect();
    // Don't early-return on empty: even when no user-configured MCPs are
    // marked include_general, we still want to ship the kronn-internal
    // introspection bridge into the workspace. Pre-fix the .mcp.json was
    // never written for general discussions on a vanilla install, so the
    // agent had no path to disc_meta / disc_get_message / disc_summarize.

    let servers = db::mcps::list_servers(conn).map_err(|e| e.to_string())?;
    let server_map: HashMap<String, &crate::models::McpServer> = servers.iter()
        .map(|s| (s.id.clone(), s)).collect();

    let mut mcp_servers = HashMap::new();
    for config in &general_configs {
        let server = match server_map.get(&config.server_id) {
            Some(s) => s,
            None => continue,
        };
        let env = decrypt_env_strict(config, secret)?;

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

    // Always write `.mcp.json` for general discussions, even when the
    // user has zero MCPs marked `include_general`. We still inject the
    // kronn-internal introspection bridge so the agent has access to
    // disc_meta / disc_get_message / disc_summarize.
    {
        // ── Claude Code: .mcp.json (stdio only) ──
        let stdio_only: HashMap<String, McpServerEntry> = mcp_servers.iter()
            .filter(|(_, e)| e.command.is_some())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut data = McpJsonFile { mcp_servers: stdio_only };
        // Inject the kronn-internal server alongside the user's MCPs.
        // The disc id is forwarded via the agent process env, so a
        // single .mcp.json works for every discussion sharing this
        // workspace.
        let injected = inject_kronn_internal(&mut data);
        // Skip the file write only if we have *nothing* to write
        // (no user MCPs AND the introspection script wasn't found on
        // disk — happens on Docker images that don't ship the script).
        if !data.mcp_servers.is_empty() || injected {
            write_owned_mcp_json_to_subpath(target_dir, ".mcp.json", &data)?;
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
        let mut kiro_data = McpJsonFile { mcp_servers: kiro_servers };
        inject_kronn_internal(&mut kiro_data);
        write_owned_mcp_json_to_subpath(target_dir, ".kiro/settings/mcp.json", &kiro_data)?;
        write_owned_mcp_json_to_subpath(target_dir, ".ai/mcp/mcp.json", &kiro_data)?;

        // ── Gemini: .gemini/settings.json (full, no localhost filter for desktop) ──
        let mut full_data = McpJsonFile { mcp_servers: mcp_servers.clone() };
        inject_kronn_internal(&mut full_data);
        write_owned_mcp_json_to_subpath(target_dir, ".gemini/settings.json", &full_data)?;

        // ── Vibe: .vibe/config.toml ──
        let server_map_owned: HashMap<String, &crate::models::McpServer> = server_map;
        sync_vibe_project_config(target_dir, &general_configs, &server_map_owned, secret, true)?;
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
    let _sync_guard = lock_project_config_sync();
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
    // Pre-compute the set of config ids that are "incomplete" (env_keys
    // declared but values missing/empty, or cipher unreadable). We skip
    // these at write time so the agent doesn't choke on a broken MCP at
    // startup. The same list is exposed via `McpOverview` so the UI can
    // surface it as a warning to the operator.
    let incomplete_ids: std::collections::HashSet<String> =
        find_incomplete_configs(&configs, &server_map, secret)
            .into_iter()
            .map(|i| i.config_id)
            .collect();

    // Build the McpJsonFile
    let mut mcp_servers = HashMap::new();
    let mut synced_config_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for config in &configs {
        let server = match server_map.get(&config.server_id) {
            Some(s) => s,
            None => continue,
        };
        if incomplete_ids.contains(&config.id) {
            tracing::warn!(
                "MCP '{}' (server: {}) has incomplete env config — skipping write to project-level files. \
                 Operator can complete the values from the MCPs page.",
                config.label, server.name
            );
            continue;
        }

        // Decrypt env — a failure with expected keys ABORTS the whole sync:
        // both "write with empty env" and "drop the entry from the regenerated
        // file" clobber the good secrets already on disk (2026-06-30 class).
        let env = decrypt_env_strict(config, secret)?;

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
        // No user MCPs linked — but the built-in `kronn-internal` introspection
        // bridge must STILL be available to project-bound discussions. So we
        // don't blank the project: we write `.mcp.json` (+ Kiro/Gemini/.ai
        // variants) containing ONLY kronn-internal. Mirrors
        // `write_general_mcp_json` ("always write, even when only the injected
        // bridge is present"). Without this, a project with no other MCPs lost
        // kronn-internal entirely (the file was deleted) — reported 2026-06-26.
        if write_kronn_internal_only(&project.path)? {
            tracing::info!("Synced kronn-internal-only MCP configs for {} (no user MCPs)", project.path);
        } else {
            // Bridge path unresolved: remove only entries whose ownership was
            // previously recorded by Kronn. User settings and manual MCPs stay.
            let empty = McpJsonFile { mcp_servers: HashMap::new() };
            for filename in &[".mcp.json", ".kiro/settings/mcp.json", ".gemini/settings.json", ".ai/mcp/mcp.json"] {
                write_owned_mcp_json_to_subpath(&project.path, filename, &empty)?;
            }
        }
        // Vibe has no kronn-internal bridge. Remove only sidecar-owned entries.
        sync_vibe_project_config(&project.path, &configs, &server_map, secret, true)?;
    } else {
        // ── Claude Code: .mcp.json ──
        // Claude Code only supports stdio servers in .mcp.json.
        // SSE/Streamable entries (with only "url", no "command") break the schema
        // validation and cause Claude Code to reject the ENTIRE file → no MCPs at all.
        let stdio_only: HashMap<String, McpServerEntry> = mcp_servers.iter()
            .filter(|(_, entry)| entry.command.is_some())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut claude_data = McpJsonFile { mcp_servers: stdio_only };
        // Inject kronn-internal so introspection tools are available
        // for project-bound discussions too. Disc id is forwarded via
        // the agent process env (KRONN_DISCUSSION_ID) — see runner.rs.
        inject_kronn_internal(&mut claude_data);
        write_owned_mcp_json_to_subpath(&project.path, ".mcp.json", &claude_data)?;
        ensure_gitignore(&project.path, ".mcp.json");
        tracing::info!("Synced .mcp.json for {} ({} stdio MCPs)", project.path, claude_data.mcp_servers.len());

        // ── Claude Code settings.local.json: keep enabledMcpjsonServers in sync ──
        let merged_claude = read_mcp_json(&project.path).unwrap_or_else(|| claude_data.clone());
        sync_claude_enabled_servers(&project.path, &merged_claude.mcp_servers);

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
        sync_vibe_project_config(&project.path, &configs, &server_map, secret, true)?;

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
        let mut kiro_data = McpJsonFile { mcp_servers: kiro_servers };
        // Inject the introspection bridge so Kiro and the .ai/mcp variant
        // get the same kronn-internal entry as Claude Code's .mcp.json.
        let kiro_excluded_count = data.mcp_servers.len() - kiro_data.mcp_servers.len();
        inject_kronn_internal(&mut kiro_data);

        // ── Kiro: .kiro/settings/mcp.json ──
        write_owned_mcp_json_to_subpath(&project.path, ".kiro/settings/mcp.json", &kiro_data)?;
        ensure_gitignore(&project.path, ".kiro/settings/");
        tracing::info!("Synced .kiro/settings/mcp.json for {} ({} servers, {} excluded)",
            project.path, kiro_data.mcp_servers.len(), kiro_excluded_count);

        // ── Gemini CLI: .gemini/settings.json (same JSON format as Claude) ──
        let mut gemini_data = data.clone();
        inject_kronn_internal(&mut gemini_data);
        write_owned_mcp_json_to_subpath(&project.path, ".gemini/settings.json", &gemini_data)?;
        ensure_gitignore(&project.path, ".gemini/");
        tracing::info!("Synced .gemini/settings.json for {}", project.path);

        // ── Kiro (new format): .ai/mcp/mcp.json ──
        write_owned_mcp_json_to_subpath(&project.path, ".ai/mcp/mcp.json", &kiro_data)?;
        ensure_gitignore(&project.path, ".ai/mcp/");
        tracing::info!("Synced .ai/mcp/mcp.json for {}", project.path);

        // NOTE: per-MCP usage-context files (`<docs>/operations/mcp-servers/<slug>.md`)
        // are NO LONGER auto-generated here. The plugin's usage knowledge already
        // reaches Kronn-launched agents via the injected `=== AVAILABLE APIs ===`
        // block (built from `api_spec`), so materialising the registry
        // `default_context` to disk was redundant — and the auto-files cluttered
        // project docs and drifted from the registry. The per-MCP context remains
        // MANUALLY editable via the McpPage drawer (read/write_mcp_context API);
        // we just stop seeding it automatically.
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

/// Ensure all agent redirector files exist in a project that has a
/// docs folder (post-pivot `docs/`, alt `doc/`, or legacy `ai/`).
/// Non-destructive: only creates missing files, never overwrites existing ones.
/// Called during MCP sync to keep redirectors up-to-date when Kronn adds new agent support.
fn ensure_redirectors(project_path: &str) {
    let resolved = resolve_host_path(project_path);
    let project_dir = Path::new(&resolved);

    // Only for projects that have ANY docs folder. Without a docs/
    // (or ai/) at all there's no point dropping CLAUDE.md redirectors.
    let docs_dir = crate::core::scanner::detect_docs_dir(project_dir);
    if !docs_dir.is_dir() {
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

pub(crate) fn merge_vibe_config(
    project_path: &str,
    file: &Path,
    entries: &[VibeMcpEntry],
    previously_owned: &BTreeSet<String>,
) -> Result<BTreeSet<String>, String> {
    refuse_project_symlink_chain(project_path, file, "Vibe config path")?;
    let existed = file.exists();
    if !existed && entries.is_empty() {
        return Ok(BTreeSet::new());
    }
    let observed_content;
    let mut root = if existed {
        let raw = std::fs::read_to_string(file)
            .map_err(|e| format!("Failed to read existing Vibe config {}: {e}", file.display()))?;
        observed_content = Some(raw.as_bytes().to_vec());
        match toml::from_str::<toml::Table>(&raw) {
            Ok(table) => table,
            Err(e) => {
                backup_before_refusal(project_path, file)?;
                return Err(format!("Invalid existing Vibe config {}: {e}; refusing overwrite", file.display()));
            }
        }
    } else {
        observed_content = None;
        toml::Table::new()
    };

    let rendered = toml::Value::try_from(VibeConfig { mcp_servers: entries.to_vec() })
        .map_err(|e| format!("Failed to serialize Vibe MCP entries: {e}"))?;
    let desired: BTreeMap<String, toml::Value> = rendered
        .get("mcp_servers")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            value.get("name").and_then(toml::Value::as_str)
                .map(|name| (name.to_string(), value.clone()))
        })
        .collect();

    let servers = root
        .entry("mcp_servers".to_string())
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    let Some(existing) = servers.as_array_mut() else {
        backup_before_refusal(project_path, file)?;
        return Err(format!("Existing Vibe config {} has a non-array mcp_servers field; refusing overwrite", file.display()));
    };

    existing.retain(|value| {
        value.get("name").and_then(toml::Value::as_str)
            .map(|name| !previously_owned.contains(name) || desired.contains_key(name))
            .unwrap_or(true)
    });

    let mut now_owned = BTreeSet::new();
    for (name, wanted) in desired {
        let found = existing.iter().position(|value| {
            value.get("name").and_then(toml::Value::as_str) == Some(name.as_str())
        });
        match found {
            Some(index) if previously_owned.contains(&name) || existing[index] == wanted => {
                existing[index] = wanted;
                now_owned.insert(name);
            }
            Some(_) => {
                tracing::warn!(
                    "Vibe MCP config collision in {} for '{}': preserving user-owned entry",
                    file.display(), name
                );
            }
            None => {
                existing.push(wanted);
                now_owned.insert(name);
            }
        }
    }

    if existed {
        // TOML serialization cannot preserve comments/formatting. Keep the
        // exact first pre-merge file in ignored, mode-0600 Kronn state.
        backup_before_refusal(project_path, file)?;
    }
    let content = toml::to_string_pretty(&toml::Value::Table(root))
        .map_err(|e| format!("Failed to serialize merged Vibe config {}: {e}", file.display()))?;
    let header = "# MCP section managed by Kronn; unrelated settings and manual MCPs are preserved.\n\n";
    atomic_write_if_unchanged(
        file,
        &format!("{header}{content}"),
        observed_content.as_deref(),
    )?;
    secure_file_0600(file)?;
    Ok(now_owned)
}

/// Write .vibe/config.toml for a project with its MCP servers.
fn sync_vibe_project_config(
    project_path: &str,
    configs: &[crate::models::McpConfig],
    server_map: &HashMap<String, &crate::models::McpServer>,
    secret: &str,
    track_ownership: bool,
) -> Result<(), String> {
    let mut entries = Vec::new();

    for config in configs {
        let server = match server_map.get(&config.server_id) {
            Some(s) => s,
            None => continue,
        };

        // Abort the whole Vibe write on decrypt failure — see decrypt_env_strict.
        let env = decrypt_env_strict(config, secret)?;

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

    let mut state = if track_ownership {
        load_mcp_ownership(project_path)?
    } else {
        McpOwnershipState::default()
    };
    let previous = state.files.get(".vibe/config.toml").cloned().unwrap_or_default();
    if entries.is_empty() && !vibe_config.exists() && previous.is_empty() {
        return Ok(());
    }
    refuse_project_symlink_chain(project_path, &vibe_config, "Vibe config path")?;
    std::fs::create_dir_all(&vibe_dir)
        .map_err(|e| format!("Failed to create .vibe dir at {}: {e}", vibe_dir.display()))?;
    let now_owned = merge_vibe_config(project_path, &vibe_config, &entries, &previous)?;
    if track_ownership {
        if now_owned.is_empty() {
            state.files.remove(".vibe/config.toml");
        } else {
            state.files.insert(".vibe/config.toml".to_string(), now_owned);
        }
        save_mcp_ownership(project_path, &state)?;
    }
    ensure_gitignore(project_path, ".vibe/");
    tracing::info!("Safely merged .vibe/config.toml for {}", project_path);
    Ok(())
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
    // toml 1.x: `parse::<toml::Value>` was the 0.x idiom; the 1.x doc-
    // root is `toml::Table` which always represents a TOML document
    // (which by spec is always a table at root). Parse straight into
    // `Table` — equivalent to the old `Value::as_table().clone()` but
    // without the spurious "not a table" branch (impossible since
    // toml documents are tables by the TOML grammar).
    match content.parse::<toml::Table>() {
        Ok(t) => CodexLoadOutcome::Loaded(t),
        Err(e) => {
            let backup = rotate_backup(codex_config, BACKUP_ROTATION_SLOTS);
            tracing::error!(
                "Failed to parse Codex config {} ({}). Backed up to {} and aborting sync to preserve user data.",
                codex_config.display(),
                e,
                backup.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "<no-backup>".to_string())
            );
            CodexLoadOutcome::Aborted
        }
    }
}

/// HostMcpSync impl for OpenAI Codex CLI. TOML format, stdio-only,
/// merges with the existing `[model_providers]` / `[profiles]` etc.
/// `~/.codex/config.toml` (single global file — no per-project support).
pub(crate) struct CodexSync;

impl HostMcpSync for CodexSync {
    fn label(&self) -> &'static str { "Codex" }

    fn prepare(
        &self,
        conn: &rusqlite::Connection,
        secret: &str,
    ) -> Option<HostSyncPlan> {
        use crate::db;

        let all_configs = match db::mcps::list_configs(conn) {
            Ok(c) => c,
            Err(e) => { tracing::warn!("Failed to list configs for Codex sync: {}", e); return None; }
        };
        let servers = match db::mcps::list_servers(conn) {
            Ok(s) => s,
            Err(e) => { tracing::warn!("Failed to list servers for Codex sync: {}", e); return None; }
        };
        let server_map: HashMap<String, &crate::models::McpServer> = servers.iter()
            .map(|s| (s.id.clone(), s))
            .collect();

        // Build MCP entries (Codex only supports stdio transport)
        let mut mcp_entries: HashMap<String, CodexMcpEntry> = HashMap::new();
        for config in &all_configs {
            if !should_host_sync(config) { continue; }
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
                    tracing::debug!("Skipping non-stdio MCP '{}' for Codex (unsupported)", server.name);
                    continue;
                }
            };
            // Parity with the .mcp.json writer: a dead entry fails Codex's
            // startup with "No such file or directory" on every spawn.
            // Host-aware: this file is read by the HOST CLI, not the backend.
            if !host_mcp_command_available(&command) {
                tracing::warn!(
                    "MCP '{}' skipped for Codex: `{}` is not installed on this host — install it (e.g. `brew install uv` for uvx) then resync",
                    config.label, command
                );
                continue;
            }
            let env = match decrypt_env_strict(config, secret) {
                Ok(e) => e,
                // Abort the whole Codex sync plan — see decrypt_env_strict.
                Err(_) => return None,
            };
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

        // Inject the kronn-internal introspection bridge into Codex's
        // global config so every Codex spawn sees the disc_meta /
        // disc_get_message / disc_summarize tools. Codex 0.121 only
        // reads `~/.codex/config.toml` (not project-local .mcp.json),
        // so the global path is the only way to wire it up. When the
        // env var KRONN_DISCUSSION_ID isn't set (Codex run outside
        // Kronn), the bridge returns a structured MCP error per call
        // — graceful, no crash. The label is already a valid Codex
        // identifier (^[a-zA-Z0-9_-]+$) so no slugification needed.
        inject_kronn_internal_codex(&mut mcp_entries);

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
        detect_codex_config_drift(&codex_dir);

        // Mtime snapshot BEFORE the read so atomic_write_checked can detect
        // a concurrent writer and abort — see TD-host-sync-flock.
        let pre_mtime = read_target_mtime(&codex_config);

        let mut doc: toml::value::Table = match load_codex_config_for_merge(&codex_config) {
            CodexLoadOutcome::Loaded(t) => t,
            CodexLoadOutcome::Empty => toml::value::Table::new(),
            CodexLoadOutcome::Aborted => return None,
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

        let summary = format!("Synced Codex global config ({} MCP servers)", mcp_entries.len());
        let content = match toml::to_string_pretty(&doc) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to serialize Codex config: {}", e);
                return None;
            }
        };
        Some(HostSyncPlan { path: codex_config, content, summary, pre_mtime })
    }
}

/// HostMcpSync impl for Copilot CLI (`~/.copilot/mcp-config.json`).
/// Top-level JSON, stdio-only, removes the config file when no Kronn
/// entries remain (special-case in `prepare`, returns None).
pub(crate) struct CopilotSync;

impl HostMcpSync for CopilotSync {
    fn label(&self) -> &'static str { "Copilot" }

    fn prepare(
        &self,
        conn: &rusqlite::Connection,
        secret: &str,
    ) -> Option<HostSyncPlan> {
        use crate::db;

        let all_configs = match db::mcps::list_configs(conn) {
            Ok(c) => c,
            Err(e) => { tracing::warn!("Failed to list configs for Copilot sync: {}", e); return None; }
        };
        let servers = match db::mcps::list_servers(conn) {
            Ok(s) => s,
            Err(e) => { tracing::warn!("Failed to list servers for Copilot sync: {}", e); return None; }
        };
        let server_map: HashMap<String, &crate::models::McpServer> = servers.iter()
            .map(|s| (s.id.clone(), s))
            .collect();

        // Build mcpServers entries (stdio only — Copilot CLI doesn't support SSE)
        let mut mcp_servers: HashMap<String, McpServerEntry> = HashMap::new();
        for config in &all_configs {
            // Phase-3 host_sync filter (same as Codex above).
            if !should_host_sync(config) { continue; }
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

            // Same host-aware availability parity as the Codex writer above.
            if !host_mcp_command_available(&command) {
                tracing::warn!(
                    "MCP '{}' skipped for Copilot: `{}` is not installed on this host — install it then resync",
                    config.label, command
                );
                continue;
            }
            let env = match decrypt_env_strict(config, secret) {
                Ok(e) => e,
                // Abort the whole Copilot sync plan — see decrypt_env_strict.
                Err(_) => return None,
            };

            let key = config.label.clone();
            mcp_servers.insert(key, McpServerEntry {
                command: Some(command),
                args: Some(args),
                url: None,
                env,
            });
        }

        // Inject the kronn-internal introspection bridge into Copilot's
        // global config (same rationale as the Codex global injection):
        // Copilot CLI reads `~/.copilot/mcp-config.json`, never project-
        // local. The bridge returns a structured MCP error when run
        // outside Kronn (no KRONN_DISCUSSION_ID), so global injection
        // is safe.
        let mut copilot_file = McpJsonFile { mcp_servers };
        inject_kronn_internal(&mut copilot_file);
        let mcp_servers = copilot_file.mcp_servers;

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
            // Remove config file if no MCPs — bypasses the standard
            // write/atomic-write path because there's nothing to write.
            if config_path.exists() {
                let _ = std::fs::remove_file(&config_path);
                tracing::info!("Removed empty Copilot MCP config");
            }
            return None;
        }

        // Mtime snapshot for the concurrent-writer guard (TD-host-sync-flock).
        let pre_mtime = read_target_mtime(&config_path);

        let data = McpJsonFile { mcp_servers };
        let summary = format!(
            "Synced Copilot global config ({} MCP servers)",
            data.mcp_servers.len()
        );
        let content = match serde_json::to_string_pretty(&data) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to serialize Copilot MCP config: {}", e);
                return None;
            }
        };
        Some(HostSyncPlan { path: config_path, content, summary, pre_mtime })
    }
}

/// Sync .mcp.json for all projects that are affected by a config change.
/// Pass the config to determine which projects need updating.
///
/// Workflow-run gate (TD-20260427-host-sync-workflow-race) : if any run
/// is currently `Running` / `Pending`, we back off the host-config writes
/// rather than risk an agent already mid-spawn reading an inconsistent
/// `~/.claude.json` / `~/.gemini/settings.json`. The DB save still went
/// through (the caller already wrote it); the operator's next save (or
/// next sync trigger after the run completes) catches up the host side.
///
/// Test coverage: `db::workflows::has_running_run` is unit-tested (4
/// status combinations). The gate itself is a single conditional —
/// integration testing it would require standing up an `AppState`
/// plus a tempdir HOME and a `workflow_runs` row, roughly 30 lines for
/// one branch of behaviour. Skipped intentionally; code review owns the
/// wiring.
pub fn sync_affected_projects(
    conn: &rusqlite::Connection,
    project_ids: &[String],
    secret: &str,
) {
    if let Ok(true) = crate::db::workflows::has_running_run(conn) {
        tracing::warn!(
            "MCP host sync deferred: a workflow run is currently active. \
             DB state is up-to-date; host configs will be re-synced on the \
             next change or after the run finishes (re-save to force)."
        );
        return;
    }
    // Sync per-project configs (Claude Code .mcp.json + Vibe .vibe/config.toml)
    for pid in project_ids {
        if let Err(e) = sync_project_mcps_to_disk(conn, pid, secret) {
            tracing::warn!("Failed to sync MCP configs for project {}: {}", pid, e);
        }
    }
    // One-shot host-binary check across all syncing configs. Surfaces
    // "uvx not in PATH" / "glab missing" issues at sync time instead
    // of "Failed to connect" at MCP-spawn time. See TD-20260429
    // recommendation 5 + `kronn doctor` for the user-facing version.
    warn_missing_host_binaries(conn);

    // Sync global host-CLI configs (once, not per-project). Each function
    // filters by `host_sync ∈ {GlobalOnly, MirrorAll}` and merges with
    // existing entries to preserve user-managed (non-Kronn) MCPs.
    // Iterate through every registered HostMcpSync impl. Adding a 5th
    // CLI = one more entry in this slice; everything else (mtime guard,
    // workflow-run gate above, log shape) flows through `run_host_sync`.
    let registry: &[&dyn HostMcpSync] = &[&CodexSync, &CopilotSync, &ClaudeSync, &GeminiSync];
    for sync in registry {
        run_host_sync(*sync, conn, secret);
    }
}

/// Walk the host_sync-enabled configs and, for each Stdio command, check
/// whether the binary is reachable on the host's PATH. Logs one warn per
/// missing binary (deduped) so the operator sees "uvx not in host PATH"
/// at config-save time instead of "Failed to connect" at agent-spawn
/// time. Best-effort: a missing `find_binary` lookup is non-fatal.
///
/// Tracked under TD-20260429-uv-cache-uid-mismatch (item 5 of suggested
/// directions). See also `kronn doctor` for the operator-side check.
fn warn_missing_host_binaries(conn: &rusqlite::Connection) {
    use crate::db;

    let configs = match db::mcps::list_configs(conn) {
        Ok(c) => c,
        Err(_) => return,
    };
    let servers = match db::mcps::list_servers(conn) {
        Ok(s) => s,
        Err(_) => return,
    };
    let server_map: HashMap<String, &crate::models::McpServer> = servers
        .iter()
        .map(|s| (s.id.clone(), s))
        .collect();

    let mut commands_seen: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for config in &configs {
        if !should_host_sync(config) {
            continue;
        }
        let server = match server_map.get(&config.server_id) {
            Some(s) => *s,
            None => continue,
        };
        let cmd = match &server.transport {
            McpTransport::Stdio { command, .. } => command.clone(),
            _ => continue, // SSE / Streamable / ApiOnly don't fork a host binary
        };
        // Skip absolute paths — operator owns the path, we trust it.
        if cmd.starts_with('/') || cmd.starts_with('.') {
            continue;
        }
        commands_seen.insert(cmd);
    }

    for cmd in commands_seen {
        if crate::agents::find_binary(&cmd).is_none() {
            tracing::warn!(
                target: "kronn::host_sync",
                "MCP host-sync writer: command `{}` not found on host PATH \
                 (binary missing in /host-bin/{{global,local,npm}}). \
                 The Kronn-managed entry will be written, but the host CLI \
                 will fail to launch this MCP until `{0}` is installed. \
                 Run `kronn doctor` for a complete check.",
                cmd
            );
        }
    }
}

// ─── Phase-3 host-sync helpers ───────────────────────────────────────────────

/// Whether a config opts in to outbound host sync. Anything other than
/// `None` means Kronn should write it to the relevant CLI config file.
pub(crate) fn should_host_sync(config: &crate::models::McpConfig) -> bool {
    use crate::models::HostSyncMode;
    matches!(config.host_sync, HostSyncMode::GlobalOnly | HostSyncMode::MirrorAll)
}

/// Best-effort `chmod 0600` on Unix. Silent no-op on Windows.
/// Claude/Gemini home files contain user secrets — match their default
/// permissions so a successful sync doesn't downgrade security.
fn ensure_user_only_perms(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            tracing::debug!("chmod 0600 failed on {}: {} (ignored)", path.display(), e);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path; // silence unused-var warning
    }
}

/// Resolve a path under the user's HOME, mirroring `sync_codex_global_config`.
fn resolve_home_subpath(subpath: &str) -> PathBuf {
    if let Ok(host_home) = std::env::var("KRONN_HOST_HOME") {
        return PathBuf::from(format!("{}/{}", host_home, subpath));
    }
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(|h| PathBuf::from(format!("{}/{}", h, subpath)))
        .unwrap_or_else(|_| directories::BaseDirs::new()
            .map(|d| d.home_dir().join(subpath))
            .unwrap_or_else(|| {
                tracing::warn!("Cannot determine home directory — using /tmp/{}", subpath);
                PathBuf::from(format!("/tmp/{}", subpath))
            }))
}

/// Build a JSON entry for a config + decrypted env, with the `_kronn`
/// marker. Returns `None` when the transport is `ApiOnly` (skipped).
fn build_kronn_managed_json_entry(
    config: &crate::models::McpConfig,
    server: &crate::models::McpServer,
    secret: &str,
    use_http_url_for_streamable: bool,
) -> Result<Option<serde_json::Value>, String> {
    use crate::models::McpTransport;
    // Err = decrypt failure with expected keys → the caller must abort its
    // whole host-config write (see decrypt_env_strict).
    let env = decrypt_env_strict(config, secret)?;

    let mut obj = serde_json::Map::new();
    match &server.transport {
        McpTransport::Stdio { command, args } => {
            let final_args = config.args_override.clone().unwrap_or_else(|| args.clone());
            obj.insert("command".into(), serde_json::Value::String(command.clone()));
            obj.insert("args".into(), serde_json::Value::Array(
                final_args.into_iter().map(serde_json::Value::String).collect()
            ));
            if !env.is_empty() {
                let env_obj: serde_json::Map<String, serde_json::Value> = env.into_iter()
                    .map(|(k, v)| (k, serde_json::Value::String(v)))
                    .collect();
                obj.insert("env".into(), serde_json::Value::Object(env_obj));
            }
        }
        McpTransport::Sse { url } => {
            obj.insert("type".into(), serde_json::Value::String("sse".into()));
            obj.insert("url".into(), serde_json::Value::String(url.clone()));
        }
        McpTransport::Streamable { url } => {
            // Gemini prefers `httpUrl`; Claude uses `type: "http" + url`.
            // Caller picks the convention via use_http_url_for_streamable.
            if use_http_url_for_streamable {
                obj.insert("httpUrl".into(), serde_json::Value::String(url.clone()));
            } else {
                obj.insert("type".into(), serde_json::Value::String("http".into()));
                obj.insert("url".into(), serde_json::Value::String(url.clone()));
            }
        }
        McpTransport::ApiOnly => return Ok(None),
    }

    let mut marker = serde_json::Map::new();
    marker.insert("managed".into(), serde_json::Value::Bool(true));
    marker.insert("config_id".into(), serde_json::Value::String(config.id.clone()));
    obj.insert("_kronn".into(), serde_json::Value::Object(marker));

    Ok(Some(serde_json::Value::Object(obj)))
}

/// Outcome of attempting to load+merge a JSON host config (Claude/Gemini/Copilot).
/// Mirrors `CodexLoadOutcome` for the TOML-based Codex flow.
#[derive(Debug)]
enum JsonLoadOutcome {
    Loaded(serde_json::Value),
    Empty,
    Aborted,
}

/// Read a JSON host config file. Backs up corrupt files (rotation N=5)
/// before returning `Aborted` so we never overwrite user data we couldn't parse.
fn load_json_config_for_merge(path: &Path) -> JsonLoadOutcome {
    if !path.exists() {
        return JsonLoadOutcome::Empty;
    }
    let raw = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Cannot read {}: {} — starting fresh", path.display(), e);
            return JsonLoadOutcome::Empty;
        }
    };
    if raw.trim().is_empty() {
        return JsonLoadOutcome::Empty;
    }
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(v) => JsonLoadOutcome::Loaded(v),
        Err(e) => {
            let backup = rotate_backup(path, BACKUP_ROTATION_SLOTS);
            tracing::error!(
                "Failed to parse {} ({}). Backed up to {} and aborting sync to preserve user data.",
                path.display(), e,
                backup.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "<no-backup>".to_string())
            );
            JsonLoadOutcome::Aborted
        }
    }
}

/// Number of `.kronn-backup.N` slots kept for each host config file.
/// Rotation: `.1` is the most recent, `.5` the oldest. On a fresh corruption,
/// `.5` is dropped, `.1..=.4` shift up by one, and the corrupt file lands at
/// `.1`. Five slots match common log-rotation defaults; bumping it requires
/// no migration (extra files are simply not written until the limit is hit).
const BACKUP_ROTATION_SLOTS: usize = 5;

/// Rotate the `.kronn-backup.N` files for `path` and copy the current file
/// to slot `.1` (the most recent). Returns the path of the new backup, or
/// `None` if the copy failed (we still return `None` rather than panic — the
/// caller has already decided to abort the sync, the backup is best-effort).
///
/// **Why rotation matters** — before 0.6.0 we kept a single `.kronn-backup`
/// slot which got overwritten on every parse failure. Two consecutive
/// corruptions = first backup lost. Rotation gives the user a recovery
/// window of N corruption events instead of one. See
/// `TD-20260427-host-sync-backup-rotation` (now resolved).
pub(crate) fn rotate_backup(path: &Path, max_n: usize) -> Option<PathBuf> {
    if max_n == 0 {
        return None;
    }
    // Build the suffix carrying the original extension so the backups are
    // self-documenting (e.g. `.claude.json.kronn-backup.1`, not
    // `.claude.kronn-backup.1`).
    let ext_with_suffix = path.extension()
        .and_then(|s| s.to_str())
        .map(|s| format!("{}.kronn-backup", s))
        .unwrap_or_else(|| "kronn-backup".to_string());
    let backup_n = |n: usize| path.with_extension(format!("{}.{}", ext_with_suffix, n));

    // 1. Drop the oldest slot (silent if absent).
    let _ = std::fs::remove_file(backup_n(max_n));

    // 2. Shift .max_n-1 → .max_n, ..., .1 → .2 (work from oldest to newest
    //    so we never overwrite a slot we haven't moved yet).
    for n in (1..max_n).rev() {
        let from = backup_n(n);
        if from.exists() {
            let _ = std::fs::rename(&from, backup_n(n + 1));
        }
    }

    // 3. Copy the current (corrupt) file to slot `.1`.
    let dest = backup_n(1);
    match std::fs::copy(path, &dest) {
        Ok(_) => Some(dest),
        Err(e) => {
            tracing::error!(
                "Failed to back up corrupt config {} → {}: {}",
                path.display(), dest.display(), e
            );
            None
        }
    }
}

/// Merge logic shared by Claude and Gemini: replace the `mcpServers` map
/// in `existing` with a 3-way merge:
///   - `_kronn`-managed entries with config_id matching a current Kronn
///     config → REPLACED (Kronn data wins)
///   - `_kronn`-managed entries with no matching current config → REMOVED
///     (orphan cleanup — config was deleted from Kronn)
///   - entries WITHOUT `_kronn` marker → PRESERVED as-is (user-managed)
///   - new Kronn configs → ADDED
fn merge_kronn_entries(
    existing: &mut serde_json::Value,
    kronn_entries: HashMap<String, serde_json::Value>,
    kronn_config_ids: &std::collections::HashSet<String>,
) {
    let root = match existing.as_object_mut() {
        Some(o) => o,
        None => return, // not a JSON object — caller should have guarded
    };

    // Get or create the mcpServers section
    let servers = root.entry("mcpServers")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let servers_obj = match servers.as_object_mut() {
        Some(o) => o,
        None => {
            tracing::warn!("Existing mcpServers is not an object — replacing");
            *servers = serde_json::Value::Object(serde_json::Map::new());
            servers.as_object_mut().unwrap()
        }
    };

    // Walk existing entries, keep user ones, drop orphan Kronn ones
    let to_remove: Vec<String> = servers_obj.iter()
        .filter_map(|(name, value)| {
            value.as_object()
                .and_then(|o| o.get("_kronn"))
                .and_then(|m| m.as_object())
                .and_then(|m| m.get("config_id"))
                .and_then(|v| v.as_str())
                .filter(|cid| !kronn_config_ids.contains(*cid))
                .map(|_| name.clone())
        })
        .collect();
    for name in to_remove {
        servers_obj.remove(&name);
        tracing::info!("host_sync: removed orphan Kronn entry '{}'", name);
    }

    // Replace/insert Kronn entries
    for (name, entry) in kronn_entries {
        servers_obj.insert(name, entry);
    }
}

/// HostMcpSync impl for Claude Code. Scope-aware JSON: routes Kronn
/// entries between top-level `mcpServers` (Claude `user` scope) and
/// `projects[<host-path>].mcpServers` (`local` scope) based on each
/// config's `is_global` / `project_ids`. Preserves user-managed
/// entries (no `_kronn` marker) and unrelated keys (cache, recents,
/// onboarding state).
pub(crate) struct ClaudeSync;

impl HostMcpSync for ClaudeSync {
    fn label(&self) -> &'static str { "Claude" }

    fn prepare(
        &self,
        conn: &rusqlite::Connection,
        secret: &str,
    ) -> Option<HostSyncPlan> {
        use crate::db;

        let all_configs = match db::mcps::list_configs(conn) {
            Ok(c) => c,
            Err(e) => { tracing::warn!("Claude sync: list_configs failed: {}", e); return None; }
        };
        let servers = match db::mcps::list_servers(conn) {
            Ok(s) => s,
            Err(e) => { tracing::warn!("Claude sync: list_servers failed: {}", e); return None; }
        };
        let server_map: HashMap<String, &crate::models::McpServer> = servers.iter()
            .map(|s| (s.id.clone(), s)).collect();

        let projects = match db::projects::list_projects(conn) {
            Ok(p) => p,
            Err(e) => { tracing::warn!("Claude sync: list_projects failed: {}", e); return None; }
        };
        let project_path_by_id: HashMap<String, String> = projects.iter()
            .map(|p| (p.id.clone(), p.path.clone())).collect();

        // Bucket Kronn-managed entries by their target scope.
        let mut top_level: HashMap<String, serde_json::Value> = HashMap::new();
        let mut by_project: HashMap<String /* abs project path */, HashMap<String, serde_json::Value>> = HashMap::new();
        let mut all_managed_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        for config in &all_configs {
            if !should_host_sync(config) { continue; }
            let server = match server_map.get(&config.server_id) { Some(s) => s, None => continue };
            let entry = match build_kronn_managed_json_entry(config, server, secret, false) {
                Ok(Some(e)) => e,
                Ok(None) => continue, // ApiOnly skipped
                // Decrypt failure: abort the whole Claude host sync so the
                // existing on-disk secrets are preserved (already logged).
                Err(_) => return None,
            };
            all_managed_ids.insert(config.id.clone());

            let goes_top_level = config.is_global || config.project_ids.is_empty();
            if goes_top_level {
                top_level.insert(config.label.clone(), entry);
            } else {
                for pid in &config.project_ids {
                    if let Some(path) = project_path_by_id.get(pid) {
                        by_project.entry(path.clone())
                            .or_default()
                            .insert(config.label.clone(), entry.clone());
                    }
                }
            }
        }

        let path = resolve_home_subpath(".claude.json");

        if top_level.is_empty() && by_project.is_empty() && !path.exists() {
            return None;
        }

        // Mtime snapshot — Claude Code rewrites this file (cache, recents,
        // mcpContextUris, onboarding state) on every session. The guard
        // prevents Kronn's read-modify-write from clobbering those edits.
        let pre_mtime = read_target_mtime(&path);

        let mut existing = match load_json_config_for_merge(&path) {
            JsonLoadOutcome::Loaded(v) => v,
            JsonLoadOutcome::Empty => serde_json::Value::Object(serde_json::Map::new()),
            JsonLoadOutcome::Aborted => return None,
        };

        if !existing.is_object() {
            tracing::error!("Claude config at {} is not a JSON object — aborting sync", path.display());
            return None;
        }

        let prev_count = count_kronn_entries_recursive(&existing);

        // Tree-wide cleanup of Kronn entries: top-level + every projects[*]
        // map. Anything Kronn-managed (marker `_kronn.managed=true`) is removed
        // before we re-insert at the correct current scope. This handles
        // scope migration (top-level ↔ per-project) for free.
        drop_all_kronn_entries(&mut existing);

        // Re-insert at the correct scope.
        if !top_level.is_empty() {
            merge_into_mcp_servers(&mut existing, top_level, &all_managed_ids, /*track_scope=*/None);
        }
        for (project_path, entries) in by_project {
            merge_into_project_mcp_servers(&mut existing, &project_path, entries, &all_managed_ids);
        }

        // Cleanup empty mcpServers maps left over from removed Kronn entries.
        prune_empty_mcp_servers(&mut existing);

        let new_count = count_kronn_entries_recursive(&existing);
        let summary = format!(
            "Synced Claude global config: {} Kronn entries (was {}) — top-level={}, per-project={} maps",
            new_count,
            prev_count,
            count_at_top_level(&existing),
            count_project_scopes(&existing)
        );
        let content = match serde_json::to_string_pretty(&existing) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Claude sync: serialize: {}", e);
                return None;
            }
        };
        Some(HostSyncPlan { path, content, summary, pre_mtime })
    }

    fn post_write(&self, path: &Path) {
        // chmod 0600 only on successful write — keep existing file's
        // perms untouched on a concurrent-write abort.
        ensure_user_only_perms(path);
    }
}

/// Walk the entire JSON tree (top-level mcpServers + each projects[*]
/// mcpServers) and remove any entry that carries the `_kronn` marker.
fn drop_all_kronn_entries(existing: &mut serde_json::Value) {
    let root = match existing.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // Top-level mcpServers
    if let Some(map) = root.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
        let drop_keys: Vec<String> = map.iter()
            .filter(|(_, v)| is_kronn_managed(v))
            .map(|(k, _)| k.clone())
            .collect();
        for k in drop_keys { map.remove(&k); }
    }

    // projects[*].mcpServers
    if let Some(projects) = root.get_mut("projects").and_then(|v| v.as_object_mut()) {
        for (_path, project_obj) in projects.iter_mut() {
            if let Some(map) = project_obj.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
                let drop_keys: Vec<String> = map.iter()
                    .filter(|(_, v)| is_kronn_managed(v))
                    .map(|(k, _)| k.clone())
                    .collect();
                for k in drop_keys { map.remove(&k); }
            }
        }
    }
}

/// True iff the value is a JSON object with `_kronn.managed = true`.
fn is_kronn_managed(value: &serde_json::Value) -> bool {
    value.as_object()
        .and_then(|o| o.get("_kronn"))
        .and_then(|m| m.as_object())
        .and_then(|m| m.get("managed"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Insert/replace Kronn-managed entries into top-level `mcpServers`.
/// `_managed_ids` is unused here because cleanup already happened
/// upstream; the parameter is kept for symmetry with the project variant.
fn merge_into_mcp_servers(
    existing: &mut serde_json::Value,
    kronn_entries: HashMap<String, serde_json::Value>,
    _managed_ids: &std::collections::HashSet<String>,
    _track_scope: Option<&str>,
) {
    let root = match existing.as_object_mut() {
        Some(o) => o,
        None => return,
    };
    let servers = root.entry("mcpServers")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let map = match servers.as_object_mut() {
        Some(m) => m,
        None => {
            *servers = serde_json::Value::Object(serde_json::Map::new());
            servers.as_object_mut().unwrap()
        }
    };
    for (k, v) in kronn_entries { map.insert(k, v); }
}

/// Insert/replace Kronn-managed entries into `projects[<project_path>].mcpServers`.
/// Creates the `projects` key + the project entry if absent.
fn merge_into_project_mcp_servers(
    existing: &mut serde_json::Value,
    project_path: &str,
    kronn_entries: HashMap<String, serde_json::Value>,
    _managed_ids: &std::collections::HashSet<String>,
) {
    let root = match existing.as_object_mut() {
        Some(o) => o,
        None => return,
    };
    let projects = root.entry("projects")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let projects_map = match projects.as_object_mut() {
        Some(m) => m,
        None => {
            *projects = serde_json::Value::Object(serde_json::Map::new());
            projects.as_object_mut().unwrap()
        }
    };
    let project = projects_map.entry(project_path.to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let project_obj = match project.as_object_mut() {
        Some(o) => o,
        None => {
            *project = serde_json::Value::Object(serde_json::Map::new());
            project.as_object_mut().unwrap()
        }
    };
    let servers = project_obj.entry("mcpServers".to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let map = match servers.as_object_mut() {
        Some(m) => m,
        None => {
            *servers = serde_json::Value::Object(serde_json::Map::new());
            servers.as_object_mut().unwrap()
        }
    };
    for (k, v) in kronn_entries { map.insert(k, v); }
}

/// Remove `mcpServers` keys that became empty after Kronn cleanup, so the
/// file doesn't accumulate `"mcpServers": {}` clutter for projects whose
/// only entries were Kronn-managed.
fn prune_empty_mcp_servers(existing: &mut serde_json::Value) {
    let root = match existing.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // Top-level
    let top_level_empty = root.get("mcpServers")
        .and_then(|v| v.as_object())
        .map(|m| m.is_empty())
        .unwrap_or(false);
    if top_level_empty {
        root.remove("mcpServers");
    }

    // projects[*]
    if let Some(projects) = root.get_mut("projects").and_then(|v| v.as_object_mut()) {
        for (_path, proj) in projects.iter_mut() {
            if let Some(proj_obj) = proj.as_object_mut() {
                let empty = proj_obj.get("mcpServers")
                    .and_then(|v| v.as_object())
                    .map(|m| m.is_empty())
                    .unwrap_or(false);
                if empty {
                    proj_obj.remove("mcpServers");
                }
            }
        }
    }
}

fn count_kronn_entries_recursive(existing: &serde_json::Value) -> usize {
    let mut total = 0;
    if let Some(map) = existing.get("mcpServers").and_then(|v| v.as_object()) {
        total += map.values().filter(|v| is_kronn_managed(v)).count();
    }
    if let Some(projects) = existing.get("projects").and_then(|v| v.as_object()) {
        for (_, proj) in projects {
            if let Some(map) = proj.get("mcpServers").and_then(|v| v.as_object()) {
                total += map.values().filter(|v| is_kronn_managed(v)).count();
            }
        }
    }
    total
}

fn count_at_top_level(existing: &serde_json::Value) -> usize {
    existing.get("mcpServers").and_then(|v| v.as_object())
        .map(|m| m.values().filter(|v| is_kronn_managed(v)).count())
        .unwrap_or(0)
}

fn count_project_scopes(existing: &serde_json::Value) -> usize {
    existing.get("projects").and_then(|v| v.as_object())
        .map(|projects| projects.values()
            .filter(|p| p.get("mcpServers").and_then(|v| v.as_object())
                .map(|m| m.values().any(is_kronn_managed))
                .unwrap_or(false))
            .count())
        .unwrap_or(0)
}

/// HostMcpSync impl for Gemini CLI (`~/.gemini/settings.json`).
/// Top-level JSON; uses `httpUrl` for Streamable HTTP transport
/// (Gemini convention — Claude calls the same field `url`).
pub(crate) struct GeminiSync;

impl HostMcpSync for GeminiSync {
    fn label(&self) -> &'static str { "Gemini" }

    fn prepare(
        &self,
        conn: &rusqlite::Connection,
        secret: &str,
    ) -> Option<HostSyncPlan> {
        use crate::db;

        let all_configs = match db::mcps::list_configs(conn) {
            Ok(c) => c,
            Err(e) => { tracing::warn!("Gemini sync: list_configs failed: {}", e); return None; }
        };
        let servers = match db::mcps::list_servers(conn) {
            Ok(s) => s,
            Err(e) => { tracing::warn!("Gemini sync: list_servers failed: {}", e); return None; }
        };
        let server_map: HashMap<String, &crate::models::McpServer> = servers.iter()
            .map(|s| (s.id.clone(), s)).collect();

        // Build Kronn-managed entries (filtered by host_sync ≠ None, ApiOnly skipped)
        let mut kronn_entries: HashMap<String, serde_json::Value> = HashMap::new();
        let mut kronn_config_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for config in &all_configs {
            if !should_host_sync(config) { continue; }
            let server = match server_map.get(&config.server_id) { Some(s) => s, None => continue };
            // Gemini: use `httpUrl` for Streamable HTTP.
            match build_kronn_managed_json_entry(config, server, secret, true) {
                Ok(Some(entry)) => {
                    kronn_entries.insert(config.label.clone(), entry);
                    kronn_config_ids.insert(config.id.clone());
                }
                Ok(None) => {} // ApiOnly skipped
                // Decrypt failure: abort the whole Gemini host sync (logged).
                Err(_) => return None,
            }
        }

        let path = resolve_home_subpath(".gemini/settings.json");

        // Empty case: if Kronn has nothing to sync AND the file doesn't exist, skip.
        // If the file exists, we still want to walk it to remove orphan Kronn entries.
        if kronn_entries.is_empty() && !path.exists() {
            return None;
        }

        // Mtime snapshot for the concurrent-writer guard (TD-host-sync-flock).
        let pre_mtime = read_target_mtime(&path);

        // Load existing (with backup-on-parse-fail safety)
        let mut existing = match load_json_config_for_merge(&path) {
            JsonLoadOutcome::Loaded(v) => v,
            JsonLoadOutcome::Empty => serde_json::Value::Object(serde_json::Map::new()),
            JsonLoadOutcome::Aborted => return None,
        };

        if !existing.is_object() {
            tracing::error!("Gemini config at {} is not a JSON object — aborting sync", path.display());
            return None;
        }

        let prev_kronn_count = count_kronn_entries(&existing);
        merge_kronn_entries(&mut existing, kronn_entries, &kronn_config_ids);
        let new_kronn_count = count_kronn_entries(&existing);

        let summary = format!(
            "Synced Gemini global config: {} Kronn entries (was {})",
            new_kronn_count, prev_kronn_count
        );
        let content = match serde_json::to_string_pretty(&existing) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Gemini sync: serialize: {}", e);
                return None;
            }
        };
        Some(HostSyncPlan { path, content, summary, pre_mtime })
    }

    fn post_write(&self, path: &Path) {
        ensure_user_only_perms(path);
    }
}

/// Count entries that carry a `_kronn.managed = true` marker.
fn count_kronn_entries(value: &serde_json::Value) -> usize {
    value.get("mcpServers")
        .and_then(|v| v.as_object())
        .map(|o| o.values()
            .filter(|v| v.as_object()
                .and_then(|e| e.get("_kronn"))
                .and_then(|m| m.as_object())
                .and_then(|m| m.get("managed"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false))
            .count())
        .unwrap_or(0)
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

    // no-follow (Codex A2): a symlinked .gitignore — dangling included —
    // would route the create+append OUTSIDE the project. Leave it intact.
    if std::fs::symlink_metadata(&gitignore).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
        tracing::warn!("{} is a symlink — skipping gitignore update", gitignore.display());
        return;
    }

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

/// Sub-path of the project's docs folder where per-MCP usage context lives.
/// The leading folder (`docs`/`doc`/`ai`) is resolved at call time via
/// `detect_docs_dir` — pre-fix this was hardcoded to `ai/...` which
/// silently dropped files into a non-canonical `<project>/ai/` even on
/// fresh post-0.7.1 projects whose docs live under `docs/`. Use
/// `mcp_context_dir(project_path)` to get the resolved absolute path.
const MCP_CONTEXT_SUBPATH: &str = "operations/mcp-servers";

/// Resolve the MCP context directory for a project, respecting whichever
/// docs convention (`docs/`/`doc/`/`ai/`) the project actually uses.
/// `resolved_project_path` is a host-resolved string path (the local
/// `resolve_host_path` returns String for legacy reasons).
fn mcp_context_dir(resolved_project_path: &str) -> PathBuf {
    crate::core::scanner::detect_docs_dir(Path::new(resolved_project_path))
        .join(MCP_CONTEXT_SUBPATH)
}

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

/// Whether a per-MCP context file is REAL user/agent-written guidance worth
/// injecting into agent prompts. Filters out (a) the scaffolding file itself
/// (`TEMPLATE.md`), (b) any file still carrying `{{...}}` placeholders — an
/// unfilled template injected verbatim wastes tokens and confuses the agent
/// (`# {{MCP_NAME}} — Context` was reaching live audit prompts), and (c) the
/// legacy `<!-- Examples: -->` boilerplate.
pub fn should_inject_mcp_context(file_name: &str, content: &str) -> bool {
    if file_name.eq_ignore_ascii_case("TEMPLATE.md") {
        return false;
    }
    if content.contains("{{") {
        return false;
    }
    !is_default_mcp_context(content)
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
    let ctx_dir = mcp_context_dir(&resolved);
    let mut contexts = Vec::new();
    if ctx_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&ctx_dir) {
            let mut files: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
                .collect();
            files.sort_by_key(|e| e.file_name());

            for entry in files {
                let name = entry.file_name();
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    if should_inject_mcp_context(&name.to_string_lossy(), &content) {
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
/// can't surface them. Credentials are deliberately NEVER rendered here:
/// authenticated calls must go through the `api_call` broker, which injects
/// them server-side without exposing them to the model, argv or logs.
///
/// `plugins_with_env` is decrypted by the caller so declared non-secret
/// config can be interpolated. Auth values may be present in that map but
/// this pure renderer must never read or emit them.
pub fn build_api_context_block(
    plugins_with_env: &[ActiveApiPlugin],
) -> String {
    use crate::models::{ApiAuthKind, ApiSpec};

    // Filter to plugins that actually have an ApiSpec — a hybrid plugin's
    // MCP side stays in .mcp.json (handled elsewhere), but its API side
    // surfaces here. Pure MCP plugins (api_spec = None) get skipped.
    let api_plugins: Vec<(&crate::models::McpServer, &std::collections::HashMap<String, String>, &ApiSpec)> =
        plugins_with_env.iter()
            .filter_map(|(s, _config_id, env)| s.api_spec.as_ref().map(|spec| (s, env, spec)))
            .collect();

    if api_plugins.is_empty() {
        return String::new();
    }

    let mut out = String::from("## REST APIs available\n\n");
    out.push_str("The following REST APIs are configured for this project. Use the `api_call` MCP tool — \
                  Kronn injects auth server-side and already knows each API's endpoints (call `mcp_list` to see them). \
                  Never reconstruct authenticated `curl` commands or ask for credentials: secret values are intentionally \
                  absent from this prompt.\n\n");

    for (server, env, spec) in api_plugins {
        out.push_str(&format!("### {}\n", server.name));
        // Base URLs may contain NON-SECRET config placeholders (tenant id,
        // workspace slug, …). Interpolate only keys explicitly declared as
        // `config_keys`: using the whole decrypted env here could leak an
        // auth value if a malformed plugin referenced it from the URL.
        let mut auth_env_keys: std::collections::HashSet<&str> = std::collections::HashSet::new();
        match &spec.auth {
            ApiAuthKind::ApiKeyQuery { env_key, .. }
            | ApiAuthKind::ApiKeyHeader { env_key, .. }
            | ApiAuthKind::Bearer { env_key }
            | ApiAuthKind::BasicApiKey { env_key } => {
                auth_env_keys.insert(env_key.as_str());
            }
            ApiAuthKind::Basic { user_env, password_env } => {
                auth_env_keys.insert(user_env.as_str());
                auth_env_keys.insert(password_env.as_str());
            }
            ApiAuthKind::OAuth2ClientCredentials {
                client_id_env,
                client_secret_env,
                ..
            } => {
                auth_env_keys.insert(client_id_env.as_str());
                auth_env_keys.insert(client_secret_env.as_str());
            }
            ApiAuthKind::TokenExchange { creds_env_keys, .. } => {
                auth_env_keys.extend(creds_env_keys.iter().map(String::as_str));
            }
            ApiAuthKind::None => {}
        }
        let public_env: std::collections::HashMap<String, String> = spec.config_keys.iter()
            .filter(|key| !auth_env_keys.contains(key.env_key.as_str()))
            .filter_map(|key| env.get(&key.env_key).map(|value| (key.env_key.clone(), value.clone())))
            .collect();
        let resolved_base = interpolate_env_template(&spec.base_url, &public_env);
        out.push_str(&format!("Base URL: `{}`\n", resolved_base));

        // Auth — describe only the injection SHAPE. Literal credentials,
        // exchanged tokens and templated secret headers must never cross the
        // broker boundary into model context (or its command-line argv).
        match &spec.auth {
            ApiAuthKind::ApiKeyQuery { param_name, .. } => {
                out.push_str(&format!("Auth: injected server-side by `api_call` as query parameter `{param_name}`.\n"));
            }
            ApiAuthKind::ApiKeyHeader { header_name, .. } => {
                out.push_str(&format!("Auth: injected server-side by `api_call` as header `{header_name}`.\n"));
            }
            ApiAuthKind::Bearer { .. } => {
                out.push_str("Auth: Bearer token injected server-side by `api_call`.\n");
            }
            ApiAuthKind::Basic { .. } => {
                out.push_str("Auth: HTTP Basic credentials injected server-side by `api_call`.\n");
            }
            ApiAuthKind::BasicApiKey { .. } => {
                out.push_str("Auth: Basic API key injected server-side by `api_call`.\n");
            }
            ApiAuthKind::OAuth2ClientCredentials { extra_headers, .. } => {
                out.push_str("Auth: OAuth2 token injected and refreshed server-side by `api_call`.\n");
                if !extra_headers.is_empty() {
                    let names = extra_headers.iter().map(|h| format!("`{}`", h.name)).collect::<Vec<_>>().join(", ");
                    out.push_str(&format!("Additional auth headers injected server-side: {names}.\n"));
                }
            }
            ApiAuthKind::TokenExchange { inject, .. } => {
                let location = match inject {
                    crate::models::TokenInjection::BearerHeader => "Bearer header".to_string(),
                    crate::models::TokenInjection::CustomHeader { name } => format!("header `{name}`"),
                    crate::models::TokenInjection::QueryParam { name } => format!("query parameter `{name}`"),
                };
                out.push_str(&format!("Auth: exchanged token injected server-side by `api_call` as {location}.\n"));
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
                out.push_str(&format!("- `${{ENV.{}}}`  ({})\n", k.env_key, k.description));
            }
        }

        // 2026-06-24 — LEAN endpoint summary. We used to dump EVERY endpoint
        // (+ a curl example) of EVERY active plugin into EVERY agent prompt
        // (all of Chartbeat / Adobe / JIRA / customs, on every run, even when
        // the agent never touches an API) — tens of KB of redundant context
        // per run. The `api_call` MCP broker already knows the endpoints and
        // injects auth server-side, and `mcp_list` lists them on demand. So we
        // surface only the COUNT + the FIRST endpoint as a shape example, and
        // point at those tools for the rest — not the whole catalogue.
        let n = spec.endpoints.len();
        out.push_str(&format!(
            "{} endpoint{} listed — invoke via the `api_call` MCP tool (Kronn injects auth), \
             or list the exact paths on demand with `mcp_list`.\n",
            n,
            if n == 1 { "" } else { "s" },
        ));
        if let Some(ep) = spec.endpoints.first() {
            out.push_str(&format!("e.g. `{} {}` — {}\n", ep.method, ep.path, ep.description));
        }
        // The listed endpoints are INDICATIVE, not an allow-list: the broker
        // forwards ANY path on this API (auth injected), so other endpoints
        // from the API's own docs work too — consult them for the full +
        // CURRENT surface (APIs add/change endpoints over time). ⚠ A path NOT
        // in the list defaults to GET, so for a WRITE on an undeclared path
        // you MUST pass `api_method` explicitly (e.g. POST).
        out.push_str(
            "The list is INDICATIVE (common calls), NOT exhaustive — any valid path on this API \
             works via `api_call`; check the API's own docs for the rest (and for updates). \
             For a non-GET on an UNLISTED path, set `api_method` explicitly (unlisted paths default to GET).\n",
        );

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
/// One active API plugin: (server, config_id, decrypted env). Exported as a
/// named alias so the signature of [`collect_active_api_plugins`] and the
/// matching `build_api_context_block` input both stay readable (and keep
/// clippy's `type_complexity` lint quiet on the return type).
///
/// The config id rides along (2026-06-10) so callers can disambiguate
/// MULTIPLE configs of the same server on one project. Pre-fix it was
/// dropped, and both the ApiCall executor (`matches_config` stub `true`)
/// and the streaming OAuth2 cache-key re-derive silently picked the FIRST
/// matching config — i.e. potentially the wrong credential.
pub type ActiveApiPlugin = (crate::models::McpServer, String, std::collections::HashMap<String, String>);

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
        out.push(((*server).clone(), config.id.clone(), env));
    }
    Ok(out)
}

/// Read a single MCP context file content.
pub fn read_mcp_context(project_path: &str, slug: &str) -> Option<String> {
    let resolved = resolve_host_path(project_path);
    let file = mcp_context_dir(&resolved).join(format!("{}.md", slug));
    std::fs::read_to_string(&file).ok()
}

/// Write a single MCP context file.
pub fn write_mcp_context(project_path: &str, slug: &str, content: &str) -> Result<(), String> {
    let resolved = resolve_host_path(project_path);
    let ctx_dir = mcp_context_dir(&resolved);
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
    let ctx_dir = mcp_context_dir(&resolved);
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
/// Walk all configs for a project (or global) and return the ones whose
/// declared `env_keys` aren't all populated — those would fail handshake
/// at agent boot and slow down the whole MCP startup. Used both at
/// sync-to-disk time (skip writing them) and by the API (surface to UI).
///
/// Decryption errors are surfaced separately with `missing_keys=[]` and
/// a "secrets unreadable" reason so the UI can suggest re-entering the
/// values rather than guessing which keys broke.
pub fn find_incomplete_configs(
    configs: &[crate::models::McpConfig],
    servers: &HashMap<String, &crate::models::McpServer>,
    secret: &str,
) -> Vec<crate::models::McpIncompleteConfig> {
    let mut out = Vec::new();
    for config in configs {
        // Configs with no declared env_keys can't be incomplete.
        if config.env_keys.is_empty() {
            continue;
        }
        let server_name = servers.get(&config.server_id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "(unknown server)".to_string());

        let env = match crate::db::mcps::decrypt_env(&config.env_encrypted, secret) {
            Ok(e) => e,
            Err(e) => {
                // Cipher unreadable — likely a key rotation or DB
                // corruption. Mark as incomplete with empty
                // missing_keys so the UI shows the generic recovery
                // hint.
                out.push(crate::models::McpIncompleteConfig {
                    config_id: config.id.clone(),
                    label: config.label.clone(),
                    server_name,
                    missing_keys: Vec::new(),
                    reason: format!("Secrets unreadable: {e}"),
                });
                continue;
            }
        };
        let missing: Vec<String> = config.env_keys.iter()
            .filter(|k| env.get(k.as_str()).map(|v| v.trim().is_empty()).unwrap_or(true))
            .cloned()
            .collect();
        if !missing.is_empty() {
            out.push(crate::models::McpIncompleteConfig {
                config_id: config.id.clone(),
                label: config.label.clone(),
                server_name,
                reason: format!("{} clé(s) requise(s) manquante(s) ou vide(s)", missing.len()),
                missing_keys: missing,
            });
        }
    }
    out
}

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

// ─── Tests for Phase-3 host_sync (merge logic) ───────────────────────────────

#[cfg(test)]
mod inject_context_tests {
    use super::should_inject_mcp_context;

    #[test]
    fn scaffolding_and_unfilled_templates_are_never_injected() {
        // The scaffolding file itself, whatever the casing.
        assert!(!should_inject_mcp_context("TEMPLATE.md", "# Real content, no placeholder"));
        assert!(!should_inject_mcp_context("template.md", "# Real content"));
        // Unfilled placeholders — `# {{MCP_NAME}} — Context` reached live
        // audit prompts verbatim.
        assert!(!should_inject_mcp_context("github.md", "# {{MCP_NAME}} — Context\n{{TOOL_1}}"));
        // Legacy default boilerplate.
        assert!(!should_inject_mcp_context("jira.md", "<!-- Examples: -->\n# Title\n> quote"));
        // Real user-written guidance passes.
        assert!(should_inject_mcp_context("github.md", "# GitHub — Context\nUse repo DocRoms/Kronn."));
    }
}

#[cfg(test)]
mod host_sync_tests {
    use super::*;
    use serial_test::serial;
    use std::collections::HashSet;

    fn kronn_entry(config_id: &str, command: &str) -> serde_json::Value {
        serde_json::json!({
            "command": command,
            "args": [],
            "_kronn": { "managed": true, "config_id": config_id }
        })
    }

    #[test]
    fn merge_preserves_user_managed_entries() {
        let mut existing = serde_json::json!({
            "mcpServers": {
                "user-fav": { "command": "user-cmd", "args": ["x"] }
            },
            "otherKey": "preserved"
        });

        let mut kronn = HashMap::new();
        kronn.insert("kronn-one".to_string(), kronn_entry("uuid-1", "npx"));
        let mut ids = HashSet::new();
        ids.insert("uuid-1".to_string());

        merge_kronn_entries(&mut existing, kronn, &ids);

        let servers = existing.get("mcpServers").unwrap().as_object().unwrap();
        assert!(servers.contains_key("user-fav"), "user entry preserved");
        assert!(servers.contains_key("kronn-one"), "kronn entry added");
        assert_eq!(existing.get("otherKey").unwrap().as_str(), Some("preserved"));
    }

    #[test]
    fn merge_replaces_kronn_managed_entries() {
        let mut existing = serde_json::json!({
            "mcpServers": {
                "linear": {
                    "command": "old-cmd",
                    "_kronn": { "managed": true, "config_id": "uuid-1" }
                }
            }
        });

        let mut kronn = HashMap::new();
        kronn.insert("linear".to_string(), kronn_entry("uuid-1", "new-cmd"));
        let mut ids = HashSet::new();
        ids.insert("uuid-1".to_string());

        merge_kronn_entries(&mut existing, kronn, &ids);

        let entry = existing.get("mcpServers").unwrap().get("linear").unwrap();
        assert_eq!(entry.get("command").unwrap().as_str(), Some("new-cmd"),
            "Kronn entry replaced with current data");
    }

    #[test]
    fn merge_orphan_cleanup_removes_stale_kronn_entries() {
        let mut existing = serde_json::json!({
            "mcpServers": {
                "deleted-from-kronn": {
                    "command": "x",
                    "_kronn": { "managed": true, "config_id": "uuid-gone" }
                },
                "still-managed": {
                    "command": "y",
                    "_kronn": { "managed": true, "config_id": "uuid-alive" }
                }
            }
        });

        let mut kronn = HashMap::new();
        kronn.insert("still-managed".to_string(), kronn_entry("uuid-alive", "y"));
        let mut ids = HashSet::new();
        ids.insert("uuid-alive".to_string());
        // "uuid-gone" intentionally NOT in ids → should be removed

        merge_kronn_entries(&mut existing, kronn, &ids);

        let servers = existing.get("mcpServers").unwrap().as_object().unwrap();
        assert!(!servers.contains_key("deleted-from-kronn"), "orphan removed");
        assert!(servers.contains_key("still-managed"), "current Kronn entry kept");
    }

    #[test]
    fn merge_creates_mcpservers_when_absent() {
        let mut existing = serde_json::json!({ "theme": "dark" });

        let mut kronn = HashMap::new();
        kronn.insert("new".to_string(), kronn_entry("uuid-1", "npx"));
        let mut ids = HashSet::new();
        ids.insert("uuid-1".to_string());

        merge_kronn_entries(&mut existing, kronn, &ids);

        let servers = existing.get("mcpServers").unwrap().as_object().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(existing.get("theme").unwrap().as_str(), Some("dark"));
    }

    #[test]
    fn merge_handles_corrupted_mcpservers_gracefully() {
        // mcpServers is a string by mistake — we replace it rather than crash
        let mut existing = serde_json::json!({
            "mcpServers": "this is wrong"
        });

        let mut kronn = HashMap::new();
        kronn.insert("x".to_string(), kronn_entry("uuid-1", "npx"));
        let mut ids = HashSet::new();
        ids.insert("uuid-1".to_string());

        merge_kronn_entries(&mut existing, kronn, &ids);
        let servers = existing.get("mcpServers").unwrap().as_object().unwrap();
        assert_eq!(servers.len(), 1);
    }

    #[test]
    fn count_kronn_entries_skips_user_entries() {
        let v = serde_json::json!({
            "mcpServers": {
                "kronn-1": { "_kronn": { "managed": true, "config_id": "u1" } },
                "user-1": { "command": "x" },
                "kronn-2": { "_kronn": { "managed": true, "config_id": "u2" } }
            }
        });
        assert_eq!(count_kronn_entries(&v), 2);
    }

    #[test]
    fn load_json_aborted_on_parse_failure() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "{ this is not valid").unwrap();
        match load_json_config_for_merge(tmp.path()) {
            JsonLoadOutcome::Aborted => {}
            other => panic!("Expected Aborted, got {:?}", other),
        }
        // Backup file should now exist at slot .1 (rotation N=5).
        let backup = tmp.path().with_extension(
            tmp.path().extension().and_then(|s| s.to_str()).map(|s| format!("{}.kronn-backup.1", s))
                .unwrap_or_else(|| "kronn-backup.1".to_string())
        );
        assert!(backup.exists(), "backup created at {}", backup.display());
        let _ = std::fs::remove_file(&backup); // cleanup
    }

    #[test]
    fn rotate_backup_keeps_at_most_n_slots() {
        // Use a TempDir + a config file inside (NamedTempFile would unlink
        // the source between iterations; we want a stable path).
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");

        // Simulate 7 corruptions in a row. With N=5, we should end up with
        // exactly slots .1..=.5 populated (no .6, no .7).
        for i in 0..7 {
            std::fs::write(&path, format!("{{ corrupt #{}", i)).unwrap();
            let result = rotate_backup(&path, 5);
            assert!(result.is_some(), "rotation #{} should succeed", i);
        }

        for n in 1..=5 {
            let p = path.with_extension(format!("json.kronn-backup.{}", n));
            assert!(p.exists(), "slot .{} must exist after 7 corruptions", n);
        }
        let p6 = path.with_extension("json.kronn-backup.6");
        assert!(!p6.exists(), "slot .6 must NOT exist (rotation cap)");
        let p7 = path.with_extension("json.kronn-backup.7");
        assert!(!p7.exists(), "slot .7 must NOT exist (rotation cap)");
    }

    #[test]
    fn rotate_backup_slot1_holds_most_recent() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");

        // First corruption — content "v1"
        std::fs::write(&path, "v1").unwrap();
        rotate_backup(&path, 3);
        // Second — content "v2" → v1 should shift to .2, v2 land at .1
        std::fs::write(&path, "v2").unwrap();
        rotate_backup(&path, 3);

        let slot1 = path.with_extension("json.kronn-backup.1");
        let slot2 = path.with_extension("json.kronn-backup.2");
        assert_eq!(std::fs::read_to_string(&slot1).unwrap(), "v2");
        assert_eq!(std::fs::read_to_string(&slot2).unwrap(), "v1");
    }

    #[test]
    fn rotate_backup_handles_no_extension() {
        // Path without an extension (rare but possible — defensive).
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("rcfile");
        std::fs::write(&path, "junk").unwrap();
        let backup = rotate_backup(&path, 5);
        assert!(backup.is_some(), "must handle ext-less paths");
        assert!(backup.unwrap().exists());
    }

    #[test]
    fn load_json_empty_for_missing_file() {
        let path = std::env::temp_dir().join("kronn-host-sync-nonexistent-12345");
        let _ = std::fs::remove_file(&path);
        match load_json_config_for_merge(&path) {
            JsonLoadOutcome::Empty => {}
            other => panic!("Expected Empty, got {:?}", other),
        }
    }

    #[test]
    fn build_entry_skips_api_only() {
        use crate::models::{HostSyncMode, McpConfig, McpServer, McpSource, McpTransport};
        let config = McpConfig {
            id: "u1".into(), server_id: "s1".into(), label: "test".into(),
            env_keys: vec![], env_encrypted: String::new(),
            args_override: None, is_global: false, include_general: true,
            config_hash: String::new(), project_ids: vec![],
            host_sync: HostSyncMode::GlobalOnly,
        };
        let server = McpServer {
            id: "s1".into(), name: "S".into(), description: String::new(),
            transport: McpTransport::ApiOnly,
            source: McpSource::Registry,
            api_spec: None,
        };
        assert!(build_kronn_managed_json_entry(&config, &server, "secret-not-used", false).unwrap().is_none());
    }

    #[test]
    fn decrypt_env_strict_aborts_on_undecryptable_expected_keys() {
        use crate::models::{HostSyncMode, McpConfig};
        // Secrets EXPECTED + garbage ciphertext → Err: the caller must abort
        // its file write instead of clobbering on-disk secrets (2026-06-30
        // incident class — the old code wrote env:{} silently).
        let mut config = McpConfig {
            id: "u1".into(), server_id: "s1".into(), label: "linear".into(),
            env_keys: vec!["API_TOKEN".into()],
            env_encrypted: "not-base64-not-ciphertext".into(),
            args_override: None, is_global: false, include_general: true,
            config_hash: String::new(), project_ids: vec![],
            host_sync: HostSyncMode::GlobalOnly,
        };
        assert!(decrypt_env_strict(&config, "0123456789abcdef0123456789abcdef").is_err());
        // No expected keys → same garbage degrades to an empty map (nothing to lose).
        config.env_keys = vec![];
        assert_eq!(decrypt_env_strict(&config, "0123456789abcdef0123456789abcdef").unwrap(), HashMap::new());
        // And the builder propagates the abort signal.
        config.env_keys = vec!["API_TOKEN".into()];
        let server = crate::models::McpServer {
            id: "s1".into(), name: "Linear".into(), description: String::new(),
            transport: crate::models::McpTransport::Stdio { command: "npx".into(), args: vec![] },
            source: crate::models::McpSource::Registry,
            api_spec: None,
        };
        assert!(build_kronn_managed_json_entry(&config, &server, "0123456789abcdef0123456789abcdef", false).is_err());
    }

    #[test]
    fn build_entry_marks_kronn_with_config_id() {
        use crate::models::{HostSyncMode, McpConfig, McpServer, McpSource, McpTransport};
        let config = McpConfig {
            id: "uuid-marker".into(), server_id: "s1".into(), label: "linear".into(),
            env_keys: vec![], env_encrypted: String::new(),
            args_override: None, is_global: false, include_general: true,
            config_hash: String::new(), project_ids: vec![],
            host_sync: HostSyncMode::GlobalOnly,
        };
        let server = McpServer {
            id: "s1".into(), name: "Linear".into(), description: String::new(),
            transport: McpTransport::Stdio { command: "npx".into(), args: vec![] },
            source: McpSource::Registry,
            api_spec: None,
        };
        let entry = build_kronn_managed_json_entry(&config, &server, "secret", false).unwrap().unwrap();
        let marker = entry.get("_kronn").unwrap();
        assert_eq!(marker.get("managed").unwrap().as_bool(), Some(true));
        assert_eq!(marker.get("config_id").unwrap().as_str(), Some("uuid-marker"));
    }

    // ─── Phase-3 refactor: Claude scope-aware writes ────────────────────────

    #[test]
    fn drop_all_kronn_entries_clears_top_level_and_per_project() {
        let mut v = serde_json::json!({
            "mcpServers": {
                "k1": { "_kronn": { "managed": true, "config_id": "u1" } },
                "user-a": { "command": "x" }
            },
            "projects": {
                "/p1": {
                    "mcpServers": {
                        "k2": { "_kronn": { "managed": true, "config_id": "u2" } },
                        "user-b": { "command": "y" }
                    }
                }
            }
        });
        drop_all_kronn_entries(&mut v);
        let top = v.get("mcpServers").unwrap().as_object().unwrap();
        assert!(!top.contains_key("k1"));
        assert!(top.contains_key("user-a"));
        let p1 = v.pointer("/projects/~1p1/mcpServers").unwrap().as_object().unwrap();
        assert!(!p1.contains_key("k2"));
        assert!(p1.contains_key("user-b"));
    }

    #[test]
    fn merge_into_project_creates_path_when_missing() {
        let mut v = serde_json::json!({});
        let mut entries = HashMap::new();
        entries.insert("linear".to_string(), serde_json::json!({"command": "x"}));
        let ids: HashSet<String> = ["u1".to_string()].into_iter().collect();
        merge_into_project_mcp_servers(&mut v, "/repo/abc", entries, &ids);
        let entry = v.pointer("/projects/~1repo~1abc/mcpServers/linear").unwrap();
        assert_eq!(entry.get("command").unwrap().as_str(), Some("x"));
    }

    #[test]
    fn count_kronn_entries_recursive_walks_both_scopes() {
        let v = serde_json::json!({
            "mcpServers": {
                "kronn-1": { "_kronn": { "managed": true, "config_id": "u1" } },
                "user-1": { "command": "x" }
            },
            "projects": {
                "/p1": {
                    "mcpServers": {
                        "kronn-2": { "_kronn": { "managed": true, "config_id": "u2" } }
                    }
                },
                "/p2": {
                    "mcpServers": {
                        "kronn-3": { "_kronn": { "managed": true, "config_id": "u3" } },
                        "kronn-4": { "_kronn": { "managed": true, "config_id": "u4" } }
                    }
                }
            }
        });
        assert_eq!(count_kronn_entries_recursive(&v), 4);
    }

    #[test]
    fn prune_empty_mcp_servers_removes_top_level_and_projects() {
        let mut v = serde_json::json!({
            "mcpServers": {},
            "theme": "dark",
            "projects": {
                "/p1": { "mcpServers": {} },
                "/p2": { "mcpServers": { "kept": { "command": "x" } } }
            }
        });
        prune_empty_mcp_servers(&mut v);
        assert!(v.get("mcpServers").is_none(), "top-level pruned");
        assert!(v.pointer("/projects/~1p1/mcpServers").is_none(), "p1 pruned");
        assert!(v.pointer("/projects/~1p2/mcpServers/kept").is_some(), "p2 kept");
        assert_eq!(v.get("theme").unwrap().as_str(), Some("dark"), "non-mcp keys preserved");
    }

    #[test]
    fn merge_into_top_level_inserts_and_replaces() {
        let mut v = serde_json::json!({
            "mcpServers": { "user-a": { "command": "user" } }
        });
        let mut entries = HashMap::new();
        entries.insert("kronn-1".to_string(), serde_json::json!({"command": "k"}));
        let ids = HashSet::new();
        merge_into_mcp_servers(&mut v, entries, &ids, None);
        let map = v.get("mcpServers").unwrap().as_object().unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("user-a"));
        assert!(map.contains_key("kronn-1"));
    }

    #[test]
    fn is_kronn_managed_detects_marker() {
        assert!(is_kronn_managed(&serde_json::json!({
            "command": "x",
            "_kronn": { "managed": true, "config_id": "u1" }
        })));
        assert!(!is_kronn_managed(&serde_json::json!({
            "command": "x"
        })));
        assert!(!is_kronn_managed(&serde_json::json!({
            "command": "x",
            "_kronn": { "managed": false }
        })));
        // Non-object values
        assert!(!is_kronn_managed(&serde_json::json!("string")));
    }

    #[test]
    #[serial]
    fn inject_kronn_internal_path_resolution() {
        // Pin the user-reported bug 2026-05-10: with KRONN_INTROSPECTION_PUBLIC_PATH
        // unset AND running in Docker (= `/app/scripts/...` only), Kronn used to
        // write a container-only path into project files. Host CLIs (kiro-cli,
        // claude on host) couldn't reach it and threw `Broken pipe (os error 32)`.
        //
        // Combined into a single test to avoid env-var race conditions when
        // cargo test runs unit tests in parallel.
        //
        // 1. Public-path env wins over Docker / dev fallbacks.
        let tmp = std::env::temp_dir().join(format!("kronn-pub-{}.py", std::process::id()));
        std::fs::write(&tmp, "#!/usr/bin/env python3\n").expect("write test fixture");
        // SAFETY: the env-var set/remove pair brackets the inject call we
        // care about; no other thread should see the set value because the
        // following remove_var happens before the next test in this function
        // observes it. Nested tests in the same suite are still in different
        // threads — this is a known limitation of std::env, accepted because
        // the alternative (serial_test crate) would balloon dev-only deps.
        unsafe {
            std::env::set_var("KRONN_INTROSPECTION_PUBLIC_PATH", &tmp);
        }
        let mut file = McpJsonFile { mcp_servers: HashMap::new() };
        let injected = inject_kronn_internal(&mut file);
        unsafe {
            std::env::remove_var("KRONN_INTROSPECTION_PUBLIC_PATH");
        }
        let _ = std::fs::remove_file(&tmp);
        assert!(injected, "injection must succeed when public env path exists");
        let entry = file.mcp_servers.get("kronn-internal").expect("entry written");
        let path = entry.args.as_ref().unwrap().first().unwrap();
        assert_eq!(path, &tmp.to_string_lossy().to_string());

        // 2. Stale entry removal — a prior Kronn build wrote `/app/scripts/...`.
        // When the public env is unset AND we're not in Docker (`.dockerenv`
        // absent on the test host), the native fallback returns the dev
        // `CARGO_MANIFEST_DIR/scripts/...` path — which IS valid on this fs
        // since cargo runs the suite from the source tree. Injection should
        // succeed; the path must point at an existing file.
        let mut file = McpJsonFile { mcp_servers: HashMap::new() };
        file.mcp_servers.insert("kronn-internal".into(), McpServerEntry {
            command: Some("python3".into()),
            args: Some(vec!["/app/scripts/disc-introspection-mcp.py".into()]),
            url: None,
            env: HashMap::new(),
        });
        let injected = inject_kronn_internal(&mut file);
        if injected {
            // Native Kronn — script reachable on the host, entry rewritten.
            let entry = file.mcp_servers.get("kronn-internal").expect("entry written");
            let path = entry.args.as_ref().unwrap().first().expect("path arg");
            assert!(std::path::Path::new(path).exists(), "written path must exist on this fs");
            assert_ne!(path, "/app/scripts/disc-introspection-mcp.py",
                "stale Docker-only path should have been replaced with a host-valid path");
        } else {
            // Docker-only Kronn (the user's bug case) — injection skipped,
            // stale entry pruned so host CLIs stop choking on it.
            assert!(!file.mcp_servers.contains_key("kronn-internal"),
                "stale `kronn-internal` entry should be removed when no shared path resolves");
        }
    }

    #[test]
    fn find_incomplete_configs_flags_missing_keys() {
        // Pin user-reported behaviour 2026-05-10: MCPs whose `env_keys`
        // are declared but values are empty/missing should be flagged
        // so the scanner can skip them at sync time and the UI can
        // surface a warning. Without this, every Kronn-spawned agent
        // tries to handshake with the broken MCP at boot, which slows
        // down the whole startup (Connection closed, OAuth invalid_client).
        use crate::models::{McpConfig, McpServer, McpSource, McpTransport, HostSyncMode};
        // 32 bytes hex-encoded (64 hex chars) — what `crypto::parse_secret` expects.
        let secret = &"a".repeat(64);
        // Config A: declares one env key, value provided → complete.
        let mut env_a = std::collections::HashMap::new();
        env_a.insert("FOO_TOKEN".to_string(), "real-value".to_string());
        let env_a_enc = crate::db::mcps::encrypt_env(&env_a, secret).unwrap();
        let cfg_a = McpConfig {
            id: "cfg-a".into(), server_id: "srv".into(), label: "Complete".into(),
            env_keys: vec!["FOO_TOKEN".into()],
            env_encrypted: env_a_enc,
            args_override: None, is_global: false, include_general: true,
            config_hash: String::new(), project_ids: vec![],
            host_sync: HostSyncMode::None,
        };
        // Config B: declares one env key, value EMPTY → incomplete.
        let mut env_b = std::collections::HashMap::new();
        env_b.insert("BAR_TOKEN".to_string(), "".to_string());
        let env_b_enc = crate::db::mcps::encrypt_env(&env_b, secret).unwrap();
        let cfg_b = McpConfig {
            id: "cfg-b".into(), server_id: "srv".into(), label: "Empty value".into(),
            env_keys: vec!["BAR_TOKEN".into()],
            env_encrypted: env_b_enc,
            args_override: None, is_global: false, include_general: true,
            config_hash: String::new(), project_ids: vec![],
            host_sync: HostSyncMode::None,
        };
        // Config C: declares two keys, only ONE provided → incomplete (1 missing).
        let mut env_c = std::collections::HashMap::new();
        env_c.insert("KEY1".to_string(), "val".to_string());
        let env_c_enc = crate::db::mcps::encrypt_env(&env_c, secret).unwrap();
        let cfg_c = McpConfig {
            id: "cfg-c".into(), server_id: "srv".into(), label: "Half".into(),
            env_keys: vec!["KEY1".into(), "KEY2".into()],
            env_encrypted: env_c_enc,
            args_override: None, is_global: false, include_general: true,
            config_hash: String::new(), project_ids: vec![],
            host_sync: HostSyncMode::None,
        };
        // Config D: no env_keys declared → never incomplete.
        let cfg_d = McpConfig {
            id: "cfg-d".into(), server_id: "srv".into(), label: "Open".into(),
            env_keys: vec![],
            env_encrypted: String::new(),
            args_override: None, is_global: false, include_general: true,
            config_hash: String::new(), project_ids: vec![],
            host_sync: HostSyncMode::None,
        };
        let server = McpServer {
            id: "srv".into(), name: "TestServer".into(), description: String::new(),
            transport: McpTransport::Stdio { command: "echo".into(), args: vec![] },
            source: McpSource::Registry, api_spec: None,
        };
        let mut server_map: HashMap<String, &McpServer> = HashMap::new();
        server_map.insert("srv".into(), &server);

        let configs = vec![cfg_a, cfg_b, cfg_c, cfg_d];
        let incomplete = find_incomplete_configs(&configs, &server_map, secret);

        // Only cfg-b and cfg-c should be flagged.
        assert_eq!(incomplete.len(), 2, "expected 2 incomplete configs, got: {:?}",
            incomplete.iter().map(|i| &i.config_id).collect::<Vec<_>>());
        let ids: HashSet<_> = incomplete.iter().map(|i| i.config_id.clone()).collect();
        assert!(ids.contains("cfg-b"));
        assert!(ids.contains("cfg-c"));
        // cfg-c lists KEY2 specifically as missing (KEY1 is fine).
        let cfg_c = incomplete.iter().find(|i| i.config_id == "cfg-c").unwrap();
        assert_eq!(cfg_c.missing_keys, vec!["KEY2".to_string()]);
        assert_eq!(cfg_c.server_name, "TestServer");
    }

    #[test]
    fn find_incomplete_configs_flags_decrypt_failure() {
        // Cipher unreadable (e.g. after key rotation) → flagged with
        // empty missing_keys + a "secrets unreadable" reason. The UI
        // should suggest re-entering values rather than guessing keys.
        use crate::models::{McpConfig, McpServer, McpSource, McpTransport, HostSyncMode};
        let cfg = McpConfig {
            id: "broken".into(), server_id: "srv".into(), label: "Broken".into(),
            env_keys: vec!["TOKEN".into()],
            env_encrypted: "definitely-not-valid-base64-or-cipher".into(),
            args_override: None, is_global: false, include_general: true,
            config_hash: String::new(), project_ids: vec![],
            host_sync: HostSyncMode::None,
        };
        let server = McpServer {
            id: "srv".into(), name: "S".into(), description: String::new(),
            transport: McpTransport::Stdio { command: "echo".into(), args: vec![] },
            source: McpSource::Registry, api_spec: None,
        };
        let mut server_map: HashMap<String, &McpServer> = HashMap::new();
        server_map.insert("srv".into(), &server);
        let incomplete = find_incomplete_configs(&[cfg], &server_map, "any-secret");
        assert_eq!(incomplete.len(), 1);
        assert!(incomplete[0].missing_keys.is_empty());
        assert!(incomplete[0].reason.starts_with("Secrets unreadable"),
            "got: {}", incomplete[0].reason);
    }

    #[test]
    fn build_entry_streamable_uses_httpurl_for_gemini() {
        use crate::models::{HostSyncMode, McpConfig, McpServer, McpSource, McpTransport};
        let config = McpConfig {
            id: "u1".into(), server_id: "s1".into(), label: "remote".into(),
            env_keys: vec![], env_encrypted: String::new(),
            args_override: None, is_global: false, include_general: true,
            config_hash: String::new(), project_ids: vec![],
            host_sync: HostSyncMode::GlobalOnly,
        };
        let server = McpServer {
            id: "s1".into(), name: "Remote".into(), description: String::new(),
            transport: McpTransport::Streamable { url: "https://example.com/mcp".into() },
            source: McpSource::Registry,
            api_spec: None,
        };
        // Gemini convention
        let gemini_entry = build_kronn_managed_json_entry(&config, &server, "s", true).unwrap().unwrap();
        assert_eq!(gemini_entry.get("httpUrl").unwrap().as_str(), Some("https://example.com/mcp"));
        assert!(gemini_entry.get("type").is_none());

        // Claude convention (type:"http" + url)
        let claude_entry = build_kronn_managed_json_entry(&config, &server, "s", false).unwrap().unwrap();
        assert_eq!(claude_entry.get("type").unwrap().as_str(), Some("http"));
        assert_eq!(claude_entry.get("url").unwrap().as_str(), Some("https://example.com/mcp"));
    }

    #[test]
    fn slugify_label_simple() {
        assert_eq!(slugify_label("MyServer"), "myserver");
        assert_eq!(slugify_label("Hello World"), "hello-world");
    }

    #[test]
    fn slugify_label_collapses_adjacent_separators() {
        // Multiple non-alnum chars in a row collapse into a single dash.
        assert_eq!(slugify_label("Foo   Bar"), "foo-bar");
        assert_eq!(slugify_label("Foo!!Bar"), "foo-bar");
        assert_eq!(slugify_label("Foo / Bar / Baz"), "foo-bar-baz");
    }

    #[test]
    fn slugify_label_strips_leading_and_trailing_separators() {
        // Empty segments before/after the meaningful content are filtered out.
        assert_eq!(slugify_label("   trim me   "), "trim-me");
        assert_eq!(slugify_label("--foo--"), "foo");
        assert_eq!(slugify_label("!@#bar!@#"), "bar");
    }

    #[test]
    fn slugify_label_keeps_alnum_only() {
        // Punctuation, accents (non-ASCII alnum), digits all handled.
        assert_eq!(slugify_label("Bug-Report-2025"), "bug-report-2025");
        assert_eq!(slugify_label("API_v3.5"), "api-v3-5");
    }

    #[test]
    fn slugify_label_empty_input_yields_empty() {
        assert_eq!(slugify_label(""), "");
        assert_eq!(slugify_label("   "), "");
        assert_eq!(slugify_label("!!!"), "");
    }

    #[test]
    fn is_default_mcp_context_recognises_unedited_template() {
        // The default template contains "<!-- Examples:" marker + only
        // comment/heading lines outside it.
        let template = r#"# foo — Usage Context

> Instructions for AI agents using **foo** in this project.

**Server:** test

## Rules

<!-- Examples:
- Always use sender address: contact@example.com
-->
"#;
        assert!(is_default_mcp_context(template));
    }

    #[test]
    fn is_default_mcp_context_detects_user_edit() {
        // A real rule outside the <!-- Examples: block flips the result.
        let edited = r#"# foo — Usage Context

> Instructions for AI agents using **foo** in this project.

**Server:** test

## Rules

Always send emails from contact@example.com

<!-- Examples:
- Always use sender address: contact@example.com
-->
"#;
        assert!(!is_default_mcp_context(edited),
            "rule line outside the examples block must flip result");
    }

    #[test]
    fn is_default_mcp_context_no_marker_means_custom() {
        // No "<!-- Examples:" marker at all → user wrote from scratch → customized.
        assert!(!is_default_mcp_context("# entirely custom\nDo X always"));
        assert!(!is_default_mcp_context(""));
    }

    #[test]
    fn is_default_mcp_context_tolerates_bullet_only_lines() {
        // Bullet lines (starting with '-') are considered structure, not custom rules.
        let bullet_only = r#"# Title

<!-- Examples:
- bullet one
- bullet two
-->
"#;
        assert!(is_default_mcp_context(bullet_only),
            "bare bullets inside Examples block are template structure");
    }
}
