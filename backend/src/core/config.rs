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
    if let Ok(dir) = std::env::var("KRONN_DATA_DIR") {
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

/// Load config from disk, or return None if first run
pub async fn load() -> Result<Option<AppConfig>> {
    let path = config_path()?;

    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)
        .await
        .context("Failed to read config file")?;

    let mut config: AppConfig = toml::from_str(&content).context("Failed to parse config file")?;

    let mut needs_save = false;

    // The encryption key is intentionally NOT (re)generated here. `load()` runs
    // BEFORE the DB is open, so it cannot tell whether encrypted rows already
    // exist — minting a key at this point is exactly what orphaned every secret
    // in the 2026-06-30 incident. The key is resolved AFTER DB open by the
    // DB-aware reconciler (env → keychain → sidecar → this legacy field), which
    // only mints on a genuinely empty install. `encryption_secret` is left as
    // read from disk (possibly None) and preserved verbatim on any re-save.

    // Auto-generate auth token on first launch. Auth defaults ON on native
    // (Tauri/CLI), where the localhost bypass keeps it transparent; OFF under
    // Docker, where Docker Desktop NATs published-port traffic to the network
    // gateway so the bypass can't see the real client → auth-on would 401 the
    // user on first launch. The token is still generated (ready for opt-in
    // multi-user); the middleware honours `auth_enabled`. See
    // `core::env::auth_on_by_default`.
    if config.server.auth_token.is_none() {
        config.server.auth_token = Some(uuid::Uuid::new_v4().to_string());
        config.server.auth_enabled = super::env::auth_on_by_default();
        needs_save = true;
        tracing::info!(
            "Generated auth token (auth_enabled={}, docker={})",
            config.server.auth_enabled,
            super::env::is_docker(),
        );
    }

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
        let updated = toml::to_string_pretty(&config).context("Failed to serialize config")?;
        // Same atomic temp+fsync+rename path as save() — a plain fs::write
        // here could truncate-then-fail and lose the encryption_secret
        // (2026-06-30 incident class).
        persist_atomic(config_dir()?, path, updated).await?;
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

    let content = toml::to_string_pretty(config).context("Failed to serialize config")?;
    let path = config_path()?;

    persist_atomic(dir, path.clone(), content).await?;

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
    if std::env::var("KRONN_DATA_DIR").is_err() {
        panic!("test attempted to WRITE the real config.toml — set KRONN_DATA_DIR (tempdir) in this test");
    }
    #[cfg(not(test))]
    if std::env::var("KRONN_DATA_DIR").is_err()
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
    use std::io::Write;
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(
        ".{}.{}.{}.tmp",
        CONFIG_FILE,
        std::process::id(),
        seq
    ));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    #[cfg(unix)]
    {
        // Best-effort: fsync the dir so the rename itself survives a crash.
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
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

fn acquire_lock_in(dir: &std::path::Path) -> Result<std::fs::File> {
    use fs2::FileExt;
    std::fs::create_dir_all(dir)?;
    let lock_path = dir.join(".kronn.lock");
    let f = std::fs::OpenOptions::new()
        .create(true)
        // Pure lock file — its (empty) content is irrelevant, only the flock
        // matters. Explicit no-truncate keeps clippy's suspicious-open-options
        // happy without implying we ever write to it.
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("open data-dir lock {}", lock_path.display()))?;
    f.try_lock_exclusive().map_err(|e| {
        anyhow::anyhow!(
            "another Kronn instance is already running against this data directory \
             ({}). Only one backend may use it at a time.\n\
             \u{2192} Stop the other one first:\n\
             \u{2022}  Docker:  kronn stop\n\
             \u{2022}  native:  pkill -f 'target/debug/kronn' ; pkill -f 'cargo watch -x run'\n\
             then start Kronn again. (lock: {e})",
            dir.display()
        )
    })?;
    Ok(f)
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
pub fn default_config() -> AppConfig {
    AppConfig {
        server: ServerConfig {
            host: "127.0.0.1".into(),
            port: DEFAULT_PORT,
            domain: None,
            auth_token: None,
            auth_enabled: false,
            auth_strict_localhost: false,
            failure_notify_url: None,
            run_retention_days: 0,
            max_concurrent_agents: 5,
            agent_stall_timeout_min: 5,
            pseudo: None,
            avatar_email: None,
            bio: None,
            global_context: None,
            global_context_mode: "always".into(),
            anti_hallucination_mode: crate::core::anti_halluc::DEFAULT_MODE_STR.into(),
            continual_learning_enabled: false, // 0.9.0 — opt-in (beta), see ServerConfig doc
            debug_mode: false,
            default_model_tier: ModelTier::Default,
            // 0.8.6 phase 4 — auto-summary off out of the box. See
            // ServerConfig field docs for rationale (modern agents
            // have large context + MCP access, no auto-summary needed).
            default_summary_strategy: crate::models::SummaryStrategy::Off,
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
                path: None,
                installed: false,
                version: None,
                full_access: false,
            },
            codex: AgentConfig {
                path: None,
                installed: false,
                version: None,
                full_access: false,
            },
            gemini_cli: AgentConfig::default(),
            kiro: AgentConfig::default(),
            vibe: AgentConfig::default(),
            copilot_cli: AgentConfig::default(),
            ollama: AgentConfig::default(),
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

/// Check if this is the first run (no config exists)
pub async fn is_first_run() -> Result<bool> {
    let path = config_path()?;
    Ok(!path.exists())
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

        let mut cfg = default_config();
        let secret = cfg.encryption_secret.clone().expect("default has a secret");
        cfg.server.auth_token = Some("tok".into());
        save(&cfg).await.expect("save must succeed");

        let loaded = load().await.expect("load Ok").expect("Some after save");
        assert_eq!(
            loaded.encryption_secret.as_deref(),
            Some(secret.as_str()),
            "an existing secret must be preserved verbatim across load"
        );

        std::env::remove_var("KRONN_DATA_DIR");
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
            loaded.encryption_secret.is_some(),
            "a complete config (with its secret) must be readable after concurrent saves"
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

    /// `load()` on a config with no `auth_token` must (1) auto-generate one and
    /// (2) set `auth_enabled` to the PLATFORM DEFAULT (`env::auth_on_by_default`),
    /// not blindly trust whatever was on disk. Under `KRONN_DATA_DIR` (= Docker)
    /// the default is OFF — this is the macOS-Docker 401 fix: a generated token
    /// must NOT silently turn auth on and lock the user out on first launch.
    #[tokio::test]
    #[serial]
    async fn load_autogenerates_token_and_defaults_auth_enabled_to_platform() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "kronn-authgen-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("KRONN_DATA_DIR", tmp.to_str().unwrap());

        // Persist a config with NO token and auth_enabled deliberately TRUE,
        // so we can prove load() overrides it from the platform default.
        let mut cfg = default_config();
        cfg.server.auth_token = None;
        cfg.server.auth_enabled = true;
        save(&cfg).await.expect("save must succeed");

        // Docker mode is now the explicit KRONN_IN_DOCKER marker (KRONN_DATA_DIR
        // alone only relocates data — a native user can set it too).
        std::env::set_var("KRONN_IN_DOCKER", "1");
        let loaded = load().await.expect("load Ok").expect("Some after save");
        assert!(
            loaded.server.auth_token.is_some(),
            "load() must auto-generate an auth token when none is set"
        );
        assert!(
            !loaded.server.auth_enabled,
            "auth must default OFF under Docker even though the saved value was true \
             (env::auth_on_by_default() == {})",
            crate::core::env::auth_on_by_default()
        );

        std::env::remove_var("KRONN_IN_DOCKER");
        std::env::remove_var("KRONN_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
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
}
