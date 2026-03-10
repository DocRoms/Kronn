use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;
use anyhow::Result;
use crate::models::{AgentDetection, AgentType};

pub mod runner;

/// Cache for runtime probe results (npx availability).
/// Key: binary name, Value: (available, probed_at)
static RUNTIME_CACHE: std::sync::LazyLock<Mutex<HashMap<String, (bool, Instant)>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// How long to cache a runtime probe result
const RUNTIME_CACHE_TTL_SECS: u64 = 300; // 5 minutes

struct AgentDef {
    name: &'static str,
    agent_type: AgentType,
    binary: &'static str,
    origin: &'static str,
    install_cmd: &'static str,
}

const KNOWN_AGENTS: &[AgentDef] = &[
    AgentDef { name: "Claude Code", agent_type: AgentType::ClaudeCode, binary: "claude", origin: "US", install_cmd: "npm install -g @anthropic-ai/claude-code" },
    AgentDef { name: "Codex", agent_type: AgentType::Codex, binary: "codex", origin: "US", install_cmd: "npm install -g @openai/codex" },
    AgentDef { name: "Vibe", agent_type: AgentType::Vibe, binary: "vibe", origin: "EU", install_cmd: "uv tool install mistral-vibe" },
    AgentDef { name: "Gemini CLI", agent_type: AgentType::GeminiCli, binary: "gemini", origin: "US", install_cmd: "npm install -g @google/gemini-cli" },
];

/// Detect the host platform label (WSL, macOS, Linux, etc.)
/// The backend runs inside Docker (always Linux), so we use runtime
/// heuristics rather than compile-time cfg!() checks.
fn detect_host_label() -> String {
    // Docker on WSL2 shares the kernel — /proc/version contains "microsoft"
    if let Ok(version) = std::fs::read_to_string("/proc/version") {
        if version.contains("microsoft") || version.contains("WSL") {
            return "WSL".into();
        }
    }
    // Docker Desktop on macOS uses a LinuxKit VM — check /proc/version
    if let Ok(version) = std::fs::read_to_string("/proc/version") {
        if version.contains("linuxkit") || version.contains("Docker Desktop") {
            return "macOS".into();
        }
    }
    // Env var fallback: docker-compose can pass KRONN_HOST_OS=macOS
    if let Ok(host_os) = std::env::var("KRONN_HOST_OS") {
        return host_os;
    }
    "host".into()
}

/// Detect all known agents on the system
pub async fn detect_all() -> Vec<AgentDetection> {
    let mut agents = Vec::new();
    for def in KNOWN_AGENTS {
        agents.push(detect_agent(def).await);
    }
    agents
}

/// Detect a single agent by checking if its binary exists in PATH or host bin dirs.
/// If no local binary is found but the agent has an npx package, probe runtime availability.
async fn detect_agent(def: &AgentDef) -> AgentDetection {
    // Check standard PATH first, then host-mounted bin directories
    let found = find_binary(def.binary);

    if let Some(loc) = found {
        // Version detection may fail if symlinks are broken inside container
        let version = get_version_from(&loc.path).await.ok();
        AgentDetection {
            name: def.name.to_string(),
            agent_type: def.agent_type.clone(),
            installed: true,
            enabled: true,
            path: Some(loc.path),
            version,
            latest_version: None,
            origin: def.origin.to_string(),
            install_command: Some(def.install_cmd.to_string()),
            host_managed: loc.host_managed,
            host_label: if loc.host_managed { Some(detect_host_label()) } else { None },
            runtime_available: true,
        }
    } else {
        // No local binary — probe npx/uvx fallback
        let runtime_available = probe_runtime(def).await;
        AgentDetection {
            name: def.name.to_string(),
            agent_type: def.agent_type.clone(),
            installed: false,
            enabled: true,
            path: None,
            version: None,
            latest_version: None,
            origin: def.origin.to_string(),
            install_command: Some(def.install_cmd.to_string()),
            host_managed: false,
            host_label: None,
            runtime_available,
        }
    }
}

/// Probe whether an agent is runnable via npx/uvx, with caching.
/// Uses the same fallback path as the runner: `npx --yes <pkg> --version`.
async fn probe_runtime(def: &AgentDef) -> bool {
    let npx_pkg = match def.agent_type {
        AgentType::ClaudeCode => Some("@anthropic-ai/claude-code"),
        AgentType::Codex => Some("@openai/codex"),
        AgentType::GeminiCli => Some("@google/gemini-cli"),
        AgentType::Vibe => None, // uvx, handled differently
        AgentType::Custom => None,
    };

    let Some(pkg) = npx_pkg else { return false };

    // Check cache
    let cache_key = def.binary.to_string();
    if let Ok(cache) = RUNTIME_CACHE.lock() {
        if let Some((available, probed_at)) = cache.get(&cache_key) {
            if probed_at.elapsed().as_secs() < RUNTIME_CACHE_TTL_SECS {
                return *available;
            }
        }
    }

    // Probe: npx --yes <pkg> --version with 15s timeout
    tracing::info!("Probing runtime for {} via npx {}", def.name, pkg);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::process::Command::new("npx")
            .args(["--yes", pkg, "--version"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
    ).await;

    let available = match result {
        Ok(Ok(output)) => output.status.success(),
        _ => false,
    };

    // Update cache
    if let Ok(mut cache) = RUNTIME_CACHE.lock() {
        cache.insert(cache_key, (available, Instant::now()));
    }

    tracing::info!("Runtime probe for {}: {}", def.name, if available { "OK" } else { "unavailable" });
    available
}

/// Result of finding a binary: path + whether it comes from the host
pub struct BinaryLocation {
    pub path: String,
    pub host_managed: bool,
}

/// Find a binary in PATH or KRONN_HOST_BIN directories.
/// Symlinks from the host may be broken inside the container,
/// so we check for the entry's existence in the directory listing
/// rather than following the symlink.
pub fn find_binary(name: &str) -> Option<BinaryLocation> {
    // Standard PATH
    if let Ok(path) = which::which(name) {
        return Some(BinaryLocation {
            path: path.to_string_lossy().to_string(),
            host_managed: false,
        });
    }

    // Host-mounted bin directories (KRONN_HOST_BIN=dir1:dir2:...)
    if let Ok(host_bin) = std::env::var("KRONN_HOST_BIN") {
        for dir in host_bin.split(':') {
            let dir_path = std::path::Path::new(dir);
            if let Ok(entries) = std::fs::read_dir(dir_path) {
                for entry in entries.flatten() {
                    if entry.file_name() == name {
                        return Some(BinaryLocation {
                            path: entry.path().to_string_lossy().to_string(),
                            host_managed: true,
                        });
                    }
                }
            }
        }
    }

    None
}

/// Try to get the version of an agent from its full path
async fn get_version_from(binary_path: &str) -> Result<String> {
    let output = tokio::process::Command::new(binary_path)
        .arg("--version")
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Some tools print version to stderr
    let version_str = if stdout.trim().is_empty() {
        stderr.trim().to_string()
    } else {
        stdout.trim().to_string()
    };

    // Extract semver pattern
    let raw = version_str.lines().next().unwrap_or(&version_str);
    if let Some(m) = regex_lite::Regex::new(r"\d+\.\d+\.\d+")
        .ok()
        .and_then(|re| re.find(raw))
    {
        Ok(m.as_str().to_string())
    } else {
        Ok(raw.to_string())
    }
}

/// Install an agent (runs the install command)
pub async fn install_agent(agent_type: &AgentType) -> Result<String> {
    let def = KNOWN_AGENTS.iter()
        .find(|d| std::mem::discriminant(&d.agent_type) == std::mem::discriminant(agent_type))
        .ok_or_else(|| anyhow::anyhow!("Unknown agent type"))?;

    tracing::info!("Installing agent: {}", def.install_cmd);

    let output = tokio::process::Command::new("sh")
        .args(["-c", def.install_cmd])
        .output()
        .await?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Installation failed: {}", err)
    }
}

/// Uninstall an agent
pub async fn uninstall_agent(agent_type: &AgentType) -> Result<String> {
    let def = KNOWN_AGENTS.iter()
        .find(|d| std::mem::discriminant(&d.agent_type) == std::mem::discriminant(agent_type))
        .ok_or_else(|| anyhow::anyhow!("Unknown agent type"))?;

    let uninstall_cmd = match def.agent_type {
        AgentType::ClaudeCode => "npm uninstall -g @anthropic-ai/claude-code",
        AgentType::Codex => "npm uninstall -g @openai/codex",
        AgentType::Vibe => "uv tool uninstall mistral-vibe 2>/dev/null || pipx uninstall mistral-vibe 2>/dev/null || pip3 uninstall -y mistral-vibe",
        AgentType::GeminiCli => "npm uninstall -g @google/gemini-cli",
        AgentType::Custom => anyhow::bail!("Cannot uninstall custom agents"),
    };

    tracing::info!("Uninstalling agent: {}", uninstall_cmd);

    let output = tokio::process::Command::new("sh")
        .args(["-c", uninstall_cmd])
        .output()
        .await?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Uninstall failed: {}", err)
    }
}
