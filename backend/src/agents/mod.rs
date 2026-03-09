use anyhow::Result;
use crate::models::{AgentDetection, AgentType};

pub mod runner;

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
];

/// Detect all known agents on the system
pub async fn detect_all() -> Vec<AgentDetection> {
    let mut agents = Vec::new();
    for def in KNOWN_AGENTS {
        agents.push(detect_agent(def).await);
    }
    agents
}

/// Detect a single agent by checking if its binary exists in PATH or host bin dirs
async fn detect_agent(def: &AgentDef) -> AgentDetection {
    // Check standard PATH first, then host-mounted bin directories
    let found = find_binary(def.binary);

    if let Some(path) = found {
        // Version detection may fail if symlinks are broken inside container
        let version = get_version_from(&path).await.ok();
        AgentDetection {
            name: def.name.to_string(),
            agent_type: def.agent_type.clone(),
            installed: true,
            enabled: true,
            path: Some(path),
            version,
            latest_version: None,
            origin: def.origin.to_string(),
            install_command: Some(def.install_cmd.to_string()),
        }
    } else {
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
        }
    }
}

/// Find a binary in PATH or KRONN_HOST_BIN directories.
/// Symlinks from the host may be broken inside the container,
/// so we check for the entry's existence in the directory listing
/// rather than following the symlink.
pub fn find_binary(name: &str) -> Option<String> {
    // Standard PATH
    if let Ok(path) = which::which(name) {
        return Some(path.to_string_lossy().to_string());
    }

    // Host-mounted bin directories (KRONN_HOST_BIN=dir1:dir2:...)
    if let Ok(host_bin) = std::env::var("KRONN_HOST_BIN") {
        for dir in host_bin.split(':') {
            let dir_path = std::path::Path::new(dir);
            if let Ok(entries) = std::fs::read_dir(dir_path) {
                for entry in entries.flatten() {
                    if entry.file_name() == name {
                        return Some(entry.path().to_string_lossy().to_string());
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
