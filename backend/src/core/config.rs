use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::path::PathBuf;
use tokio::fs;

use crate::models::{
    AgentConfig, AgentsConfig, ApiKey, AppConfig, ModelTier, ScanConfig, ServerConfig, TokensConfig,
};

const CONFIG_FILE: &str = "config.toml";
const DEFAULT_PORT: u16 = 3140;

/// Resolve the config directory: ~/.config/kronn/
pub fn config_dir() -> Result<PathBuf> {
    // Check env override first (Docker)
    if let Ok(dir) = crate::core::child_env::var("KRONN_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }

    ProjectDirs::from("com", "kronn", "kronn")
        .map(|d| d.config_dir().to_path_buf())
        .context("Cannot determine config directory")
}

/// Full path to config.toml
pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join(CONFIG_FILE))
}

/// Per data directory, the key `config.toml` must still carry. `encryption_secret`
/// is never serialized from the struct; the reconciler decides here whether the
/// file keeps a copy (no vault holds the key yet) or drops it.
static DISK_KEYS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, String>>,
> = std::sync::LazyLock::new(Default::default);

fn disk_keys() -> std::sync::MutexGuard<'static, std::collections::HashMap<PathBuf, String>> {
    DISK_KEYS.lock().unwrap_or_else(|p| p.into_inner())
}

/// Keep `key` in `dir`'s config.toml on every save.
pub fn retain_disk_key(dir: &std::path::Path, key: &str) {
    disk_keys().insert(dir.to_path_buf(), key.to_string());
}

/// Stop writing the key to `dir`'s config.toml. Returns whether one was kept.
pub fn release_disk_key(dir: &std::path::Path) -> bool {
    disk_keys().remove(dir).is_some()
}

pub fn retained_disk_key(dir: &std::path::Path) -> Option<String> {
    disk_keys().get(dir).cloned()
}

/// The TOML written for `config` in `dir`. Credentials are left out once the
/// encrypted store holds them (`strip_credentials`).
pub(crate) fn disk_toml(
    config: &AppConfig,
    dir: &std::path::Path,
    strip_credentials: bool,
) -> Result<String> {
    let body = if strip_credentials || config.server.auth_token_session_only {
        let mut disk = config.clone();
        disk.server.auth_token = None;
        if strip_credentials {
            disk.tokens.keys.clear();
        }
        toml::to_string_pretty(&disk)
    } else {
        toml::to_string_pretty(config)
    }
    .context("Failed to serialize config")?;
    Ok(match retained_disk_key(dir) {
        // A top-level key before any table header is valid TOML.
        Some(key) => format!("encryption_secret = {}\n{body}", toml::Value::String(key)),
        None => body,
    })
}

/// Load config from disk, or return None if first run
pub async fn load() -> Result<Option<AppConfig>> {
    let path = config_path()?;

    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)
        .await
        .context("Failed to read config file")?;
    let dir = config_dir()?;
    if let Err(schema_error) = toml::from_str::<AppConfig>(&content) {
        if key_only_file(&content).ok().flatten().is_none() {
            // Not a config Kronn can read (manual edit, foreign tool, a newer
            // Kronn's values): keep it aside untouched, salvage its key, start
            // as a first run, and tell the user (wizard / Settings).
            let cause = if content.parse::<toml::Table>().is_err() {
                "it is not valid TOML"
            } else {
                "it is valid TOML but not a configuration this Kronn understands"
            };
            let kept = set_aside_unreadable_config(&dir, &path)?;
            let key = salvage_key_line(&content);
            tracing::error!(
                "config.toml was set aside as {kept}: {cause} ({schema_error}). Kronn starts as \
                 a first run{}",
                if key.is_some() {
                    " with the key it held"
                } else {
                    ""
                }
            );
            record_set_aside(
                &dir,
                if key.is_some() {
                    format!(
                        "config.toml could not be read ({cause}) and was kept as {kept}. The \
                         encryption key it held was recovered; the settings start over"
                    )
                } else {
                    format!(
                        "config.toml could not be read ({cause}) and was kept as {kept}. No \
                         encryption key could be read from it; the settings start over, and the \
                         key is looked for in the other key stores and backups"
                    )
                },
            );
            if let Some(key) = key.as_deref() {
                retain_disk_key(&dir, key);
            }
            let mut config = default_config_without_key();
            config.encryption_secret = key;
            return Ok(Some(config));
        }
    }
    if let Some(key) = key_only_file(&content)? {
        // Left by a reset that had to keep the key: first run, with that key.
        if let Some(key) = key.as_deref() {
            retain_disk_key(&dir, key);
        }
        let mut config = default_config_without_key();
        config.encryption_secret = key;
        return Ok(Some(config));
    }

    let mut config: AppConfig = toml::from_str(&content).context("Failed to parse config file")?;
    if let Some(key) = config
        .encryption_secret
        .as_deref()
        .filter(|k| !k.is_empty())
    {
        // Kept verbatim until the reconciler proves a vault holds it.
        retain_disk_key(&dir, key);
    }

    let mut needs_save = false;

    // The encryption key is intentionally NOT (re)generated here. `load()` runs
    // BEFORE the DB is open, so it cannot tell whether encrypted rows already
    // exist — minting a key at this point is exactly what orphaned every secret
    // in the 2026-06-30 incident. The key is resolved AFTER DB open by the
    // DB-aware reconciler (env → keychain → sidecar → this legacy field), which
    // only mints on a genuinely empty install. `encryption_secret` is left as
    // read from disk (possibly None) and preserved verbatim on any re-save.
    //
    // A missing auth token is not generated here either: since KT-1007 it
    // lives in the encrypted credential store, which `credential_store::boot`
    // reads after the DB opens and only then mints a token if none exists.

    // Migrate legacy single-key fields to multi-key system
    if config.tokens.keys.is_empty() {
        let legacy_keys: Vec<(&str, &Option<String>)> = vec![
            ("anthropic", &config.tokens.anthropic),
            ("openai", &config.tokens.openai),
            ("google", &config.tokens.google),
        ];
        for (provider, key_opt) in legacy_keys {
            if let Some(ref val) = key_opt {
                config.tokens.keys.push(ApiKey {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: "Personal API Key".into(),
                    provider: provider.into(),
                    value: val.clone(),
                    active: true,
                });
            }
        }
        if !config.tokens.keys.is_empty() {
            // Clear legacy fields (skip_serializing prevents them from being written)
            config.tokens.anthropic = None;
            config.tokens.openai = None;
            config.tokens.google = None;
            needs_save = true;
            tracing::info!(
                "Migrated {} legacy API key(s) to multi-key format",
                config.tokens.keys.len()
            );
        }
    }

    if needs_save {
        // Before the boot migration: the credential store is not armed yet, so
        // the file keeps whatever credentials it had.
        let updated = disk_toml(&config, &dir, false)?;
        // Same atomic temp+fsync+rename path as save() — a plain fs::write
        // here could truncate-then-fail and lose the encryption_secret
        // (2026-06-30 incident class).
        persist_atomic(dir, path, updated).await?;
    }

    Ok(Some(config))
}

/// Save config to disk.
///
/// On Unix the config directory and `config.toml` file are tightened to
/// `0700`/`0600` so other users on the host cannot read the auth token,
/// encryption secret, or stored API keys. On Windows we rely on the standard
/// per-user `%APPDATA%` ACLs (no chmod equivalent — Windows ACLs already
/// restrict the user profile dir to its owner).
pub async fn save(config: &AppConfig) -> Result<()> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).await?;
    restrict_permissions(&dir, true).await;

    // Credentials go to the encrypted store first; a failure there leaves the
    // previous config.toml untouched.
    let credentials_stored = super::credential_store::sync_for_save(&dir, config).await?;
    let content = disk_toml(config, &dir, credentials_stored)?;
    let path = config_path()?;

    persist_atomic(dir, path.clone(), content).await?;

    // KT-373 — republish what the running process reads directly rather than
    // through the config on every use. Doing it here instead of at each of the
    // half-dozen save sites means a new one cannot forget: persisting a
    // threshold and applying it are the same action.
    crate::core::worktree::set_disk_thresholds(
        config.server.disk_warning_gib,
        config.server.disk_critical_gib,
    );

    tracing::info!("Config saved to {}", path.display());
    Ok(())
}

/// Atomic write on a blocking thread: fully-written temp (0600) → fsync →
/// rename over the target (atomic on one filesystem) → fsync the dir. A crash
/// or a concurrent reader never sees a half-written config, so the key/token
/// fields can't be lost to a torn write. Replaces the old `fs::write`, which
/// could truncate-then-fail and leave a corrupt config. Shared by `save()`
/// and the migration re-save in `load()`.
async fn persist_atomic(dir: PathBuf, path: PathBuf, content: String) -> Result<()> {
    // Write chokepoint guard: a test writing config without KRONN_DATA_DIR
    // clobbers the developer's REAL config.toml (a full `cargo test` wiped
    // pseudo/avatar/model-tiers on the host, 2026-07-13). Two layers because
    // integration binaries compile this lib WITHOUT cfg(test): the runtime
    // check keys on the executable living in `target/*/deps/` — true for
    // every cargo test/bench binary, never for `cargo run` or installed
    // binaries. Reads stay free; only the destructive act is fenced.
    #[cfg(test)]
    if crate::core::child_env::var("KRONN_DATA_DIR").is_err() {
        panic!("test attempted to WRITE the real config.toml — set KRONN_DATA_DIR (tempdir) in this test");
    }
    #[cfg(not(test))]
    if crate::core::child_env::var("KRONN_DATA_DIR").is_err()
        && std::env::current_exe()
            .map(|p| p.components().any(|c| c.as_os_str() == "deps"))
            .unwrap_or(false)
    {
        anyhow::bail!(
            "test binary attempted to WRITE the real config.toml — call isolate_config_dir() \
             (set KRONN_DATA_DIR to a tempdir) in this integration test"
        );
    }
    tokio::task::spawn_blocking(move || write_config_atomic(&dir, &path, content.as_bytes()))
        .await
        .context("config write task panicked")?
        .context("atomic config write failed")?;
    Ok(())
}

/// Sequence counter so concurrent `save()` calls in one process use distinct
/// temp filenames (a shared temp name would let one writer's rename yank the
/// other's temp out from under it).
static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Atomically persist `content` to `path` inside the already-existing `dir`:
/// write a sibling temp file, tighten it to `0600`, fsync it, then rename over
/// the target and fsync the directory so the rename itself is durable. On any
/// failure the pre-existing `path` is left untouched and the temp is removed.
fn write_config_atomic(
    dir: &std::path::Path,
    path: &std::path::Path,
    content: &[u8],
) -> std::io::Result<()> {
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(
        ".{}.{}.{}.tmp",
        CONFIG_FILE,
        std::process::id(),
        seq
    ));
    // Owner-only from creation, never through a planted symlink, and removed
    // when the write or fsync fails.
    let _ = dir;
    super::keyvault::write_private_atomic(&tmp, path, content)
}

/// Acquire an exclusive advisory lock on the data dir so exactly ONE backend
/// runs against a given `config_dir()`. Prevents two instances (a stale
/// process, or P2P peers sharing a synced dir) from racing on config.toml / the
/// key / the DB. Hold the returned handle for the process lifetime; dropping it
/// releases the lock.
pub fn acquire_data_dir_lock() -> Result<std::fs::File> {
    let dir = config_dir()?;
    acquire_lock_in(&dir)
}

pub(crate) fn acquire_lock_in(dir: &std::path::Path) -> Result<std::fs::File> {
    #[cfg(not(windows))]
    use fs2::FileExt;
    #[cfg(windows)]
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::create_dir_all(dir)?;
    let lock_path = dir.join(".kronn.lock");
    let mut options = std::fs::OpenOptions::new();
    options
        .create(true)
        // Pure lock file — its (empty) content is irrelevant, only the flock
        // matters. Explicit no-truncate keeps clippy's suspicious-open-options
        // happy without implying we ever write to it.
        .truncate(false)
        .write(true);
    #[cfg(windows)]
    options.share_mode(0);
    let f = options
        .open(&lock_path)
        .with_context(|| format!("open data-dir lock {}", lock_path.display()))?;
    #[cfg(not(windows))]
    f.try_lock_exclusive().map_err(|e| {
        anyhow::anyhow!(
            "another Kronn instance is already running against this data directory \
             ({}). Only one backend may use it at a time.\n\
             \u{2192} Stop the other one first:\n\
             \u{2022}  Docker:  kronn stop\n\
             \u{2022}  native:  ./kronn stop\n\
             then start Kronn again. (lock: {e})",
            dir.display()
        )
    })?;
    Ok(f)
}

/// Duplicate the backend lock for a child which may outlive this process.
///
/// A spawned Git process can survive an ungraceful backend exit.  Keeping this
/// descriptor open across `exec` means the replacement backend cannot acquire
/// the data-directory lock and therefore cannot reap that Git operation's
/// commit lease while Git (or one of its hooks) is still alive.
#[cfg(unix)]
pub(crate) fn inherit_data_dir_lock_for_child(lock: &std::fs::File) -> Result<std::fs::File> {
    lock.try_clone()
        .context("duplicate data-directory lock for Git child")
}

/// Make only this command inherit `lock`.
///
/// The duplicate remains `FD_CLOEXEC` in the parent. `pre_exec` runs after
/// `fork`, so clearing the flag there cannot leak the lock into an unrelated
/// command spawned concurrently by another backend thread.
#[cfg(unix)]
pub(crate) fn inherit_data_dir_lock_on_command(
    command: &mut std::process::Command,
    lock: &std::fs::File,
) {
    use std::os::{fd::AsRawFd, unix::process::CommandExt};

    let fd = lock.as_raw_fd();
    unsafe {
        command.pre_exec(move || {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
pub(crate) fn inherit_data_dir_lock_for_child(lock: &std::fs::File) -> Result<std::fs::File> {
    lock.try_clone()
        .context("duplicate exclusive data-directory handle for Git child")
}

/// Pass the lock to exactly this Windows child through its standard-handle
/// inheritance list. `std::process` prepares an invocation-local inheritable
/// duplicate for `Stdio`; the parent lock handle itself never becomes globally
/// inheritable, so concurrent children cannot retain it.
#[cfg(windows)]
pub(crate) fn inherit_data_dir_lock_on_command(
    command: &mut std::process::Command,
    lock: &std::fs::File,
) -> Result<()> {
    let child_standard_handle = lock
        .try_clone()
        .context("duplicate data-directory lock as Git child standard handle")?;
    command.stdin(std::process::Stdio::from(child_standard_handle));
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn inherit_data_dir_lock_for_child(_: &std::fs::File) -> Result<std::fs::File> {
    anyhow::bail!("this platform cannot safely inherit the data-directory lock into Git")
}

/// Restrict a path to owner-only access on Unix; no-op on Windows.
async fn restrict_permissions(path: &std::path::Path, is_dir: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if is_dir { 0o700 } else { 0o600 };
        if let Err(e) = fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await {
            tracing::warn!("Failed to chmod {} to {:o}: {}", path.display(), mode, e);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, is_dir); // suppress unused warning
    }
}

/// Create default config (used during setup wizard)
/// Mirror an operator-set `KRONN_AUTH_TOKEN` into the config the auth
/// middleware reads, and enable auth: setting it is asking for auth. The
/// caller removes the variable from the process environment, so no child
/// process inherits the admin token (KT-1006).
/// Read an operator-set `KRONN_AUTH_TOKEN` and remove it from this process's
/// environment, so no child can inherit the admin token (KT-1006).
pub fn take_env_auth_token() -> Option<String> {
    let token = crate::core::child_env::var("KRONN_AUTH_TOKEN").ok();
    std::env::remove_var("KRONN_AUTH_TOKEN");
    token
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

pub fn adopt_env_auth_token(server: &mut ServerConfig, env_token: Option<String>) {
    let Some(token) = env_token.filter(|token| !token.is_empty()) else {
        return;
    };
    match &server.auth_token {
        None => server.auth_token = Some(token),
        Some(configured) if *configured != token => tracing::warn!(
            "KRONN_AUTH_TOKEN env differs from the token in config.toml; the API \
             validates the CONFIG token. Align them (unset the env var, or clear \
             server.auth_token in config.toml)."
        ),
        Some(_) => {}
    }
    if !server.auth_enabled {
        tracing::info!("KRONN_AUTH_TOKEN set: enabling API authentication (was disabled)");
        server.auth_enabled = true;
    }
}

pub fn default_config() -> AppConfig {
    AppConfig {
        server: ServerConfig {
            host: "127.0.0.1".into(),
            port: DEFAULT_PORT,
            runtime_port: None,
            domain: None,
            auth_token: None,
            auth_enabled: false,
            auth_locked: false,
            auth_token_session_only: false,
            auth_strict_localhost: false,
            failure_notify_url: None,
            run_retention_days: 0,
            run_payload_retention_days: 0,
            execution_variable_retention_days: 30,
            interrupted_worktree_ttl_days:
                crate::models::setup::DEFAULT_INTERRUPTED_WORKTREE_TTL_DAYS,
            disk_critical_gib: crate::models::setup::DEFAULT_DISK_CRITICAL_GIB,
            disk_warning_gib: crate::models::setup::DEFAULT_DISK_WARNING_GIB,
            max_concurrent_agents: 5,
            agent_stall_timeout_min: 5,
            agent_global_timeout_min: crate::models::DEFAULT_AGENT_GLOBAL_TIMEOUT_MIN,
            local_agent_global_timeout_min: crate::models::DEFAULT_LOCAL_AGENT_GLOBAL_TIMEOUT_MIN,
            ollama_context_overrides: std::collections::HashMap::new(),
            pseudo: None,
            avatar_email: None,
            bio: None,
            global_context: None,
            global_context_mode: "always".into(),
            anti_hallucination_mode: crate::core::anti_halluc::DEFAULT_MODE_STR.into(),
            continual_learning_enabled: false, // 0.10.0 — opt-in (beta), see ServerConfig doc
            discussion_notes_enabled: true,
            debug_mode: false,
            default_model_tier: ModelTier::Default,
            // 0.8.6 phase 4 — auto-summary off out of the box. See
            // ServerConfig field docs for rationale (modern agents
            // have large context + MCP access, no auto-summary needed).
            default_summary_strategy: crate::models::SummaryStrategy::Off,
            agent_handoffs_enabled: false,
            agent_handoff_paid_limit: 1,
            agent_handoff_paid_unlimited: false,
            agent_handoff_blocked_agents: vec![],
            discussion_weight: crate::models::DiscussionWeightConfig::default(),
        },
        tokens: TokensConfig {
            anthropic: None,
            openai: None,
            google: None,
            keys: vec![],
            disabled_overrides: vec![],
        },
        scan: ScanConfig {
            paths: vec![],
            ignore: vec![
                "node_modules".into(),
                ".git".into(),
                "target".into(),
                "dist".into(),
                "__pycache__".into(),
                ".venv".into(),
                // macOS system directories (protected, cause permission errors in Docker)
                "Library".into(),
                "Movies".into(),
                "Music".into(),
                "Pictures".into(),
                ".Trash".into(),
                "Applications".into(),
                // Common non-project directories
                ".cache".into(),
                ".local".into(),
                ".npm".into(),
                ".cargo".into(),
                ".rustup".into(),
            ],
            scan_depth: 4,
        },
        agents: AgentsConfig {
            claude_code: AgentConfig {
                concurrency: None,
                path: None,
                installed: false,
                version: None,
                full_access: false,
                mention_color: None,
                base_url: None,
            },
            codex: AgentConfig {
                concurrency: None,
                path: None,
                installed: false,
                version: None,
                full_access: false,
                mention_color: None,
                base_url: None,
            },
            open_code: AgentConfig::default(),
            gemini_cli: AgentConfig::default(),
            kiro: AgentConfig::default(),
            vibe: AgentConfig::default(),
            copilot_cli: AgentConfig::default(),
            ollama: AgentConfig::default(),
            lite_llm: AgentConfig::default(),
            nvidia: AgentConfig::default(),
            model_tiers: Default::default(),
        },
        language: "fr".into(),
        ui_language: "fr".into(),
        stt_model: None,
        tts_voices: std::collections::HashMap::new(),
        disabled_agents: vec![],
        encryption_secret: Some(super::crypto::generate_secret()),
        secret_themes: std::collections::HashMap::new(),
        unlocked_profiles: Vec::new(),
        disabled_auto_skills: Vec::new(),
    }
}

/// Per data directory, the notice about a config.toml kept aside at start.
static SET_ASIDE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, String>>,
> = std::sync::LazyLock::new(Default::default);

fn record_set_aside(dir: &std::path::Path, notice: String) {
    SET_ASIDE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(dir.to_path_buf(), notice);
}

/// The notice about a config.toml set aside at this start, if any.
pub fn set_aside_notice(dir: &std::path::Path) -> Option<String> {
    SET_ASIDE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(dir)
        .cloned()
}

/// Move an unreadable config.toml to `config.toml.corrupt.<ts>` (owner-only).
fn set_aside_unreadable_config(dir: &std::path::Path, path: &std::path::Path) -> Result<String> {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.6fZ");
    let name = format!("{CONFIG_FILE}.corrupt.{stamp}");
    std::fs::rename(path, dir.join(&name)).context("move the unreadable config.toml aside")?;
    super::keyvault::sync_dir(dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir.join(&name), std::fs::Permissions::from_mode(0o600));
    }
    Ok(name)
}

/// A top-level `encryption_secret = "..."` line that parses on its own.
pub(crate) fn salvage_key_line(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let table: toml::Table = line.trim().parse().ok()?;
        table
            .get("encryption_secret")
            .and_then(|v| v.as_str())
            .filter(|k| !k.is_empty())
            .map(str::to_string)
    })
}

/// `default_config()` without a key: a missing config.toml must not offer a
/// random key to the reconciler, which could then keep it for good.
pub fn default_config_without_key() -> AppConfig {
    let mut config = default_config();
    config.encryption_secret = None;
    config
}

/// `Some(key)` when `content` holds no settings, only (optionally) the key:
/// the file a reset writes when config.toml carries a needed copy of the key.
fn key_only_file(content: &str) -> Result<Option<Option<String>>> {
    let table: toml::Table = content.parse().context("Failed to parse config file")?;
    // Only an empty file or one holding just the key: anything else (a
    // `[tokens]` table, say) is a real config whose content must not be dropped.
    if !table.keys().all(|k| k == "encryption_secret") {
        return Ok(None);
    }
    Ok(Some(
        table
            .get("encryption_secret")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    ))
}

/// Write `dir`'s config.toml now so it carries the retained key: the settings
/// in memory when the file is a real config, the key alone otherwise (a first
/// run stays a first run). Used when no key store could take a new key.
pub fn write_key_copy_now(dir: &std::path::Path, config: &AppConfig) -> Result<()> {
    let path = dir.join(CONFIG_FILE);
    let real_config = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| key_only_file(&text).ok())
        .is_some_and(|key_only| key_only.is_none());
    let content = if real_config {
        disk_toml(config, dir, false)?
    } else {
        match retained_disk_key(dir) {
            Some(key) => format!("encryption_secret = {}\n", toml::Value::String(key)),
            None => return Ok(()),
        }
    };
    std::fs::create_dir_all(dir)?;
    write_config_atomic(dir, &path, content.as_bytes()).context("write config.toml")?;
    Ok(())
}

/// The `encryption_secret` config.toml in `dir` holds, if any.
pub fn read_disk_key(dir: &std::path::Path) -> Result<Option<String>> {
    let text = std::fs::read_to_string(dir.join(CONFIG_FILE)).context("read config.toml")?;
    let table: toml::Table = text.parse().context("parse config.toml")?;
    Ok(table
        .get("encryption_secret")
        .and_then(|v| v.as_str())
        .map(str::to_string))
}

/// Replace config.toml with a file holding only the retained key (reset).
/// Without a retained key, the file is removed as before.
pub async fn reset_to_key_only() -> Result<()> {
    let dir = config_dir()?;
    let path = config_path()?;
    match retained_disk_key(&dir) {
        Some(key) => {
            let content = format!("encryption_secret = {}\n", toml::Value::String(key));
            persist_atomic(dir, path, content).await
        }
        None => match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        },
    }
}

/// Check if this is the first run (no config, or one holding only the key)
pub async fn is_first_run() -> Result<bool> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(true);
    }
    let content = fs::read_to_string(&path).await?;
    Ok(key_only_file(&content).ok().flatten().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Tests that mutate `KRONN_DATA_DIR` (a process-wide env var) must
    /// run serialized — parallel execution would have them stomp each
    /// other's paths and see the wrong file on read-back. Uses
    /// `tokio::sync::Mutex` rather than `std::sync::Mutex` because the
    /// guard is held across `.await` points (save + load), which
    /// clippy's `await_holding_lock` lint rejects with a std mutex.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[test]
    fn run_payload_retention_is_off_on_every_install_until_chosen() {
        let fresh = default_config();
        assert_eq!(fresh.server.run_payload_retention_days, 0);

        // A config.toml written before the field existed has no such line.
        let toml = toml::to_string_pretty(&fresh).unwrap();
        let legacy: String = toml
            .lines()
            .filter(|l| !l.starts_with("run_payload_retention_days"))
            .collect::<Vec<_>>()
            .join("\n");
        let existing: crate::models::AppConfig = toml::from_str(&legacy).unwrap();
        assert_eq!(existing.server.run_payload_retention_days, 0);

        let chosen = toml.replace(
            "run_payload_retention_days = 0",
            "run_payload_retention_days = 30",
        );
        let kept: crate::models::AppConfig = toml::from_str(&chosen).unwrap();
        assert_eq!(kept.server.run_payload_retention_days, 30);
    }

    #[test]
    fn default_config_is_valid() {
        let cfg = default_config();
        assert!(!cfg.server.host.is_empty(), "host must be non-empty");
        assert!(cfg.server.port > 0, "port must be > 0");
        assert!(
            cfg.encryption_secret.is_some(),
            "encryption_secret must be set"
        );
        assert!(
            !cfg.encryption_secret.as_ref().unwrap().is_empty(),
            "encryption_secret must be non-empty"
        );
    }

    #[test]
    fn config_dir_returns_path() {
        // May fail in exotic CI environments without HOME, but works in normal setups
        let dir = config_dir();
        assert!(
            dir.is_ok(),
            "config_dir() should return Ok: {:?}",
            dir.err()
        );
        let path = dir.unwrap();
        assert!(
            !path.as_os_str().is_empty(),
            "config dir path must be non-empty"
        );
    }

    #[test]
    fn config_path_ends_in_config_toml() {
        let path = config_path();
        assert!(
            path.is_ok(),
            "config_path() should return Ok: {:?}",
            path.err()
        );
        let p = path.unwrap();
        assert!(
            p.to_string_lossy().ends_with("config.toml"),
            "config path should end in config.toml, got: {}",
            p.display()
        );
    }

    /// A unique scratch dir per test (no `KRONN_DATA_DIR` needed — these test the
    /// filesystem helpers directly).
    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "kronn-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// C2-17 — the temp file is created owner-only and never follows a
    /// symlink planted at its name.
    #[cfg(unix)]
    #[test]
    fn write_config_atomic_never_follows_a_planted_temp_symlink() {
        let dir = scratch_dir("atomic-symlink");
        let victim = dir.join("victim");
        std::fs::write(&victim, "untouched").unwrap();
        let seq = TMP_SEQ.load(std::sync::atomic::Ordering::Relaxed);
        for next in seq..seq + 64 {
            let name = format!(".{}.{}.{}.tmp", CONFIG_FILE, std::process::id(), next);
            let _ = std::os::unix::fs::symlink(&victim, dir.join(name));
        }
        write_config_atomic(&dir, &dir.join(CONFIG_FILE), b"secret = 1\n").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
        assert_eq!(
            std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap(),
            "secret = 1\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// C3-09 — an unparseable config.toml is kept aside, its key salvaged, and
    /// the start continues as a first run.
    #[tokio::test]
    #[serial]
    async fn an_unparseable_config_is_kept_aside_and_its_key_salvaged() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = scratch_dir("corrupt");
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());
        let key = crate::core::crypto::generate_secret();
        std::fs::write(
            tmp.join(CONFIG_FILE),
            format!("encryption_secret = \"{key}\"\nx = [broken\n"),
        )
        .unwrap();
        let loaded = load()
            .await
            .unwrap()
            .expect("first run with the salvaged key");
        assert_eq!(loaded.encryption_secret.as_deref(), Some(key.as_str()));
        assert_eq!(retained_disk_key(&tmp), Some(key.clone()));
        assert!(!tmp.join(CONFIG_FILE).exists());
        let kept: Vec<_> = std::fs::read_dir(&tmp)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("config.toml.corrupt.")
            })
            .collect();
        assert_eq!(kept.len(), 1);
        assert!(std::fs::read_to_string(kept[0].path())
            .unwrap()
            .contains("x = [broken"));
        assert!(is_first_run().await.unwrap());
        release_disk_key(&tmp);
        std::env::remove_var("KRONN_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// C4-09 — valid TOML that is not a Kronn config: set aside, and the
    /// notice for the wizard / Settings is recorded with its cause.
    #[tokio::test]
    #[serial]
    async fn a_schema_invalid_config_is_set_aside_with_a_notice() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = scratch_dir("schema");
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());
        std::fs::write(tmp.join(CONFIG_FILE), "[server]\nport = \"not a number\"\n").unwrap();
        let loaded = load().await.unwrap().expect("first run");
        assert!(loaded.encryption_secret.is_none());
        let notice = set_aside_notice(&tmp).expect("recorded");
        assert!(
            notice.contains("valid TOML but not a configuration"),
            "{notice}"
        );
        assert!(notice.contains("config.toml.corrupt."), "{notice}");
        // C5-02: no key could be salvaged, so the notice never claims one is intact.
        assert!(
            !notice.contains("intact") && notice.contains("No encryption key"),
            "{notice}"
        );
        std::env::remove_var("KRONN_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// C2-31 — only an empty or key-only file counts as "key only".
    #[test]
    fn only_a_key_only_file_is_read_as_key_only() {
        assert_eq!(key_only_file("").unwrap(), Some(None));
        assert_eq!(
            key_only_file("encryption_secret = \"ab\"\n").unwrap(),
            Some(Some("ab".into()))
        );
        assert_eq!(
            key_only_file("[tokens]\nanthropic = \"sk\"\n").unwrap(),
            None
        );
        assert_eq!(key_only_file("language = \"fr\"\n").unwrap(), None);
    }

    #[test]
    fn write_config_atomic_writes_and_leaves_no_temp() {
        let dir = scratch_dir("atomic");
        let path = dir.join(CONFIG_FILE);
        write_config_atomic(&dir, &path, b"port = 3140\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "port = 3140\n");
        let has_tmp = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains(".tmp"));
        assert!(!has_tmp, "atomic write must leave no .tmp behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn write_config_atomic_sets_file_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch_dir("atomic0600");
        let path = dir.join(CONFIG_FILE);
        write_config_atomic(&dir, &path, b"x = 1").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config file must be 0600");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_config_atomic_errors_when_dir_missing() {
        // Covers the File::create error path (root-independent).
        let dir = scratch_dir("atomicmissing");
        let path = dir.join(CONFIG_FILE);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(
            write_config_atomic(&dir, &path, b"x = 1\n").is_err(),
            "writing into a missing dir must error, not panic"
        );
    }

    #[test]
    fn write_config_atomic_leaves_original_when_rename_fails() {
        // Make `path` a NON-EMPTY dir so rename(temp, path) fails — proves the
        // pre-existing target is untouched on failure and the temp is cleaned up.
        let dir = scratch_dir("atomicrename");
        let path = dir.join(CONFIG_FILE);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("marker"), b"keep").unwrap();
        assert!(
            write_config_atomic(&dir, &path, b"new = 1\n").is_err(),
            "rename over a non-empty dir must fail"
        );
        assert_eq!(
            std::fs::read_to_string(path.join("marker")).unwrap(),
            "keep",
            "the pre-existing target must be untouched on failure"
        );
        let has_tmp = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains(".tmp"));
        assert!(!has_tmp, "temp must be removed when the rename fails");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial] // lock path derives from KRONN_DATA_DIR — races the env-mutating tests
    fn data_dir_lock_is_exclusive() {
        let dir = scratch_dir("lock");
        let g1 = acquire_lock_in(&dir).expect("first exclusive lock must succeed");
        assert!(
            acquire_lock_in(&dir).is_err(),
            "a second exclusive lock on the same data dir must be refused"
        );
        drop(g1); // releasing lets a later acquire succeed
                  // Retry briefly: under full-suite load the re-open can hit transient
                  // resource errors (EMFILE from parallel git/sqlite fds). A genuinely
                  // stuck lock still fails after the window — with the REAL error shown.
        let mut last_err = None;
        for _ in 0..20 {
            match acquire_lock_in(&dir) {
                Ok(_) => {
                    last_err = None;
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
        assert!(
            last_err.is_none(),
            "lock must be re-acquirable after release: {last_err:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn data_dir_lock_resolves_the_configured_data_directory() {
        let dir = scratch_dir("configured-lock");
        let previous_data_dir = crate::core::child_env::var_os("KRONN_DATA_DIR");
        std::env::set_var("KRONN_DATA_DIR", &dir);

        let first = acquire_data_dir_lock().expect("configured data-dir lock must succeed");
        assert!(
            acquire_data_dir_lock().is_err(),
            "the public lock entry point must protect the configured data directory"
        );
        drop(first);

        match previous_data_dir {
            Some(value) => std::env::set_var("KRONN_DATA_DIR", value),
            None => std::env::remove_var("KRONN_DATA_DIR"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// I1 — `load()` must NEVER mint a key when the field is missing. That silent
    /// regeneration is exactly what orphaned every secret in the 2026-06-30
    /// incident. The DB-aware reconciler owns key resolution now.
    #[tokio::test]
    #[serial]
    async fn load_does_not_regenerate_a_missing_encryption_secret() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = scratch_dir("noregen");
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());

        let mut cfg = default_config();
        cfg.server.auth_token = Some("tok".into()); // avoid the auth-gen re-save path
        cfg.encryption_secret = None; // simulate a config that lost its key
        save(&cfg).await.expect("save must succeed");

        let loaded = load().await.expect("load Ok").expect("Some after save");
        assert!(
            loaded.encryption_secret.is_none(),
            "load() MUST NOT generate a key when the field is missing (I1)"
        );

        std::env::remove_var("KRONN_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[serial]
    async fn load_keeps_an_existing_encryption_secret() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = scratch_dir("keepsecret");
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());

        // A 0.14.2 file carries the key; the struct never writes it.
        let mut cfg = default_config();
        let secret = cfg.encryption_secret.clone().expect("default has a secret");
        cfg.server.auth_token = Some("tok".into());
        let legacy = format!(
            "encryption_secret = \"{secret}\"\n{}",
            toml::to_string_pretty(&cfg).unwrap()
        );
        std::fs::write(tmp.join(CONFIG_FILE), legacy).unwrap();

        let loaded = load().await.expect("load Ok").expect("Some after save");
        assert_eq!(
            loaded.encryption_secret.as_deref(),
            Some(secret.as_str()),
            "an existing secret must be preserved verbatim across load"
        );
        // Kept on re-save until the reconciler releases it...
        save(&loaded).await.unwrap();
        let reread = load().await.unwrap().unwrap();
        assert_eq!(reread.encryption_secret.as_deref(), Some(secret.as_str()));
        // ...then gone from the file, although the live config still holds it.
        assert!(release_disk_key(&tmp));
        assert!(!release_disk_key(&tmp), "already released");
        save(&reread).await.unwrap();
        let text = std::fs::read_to_string(tmp.join(CONFIG_FILE)).unwrap();
        assert!(!text.contains("encryption_secret"), "{text}");
        assert!(!text.contains(&secret));

        std::env::remove_var("KRONN_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// KT-405 — the per-model context override must survive an actual
    /// save/load round trip through the real config file, not just serde's
    /// in-memory (de)serialization: an empty map is the common case and must
    /// not become `None`/error on an older config that never had the key.
    #[tokio::test]
    #[serial]
    async fn ollama_context_overrides_round_trip_through_the_real_config_file() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = scratch_dir("ollama-ctx-overrides");
        let previous_data_dir = crate::core::child_env::var_os("KRONN_DATA_DIR");
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());

        let mut cfg = default_config();
        cfg.server
            .ollama_context_overrides
            .insert("qwen3.8:27b-mlx".to_string(), 100_000);
        cfg.server
            .ollama_context_overrides
            .insert("gemma4:12b-mlx".to_string(), 40_000);
        save(&cfg).await.unwrap();

        let loaded = load().await.unwrap().expect("config was just saved");
        assert_eq!(
            loaded
                .server
                .ollama_context_overrides
                .get("qwen3.8:27b-mlx"),
            Some(&100_000)
        );
        assert_eq!(
            loaded.server.ollama_context_overrides.get("gemma4:12b-mlx"),
            Some(&40_000)
        );

        // An older config.toml with no such key at all — not merely an empty
        // table — must still load: the field is genuinely new.
        let path = tmp.join("config.toml");
        let raw = std::fs::read_to_string(&path).unwrap();
        let stripped: String = raw
            .lines()
            .filter(|line| !line.contains("ollama_context_overrides"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !stripped.contains("ollama_context_overrides"),
            "sanity: the key is really gone from the written file"
        );
        std::fs::write(&path, stripped).unwrap();
        let reloaded = load().await.unwrap().expect("config still loads");
        assert!(reloaded.server.ollama_context_overrides.is_empty());

        match previous_data_dir {
            Some(value) => std::env::set_var("KRONN_DATA_DIR", value),
            None => std::env::remove_var("KRONN_DATA_DIR"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Legacy single-key fields (`tokens.anthropic/openai/google`) must migrate
    /// into the multi-key `keys[]` on load, and the legacy fields get cleared.
    #[tokio::test]
    #[serial]
    async fn load_migrates_legacy_single_keys_to_multikey() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = scratch_dir("legacymig");
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());

        // The legacy fields are `skip_serializing`, so we can't round-trip them
        // through save(); write a raw config.toml with the legacy line injected
        // under [tokens] — exactly the shape an OLD Kronn wrote.
        let mut cfg = default_config();
        cfg.server.auth_token = Some("tok".into());
        let toml_str = toml::to_string_pretty(&cfg).unwrap();
        let injected =
            toml_str.replacen("[tokens]\n", "[tokens]\nanthropic = \"sk-ant-legacy\"\n", 1);
        assert!(
            injected.contains("anthropic = \"sk-ant-legacy\""),
            "injection sanity"
        );
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(config_path().unwrap(), injected).unwrap();

        let loaded = load().await.expect("load Ok").expect("Some after save");
        assert_eq!(loaded.tokens.keys.len(), 1, "one legacy key must migrate");
        assert_eq!(loaded.tokens.keys[0].provider, "anthropic");
        assert_eq!(loaded.tokens.keys[0].value, "sk-ant-legacy");
        assert!(
            loaded.tokens.anthropic.is_none(),
            "legacy field must be cleared"
        );

        std::env::remove_var("KRONN_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[serial]
    async fn desktop_runtime_port_is_not_saved_for_the_next_cli_start() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let previous_dir = crate::core::child_env::var_os("KRONN_DATA_DIR");
        std::env::set_var("KRONN_DATA_DIR", tmp.path());

        for saved_port in [3140, 4242] {
            let mut desktop = default_config();
            desktop.server.port = saved_port;
            desktop.server.runtime_port = Some(53591);
            desktop.server.pseudo = Some("Desktop setting retained".into());
            save(&desktop).await.unwrap();

            let cli = load().await.unwrap().unwrap();
            assert_eq!(cli.server.port, saved_port);
            assert_eq!(cli.server.listening_port(), saved_port);
            assert_eq!(cli.server.runtime_port, None);
            assert_eq!(cli.server.pseudo, desktop.server.pseudo);
            // The in-memory key is never serialized from the struct.
            assert_eq!(cli.encryption_secret, None);
            assert_eq!(desktop.server.listening_port(), 53591);
            let persisted = fs::read_to_string(config_path().unwrap()).await.unwrap();
            assert!(!persisted.contains("runtime_port"));
        }

        match previous_dir {
            Some(value) => std::env::set_var("KRONN_DATA_DIR", value),
            None => std::env::remove_var("KRONN_DATA_DIR"),
        }
    }

    /// Atomic rename guarantees a concurrent flurry of `save()` never yields a
    /// torn, unparseable config — `load()` always parses cleanly.
    #[tokio::test]
    #[serial]
    async fn concurrent_saves_never_produce_a_torn_config() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = scratch_dir("concurrent");
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());

        let base = default_config();
        let mut handles = Vec::new();
        for i in 0..8u16 {
            let mut c = base.clone();
            c.server.port = 3000 + i;
            handles.push(tokio::spawn(async move { save(&c).await }));
        }
        for h in handles {
            h.await.expect("task join").expect("save must succeed");
        }

        // The payoff: the final file is always fully parseable (never torn).
        let loaded = load()
            .await
            .expect("load Ok")
            .expect("Some after concurrent saves");
        assert!(
            (3000..3008).contains(&loaded.server.port),
            "a complete config must be readable after concurrent saves"
        );

        std::env::remove_var("KRONN_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A Batman unlock writes "batman" into config.unlocked_profiles AND
    /// persists — a backend restart must NOT reset it. Also checks
    /// secret_themes round-trips (operator-local plaintext overrides).
    #[tokio::test]
    #[serial]
    async fn secret_theme_fields_survive_save_and_reload() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "kronn-secret-fields-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());

        let mut cfg = default_config();
        cfg.unlocked_profiles.push("batman".into());
        cfg.secret_themes
            .insert("matrix".into(), "some-local-code".into());
        save(&cfg).await.expect("save must succeed");

        let reloaded = load().await.expect("load Ok").expect("Some after save");
        assert!(
            reloaded.unlocked_profiles.iter().any(|p| p == "batman"),
            "unlocked_profiles lost across save/load: {:?}",
            reloaded.unlocked_profiles
        );
        assert_eq!(
            reloaded.secret_themes.get("matrix").map(String::as_str),
            Some("some-local-code"),
            "secret_themes.matrix lost across save/load: {:?}",
            reloaded.secret_themes
        );

        std::env::remove_var("KRONN_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `load()` no longer mints an auth token nor resets `auth_enabled`: the
    /// token lives in the encrypted credential store, read after the DB opens
    /// (`credential_store::boot` generates one only when none exists anywhere).
    #[tokio::test]
    #[serial]
    async fn load_does_not_generate_an_auth_token_nor_touch_auth_enabled() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = scratch_dir("noauthgen");
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());

        let mut cfg = default_config();
        cfg.server.auth_token = None;
        cfg.server.auth_enabled = true;
        save(&cfg).await.expect("save must succeed");

        let loaded = load().await.expect("load Ok").expect("Some after save");
        assert!(loaded.server.auth_token.is_none());
        assert!(loaded.server.auth_enabled, "the saved choice is kept");

        std::env::remove_var("KRONN_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn disk_toml_puts_a_retained_key_first_and_strips_credentials_on_request() {
        let dir = scratch_dir("disktoml");
        let mut cfg = default_config();
        cfg.server.auth_token = Some("tok-value".into());
        cfg.tokens.keys.push(ApiKey {
            id: "k".into(),
            name: "n".into(),
            provider: "anthropic".into(),
            value: "sk-value".into(),
            active: true,
        });
        let plain = disk_toml(&cfg, &dir, false).unwrap();
        assert!(plain.contains("tok-value") && plain.contains("sk-value"));
        assert!(!plain.contains("encryption_secret"));

        retain_disk_key(&dir, "abcd");
        let stripped = disk_toml(&cfg, &dir, true).unwrap();
        release_disk_key(&dir);
        assert!(stripped.starts_with("encryption_secret = \"abcd\"\n"));
        assert!(!stripped.contains("tok-value") && !stripped.contains("sk-value"));
        let parsed: AppConfig = toml::from_str(&stripped).unwrap();
        assert_eq!(parsed.encryption_secret.as_deref(), Some("abcd"));
        assert!(parsed.tokens.keys.is_empty());
        assert!(parsed.server.auth_token.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial]
    async fn save_chmods_dir_700_and_file_600_on_unix() {
        let _lock = ENV_LOCK.lock().await;
        // Real save() round-trip via KRONN_DATA_DIR override so we don't
        // touch the user's real ~/.config/kronn during tests.
        use std::os::unix::fs::PermissionsExt;

        let tmp = std::env::temp_dir().join(format!("kronn-config-perms-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());

        let cfg = default_config();
        save(&cfg).await.expect("save must succeed");

        let dir_meta = std::fs::metadata(&tmp).unwrap();
        let dir_mode = dir_meta.permissions().mode() & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "config dir must be 0700 on Unix, got {:o}",
            dir_mode
        );

        let file_meta = std::fs::metadata(tmp.join("config.toml")).unwrap();
        let file_mode = file_meta.permissions().mode() & 0o777;
        assert_eq!(
            file_mode, 0o600,
            "config.toml must be 0600 on Unix (contains auth_token + encryption_secret), got {:o}",
            file_mode
        );

        std::env::remove_var("KRONN_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[serial]
    async fn load_returns_none_when_no_config_file() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "kronn-load-none-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());

        let loaded = load().await.expect("load must succeed even with no file");
        assert!(
            loaded.is_none(),
            "absent config file must return None, got {loaded:?}"
        );

        std::env::remove_var("KRONN_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[serial]
    async fn is_first_run_true_before_any_save() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "kronn-first-run-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());

        let first = is_first_run().await.expect("first_run check");
        assert!(first, "with no config file, is_first_run must be true");

        let cfg = default_config();
        save(&cfg).await.expect("save");

        let still_first = is_first_run().await.expect("first_run after save");
        assert!(!still_first, "after a save, is_first_run must be false");

        std::env::remove_var("KRONN_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[serial]
    async fn save_then_load_preserves_default_scan_ignores() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "kronn-scan-ignore-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());

        let cfg = default_config();
        save(&cfg).await.expect("save");
        let loaded = load().await.expect("load").expect("Some");

        // Default ignore list must roundtrip — these strings are checked
        // against during scans so a serialization drop would silently scan
        // node_modules / .git / target etc.
        for needle in [
            "node_modules",
            ".git",
            "target",
            "dist",
            ".cache",
            ".rustup",
        ] {
            assert!(
                loaded.scan.ignore.iter().any(|s| s == needle),
                "loaded scan.ignore must contain {needle:?} ; got {:?}",
                loaded.scan.ignore
            );
        }
        assert_eq!(loaded.scan.scan_depth, 4, "scan_depth must default to 4");

        std::env::remove_var("KRONN_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[serial]
    async fn save_preserves_anti_hallucination_mode_and_default_tier() {
        // 0.8.7 + 0.8.6 fields that should NEVER be dropped on roundtrip.
        let _lock = ENV_LOCK.lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "kronn-anti-hallu-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());

        let mut cfg = default_config();
        cfg.server.anti_hallucination_mode = "strict".into();
        cfg.server.default_model_tier = ModelTier::Reasoning;
        save(&cfg).await.expect("save");

        let loaded = load().await.expect("load").expect("Some");
        assert_eq!(loaded.server.anti_hallucination_mode, "strict");
        assert!(
            matches!(loaded.server.default_model_tier, ModelTier::Reasoning),
            "default_model_tier dropped on save/load: {:?}",
            loaded.server.default_model_tier
        );

        std::env::remove_var("KRONN_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn an_env_auth_token_enables_auth_through_the_config() {
        let mut server = default_config().server;
        server.auth_token = None;
        server.auth_enabled = false;
        adopt_env_auth_token(&mut server, Some("op-token".into()));
        assert_eq!(server.auth_token.as_deref(), Some("op-token"));
        assert!(server.auth_enabled);

        // The configured token wins over a differing env one.
        adopt_env_auth_token(&mut server, Some("other".into()));
        assert_eq!(server.auth_token.as_deref(), Some("op-token"));

        // Empty or absent: nothing changes.
        let mut untouched = default_config().server;
        untouched.auth_enabled = false;
        let before = untouched.auth_token.clone();
        adopt_env_auth_token(&mut untouched, Some(String::new()));
        adopt_env_auth_token(&mut untouched, None);
        assert_eq!(untouched.auth_token, before);
        assert!(!untouched.auth_enabled);
    }

    /// The backend no longer puts its admin token into its own environment,
    /// where every child process would inherit it (KT-1006).
    #[test]
    fn main_never_exports_the_admin_token() {
        let main = include_str!("../main.rs");
        assert!(!main.contains("set_var(\"KRONN_AUTH_TOKEN\""));
        // `take_env_auth_token` reads and removes it (behaviour tested in
        // credential_store::tests::an_env_auth_token_is_stored_encrypted...).
        assert!(main.contains("take_env_auth_token()"));
    }
}
