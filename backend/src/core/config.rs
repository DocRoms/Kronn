use std::path::PathBuf;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use tokio::fs;

use crate::models::{
    AgentConfig, AgentsConfig, ApiKey, AppConfig, ScanConfig, ServerConfig, TokensConfig,
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

    let mut config: AppConfig = toml::from_str(&content)
        .context("Failed to parse config file")?;

    let mut needs_save = false;

    // Ensure encryption secret exists (migrate older configs)
    if config.encryption_secret.is_none() {
        config.encryption_secret = Some(super::crypto::generate_secret());
        needs_save = true;
        tracing::info!("Generated encryption secret for existing config");
    }

    // Auto-generate auth token on first launch.
    // Auth is on by default — localhost requests bypass it (see auth_middleware),
    // but remote peers (multi-user) must provide the token.
    if config.server.auth_token.is_none() {
        config.server.auth_token = Some(uuid::Uuid::new_v4().to_string());
        config.server.auth_enabled = true;
        needs_save = true;
        tracing::info!("Generated auth token for API security (localhost exempt, peers require Bearer token)");
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
            tracing::info!("Migrated {} legacy API key(s) to multi-key format", config.tokens.keys.len());
        }
    }

    if needs_save {
        let updated = toml::to_string_pretty(&config)
            .context("Failed to serialize config")?;
        tokio::fs::write(&path, updated).await?;
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

    let content = toml::to_string_pretty(config)
        .context("Failed to serialize config")?;

    let path = config_path()?;
    fs::write(&path, content).await?;
    restrict_permissions(&path, false).await;

    tracing::info!("Config saved to {}", path.display());
    Ok(())
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
            max_concurrent_agents: 5,
            agent_stall_timeout_min: 5,
            pseudo: None,
            avatar_email: None,
            bio: None,
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
            model_tiers: Default::default(),
        },
        language: "fr".into(),
        ui_language: "fr".into(),
        stt_model: None,
        tts_voices: std::collections::HashMap::new(),
        disabled_agents: vec![],
        encryption_secret: Some(super::crypto::generate_secret()),
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

    #[test]
    fn default_config_is_valid() {
        let cfg = default_config();
        assert!(!cfg.server.host.is_empty(), "host must be non-empty");
        assert!(cfg.server.port > 0, "port must be > 0");
        assert!(cfg.encryption_secret.is_some(), "encryption_secret must be set");
        assert!(!cfg.encryption_secret.as_ref().unwrap().is_empty(), "encryption_secret must be non-empty");
    }

    #[test]
    fn config_dir_returns_path() {
        // May fail in exotic CI environments without HOME, but works in normal setups
        let dir = config_dir();
        assert!(dir.is_ok(), "config_dir() should return Ok: {:?}", dir.err());
        let path = dir.unwrap();
        assert!(!path.as_os_str().is_empty(), "config dir path must be non-empty");
    }

    #[test]
    fn config_path_ends_in_config_toml() {
        let path = config_path();
        assert!(path.is_ok(), "config_path() should return Ok: {:?}", path.err());
        let p = path.unwrap();
        assert!(
            p.to_string_lossy().ends_with("config.toml"),
            "config path should end in config.toml, got: {}",
            p.display()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn save_chmods_dir_700_and_file_600_on_unix() {
        // Real save() round-trip via KRONN_DATA_DIR override so we don't
        // touch the user's real ~/.config/kronn during tests.
        use std::os::unix::fs::PermissionsExt;

        let tmp = std::env::temp_dir().join(format!(
            "kronn-config-perms-{}",
            std::process::id()
        ));
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
}
