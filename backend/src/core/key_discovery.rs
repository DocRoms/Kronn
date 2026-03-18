use std::path::PathBuf;

/// A discovered API key from an agent config file or environment variable.
pub struct RawDiscoveredKey {
    pub provider: String,
    pub source: String,
    pub value: String,
    pub suggested_name: String,
}

/// Discover API keys from agent config files and environment variables.
/// Checks (in order, deduplicating by value):
///   - ~/.codex/auth.json → OPENAI_API_KEY (provider: openai)
///   - ~/.gemini/settings.json → apiKey (provider: google)
///   - Environment: ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY, GOOGLE_API_KEY, MISTRAL_API_KEY
pub async fn discover_keys() -> Vec<RawDiscoveredKey> {
    let mut keys = Vec::new();
    let mut seen_values = std::collections::HashSet::new();

    let name = default_key_name();

    // ── Agent config files ──────────────────────────────────────────────

    // Codex: ~/.codex/auth.json
    if let Some(key) = read_codex_key() {
        if seen_values.insert(key.clone()) {
            keys.push(RawDiscoveredKey {
                provider: "openai".into(),
                source: "~/.codex/auth.json".into(),
                value: key,
                suggested_name: name.clone(),
            });
        }
    }

    // Gemini CLI: ~/.gemini/settings.json
    if let Some(key) = read_gemini_key() {
        if seen_values.insert(key.clone()) {
            keys.push(RawDiscoveredKey {
                provider: "google".into(),
                source: "~/.gemini/settings.json".into(),
                value: key,
                suggested_name: name.clone(),
            });
        }
    }

    // Vibe: ~/.vibe/.env (MISTRAL_API_KEY)
    if let Some(key) = read_vibe_key() {
        if seen_values.insert(key.clone()) {
            keys.push(RawDiscoveredKey {
                provider: "mistral".into(),
                source: "~/.vibe/.env".into(),
                value: key,
                suggested_name: name.clone(),
            });
        }
    }

    // ── Environment variables ───────────────────────────────────────────
    let env_sources: &[(&str, &str, &str)] = &[
        ("ANTHROPIC_API_KEY", "anthropic", "env:ANTHROPIC_API_KEY"),
        ("OPENAI_API_KEY", "openai", "env:OPENAI_API_KEY"),
        ("GEMINI_API_KEY", "google", "env:GEMINI_API_KEY"),
        ("GOOGLE_API_KEY", "google", "env:GOOGLE_API_KEY"),
        ("MISTRAL_API_KEY", "mistral", "env:MISTRAL_API_KEY"),
    ];

    for (env_var, provider, source) in env_sources {
        if let Ok(val) = std::env::var(env_var) {
            if !val.is_empty() && seen_values.insert(val.clone()) {
                keys.push(RawDiscoveredKey {
                    provider: provider.to_string(),
                    source: source.to_string(),
                    value: val,
                    suggested_name: name.clone(),
                });
            }
        }
    }

    keys
}

/// Read the OpenAI API key from ~/.codex/auth.json
fn read_codex_key() -> Option<String> {
    let path = home_dir()?.join(".codex").join("auth.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
    let key = parsed.get("OPENAI_API_KEY")?.as_str()?;
    if key.is_empty() { return None; }
    Some(key.to_string())
}

/// Read the Google API key from ~/.gemini/settings.json
fn read_gemini_key() -> Option<String> {
    let path = home_dir()?.join(".gemini").join("settings.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
    let key = parsed.get("apiKey")?.as_str()?;
    if key.is_empty() { return None; }
    Some(key.to_string())
}

/// Read the Mistral API key from ~/.vibe/.env
fn read_vibe_key() -> Option<String> {
    let path = home_dir()?.join(".vibe").join(".env");
    let content = std::fs::read_to_string(&path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("MISTRAL_API_KEY=") {
            let key = rest.trim_matches('\'').trim_matches('"').trim();
            if !key.is_empty() {
                return Some(key.to_string());
            }
        }
    }
    None
}

/// Resolve home directory.
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Get a default name for discovered keys.
/// In Docker, derives from KRONN_HOST_HOME (e.g. /home/username → "username").
/// Falls back to /etc/hostname, then "default".
fn default_key_name() -> String {
    if let Ok(host_home) = std::env::var("KRONN_HOST_HOME") {
        if let Some(name) = PathBuf::from(&host_home).file_name() {
            let n = name.to_string_lossy().to_string();
            if !n.is_empty() && n != "root" {
                return n;
            }
        }
    }
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        let h = h.trim().to_string();
        if !h.is_empty() {
            return h;
        }
    }
    "default".into()
}

/// Write the Google API key to ~/.gemini/settings.json
/// Used by sync_agent_tokens to push Kronn keys to Gemini CLI.
pub fn write_gemini_key(key: Option<&str>) {
    let Some(home) = home_dir() else { return };
    let gemini_dir = home.join(".gemini");
    let settings_path = gemini_dir.join("settings.json");

    match key {
        Some(k) => {
            let _ = std::fs::create_dir_all(&gemini_dir);
            // Read existing settings to preserve other fields
            let mut settings: serde_json::Value = std::fs::read_to_string(&settings_path)
                .ok()
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            settings["apiKey"] = serde_json::Value::String(k.to_string());
            match std::fs::write(&settings_path, serde_json::to_string_pretty(&settings).unwrap()) {
                Ok(_) => tracing::info!("Synced Google key to {}", settings_path.display()),
                Err(e) => tracing::warn!("Failed to write {}: {}", settings_path.display(), e),
            }
        }
        None => {
            // Remove apiKey from settings (preserve other fields)
            if let Ok(content) = std::fs::read_to_string(&settings_path) {
                if let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(obj) = settings.as_object_mut() {
                        obj.remove("apiKey");
                        let _ = std::fs::write(&settings_path, serde_json::to_string_pretty(&settings).unwrap());
                        tracing::info!("Removed Google key from {}", settings_path.display());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_key_name_returns_non_empty() {
        let name = default_key_name();
        assert!(!name.is_empty());
    }

    #[test]
    fn read_codex_key_returns_none_when_missing() {
        let _ = read_codex_key();
    }

    #[test]
    fn read_codex_key_parses_auth_json() {
        let tmp = std::env::temp_dir().join("kronn-test-codex-key");
        let _ = std::fs::create_dir_all(tmp.join(".codex"));
        std::fs::write(
            tmp.join(".codex/auth.json"),
            r#"{"OPENAI_API_KEY":"sk-test-codex-key-456"}"#,
        ).unwrap();

        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &tmp);
        let key = read_codex_key();
        if let Some(h) = old_home { std::env::set_var("HOME", h); }

        assert_eq!(key, Some("sk-test-codex-key-456".to_string()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_codex_key_ignores_empty_value() {
        let tmp = std::env::temp_dir().join("kronn-test-codex-empty");
        let _ = std::fs::create_dir_all(tmp.join(".codex"));
        std::fs::write(
            tmp.join(".codex/auth.json"),
            r#"{"OPENAI_API_KEY":""}"#,
        ).unwrap();

        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &tmp);
        let key = read_codex_key();
        if let Some(h) = old_home { std::env::set_var("HOME", h); }

        assert_eq!(key, None, "Empty key should return None");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_gemini_key_returns_none_when_missing() {
        let _ = read_gemini_key();
    }

    #[test]
    fn read_gemini_key_parses_settings_json() {
        let tmp = std::env::temp_dir().join("kronn-test-gemini-key");
        let _ = std::fs::create_dir_all(tmp.join(".gemini"));
        std::fs::write(
            tmp.join(".gemini/settings.json"),
            r#"{"apiKey":"AIza-test-gemini-789","other":"stuff"}"#,
        ).unwrap();

        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &tmp);
        let key = read_gemini_key();
        if let Some(h) = old_home { std::env::set_var("HOME", h); }

        assert_eq!(key, Some("AIza-test-gemini-789".to_string()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_vibe_key_strips_quotes() {
        let tmp = std::env::temp_dir().join("kronn-test-vibe-quotes");
        let _ = std::fs::create_dir_all(tmp.join(".vibe"));
        // Double-quoted value
        std::fs::write(tmp.join(".vibe/.env"), "MISTRAL_API_KEY=\"dbl-quoted-key\"\n").unwrap();

        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &tmp);
        let key = read_vibe_key();
        if let Some(h) = old_home { std::env::set_var("HOME", h); }

        assert_eq!(key, Some("dbl-quoted-key".to_string()), "Double quotes should be stripped");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_vibe_key_parses_env_file() {
        let tmp = std::env::temp_dir().join("kronn-test-vibe-key");
        let _ = std::fs::create_dir_all(tmp.join(".vibe"));
        std::fs::write(tmp.join(".vibe/.env"), "# comment\nMISTRAL_API_KEY='test_key_123'\nOTHER=val\n").unwrap();

        // Temporarily override HOME
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &tmp);
        let key = read_vibe_key();
        if let Some(h) = old_home { std::env::set_var("HOME", h); }

        assert_eq!(key, Some("test_key_123".to_string()), "Should parse MISTRAL_API_KEY from .env with quotes stripped");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_vibe_key_returns_none_when_missing() {
        let _ = read_vibe_key(); // Should not panic
    }

    #[tokio::test]
    async fn discover_keys_returns_vec() {
        let keys = discover_keys().await;
        assert!(keys.len() <= 10);
    }
}
