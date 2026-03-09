use std::path::PathBuf;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use tokio::fs;

use crate::models::{
    AgentConfig, AgentsConfig, AppConfig, ScanConfig, ServerConfig, TokensConfig,
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

    // Ensure encryption secret exists (migrate older configs)
    if config.encryption_secret.is_none() {
        config.encryption_secret = Some(super::crypto::generate_secret());
        // Save back so the secret persists
        let updated = toml::to_string_pretty(&config)
            .context("Failed to serialize config")?;
        tokio::fs::write(&path, updated).await?;
        tracing::info!("Generated encryption secret for existing config");
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
        },
        tokens: TokensConfig {
            anthropic: None,
            openai: None,
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
            ],
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
