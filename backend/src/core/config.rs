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

    // Auth token is opt-in — user enables it from the Settings UI.
    // Remove tokens from old auto-generation (before auth_enabled flag existed).
    if config.server.auth_token.is_some() && !config.server.auth_enabled {
        config.server.auth_token = None;
        needs_save = true;
        tracing::info!("Removed legacy auto-generated auth token — re-enable from Settings UI");
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

/// Save config to disk
pub async fn save(config: &AppConfig) -> Result<()> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).await?;

    let content = toml::to_string_pretty(config)
        .context("Failed to serialize config")?;

    let path = config_path()?;
    fs::write(&path, content).await?;

    tracing::info!("Config saved to {}", path.display());
    Ok(())
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
            model_tiers: Default::default(),
        },
        language: "fr".into(),
        disabled_agents: vec![],
        encryption_secret: Some(super::crypto::generate_secret()),
    }
}

/// Check if this is the first run (no config exists)
pub async fn is_first_run() -> Result<bool> {
    let path = config_path()?;
    Ok(!path.exists())
}
