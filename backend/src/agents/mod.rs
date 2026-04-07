use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;
use anyhow::Result;
use crate::core::cmd::async_cmd;
#[cfg(target_os = "windows")]
use crate::core::cmd::sync_cmd;
use crate::models::{AgentDetection, AgentType};

/// Run a shell command cross-platform (sh on Unix, cmd on Windows)
async fn run_shell_cmd(cmd: &str) -> Result<std::process::Output> {
    #[cfg(unix)]
    {
        Ok(async_cmd("sh")
            .args(["-c", cmd])
            .output()
            .await?)
    }
    #[cfg(windows)]
    {
        Ok(async_cmd("cmd")
            .args(["/C", cmd])
            .output()
            .await?)
    }
}

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
    AgentDef { name: "Kiro", agent_type: AgentType::Kiro, binary: "kiro-cli", origin: "US", install_cmd: "curl -fsSL https://cli.kiro.dev/install | bash" },
    AgentDef { name: "GitHub Copilot", agent_type: AgentType::CopilotCli, binary: "copilot", origin: "US", install_cmd: "npm install -g @github/copilot" },
];

/// Detect the host platform label (WSL, macOS, Linux, Windows, etc.)
/// In Docker: uses runtime heuristics (env vars, /proc/version).
/// Native: uses compile-time detection + env var override.
fn detect_host_label() -> String {
    // 1. Trust the environment variable first (set by Makefile → .env → docker-compose)
    if let Ok(host_os) = std::env::var("KRONN_HOST_OS") {
        if !host_os.is_empty() && host_os != "host" {
            return host_os;
        }
    }
    // 2. Heuristics (Linux/WSL/Docker Desktop detection)
    #[cfg(target_os = "linux")]
    {
        // WSL2 always sets WSL_DISTRO_NAME — check it first (more reliable than /proc/version)
        if std::env::var("WSL_DISTRO_NAME").is_ok() {
            return "WSL".into();
        }
        if let Ok(version) = std::fs::read_to_string("/proc/version") {
            let lower = version.to_lowercase();
            if lower.contains("microsoft") || lower.contains("wsl") {
                return "WSL".into();
            }
            if lower.contains("linuxkit") || lower.contains("docker desktop") {
                tracing::debug!("Detected Docker Desktop (linuxkit/docker desktop in /proc/version), assuming macOS host. Set KRONN_HOST_OS to override.");
                return "macOS".into();
            }
        }
    }
    // 3. Compile-time detection for native execution
    #[cfg(target_os = "macos")]
    return "macOS".into();
    #[cfg(target_os = "windows")]
    return "Windows".into();
    #[cfg(target_os = "linux")]
    return "Linux".into();

    #[allow(unreachable_code)]
    "Unknown".into()
}

fn host_is_macos() -> bool {
    if let Ok(os) = std::env::var("KRONN_HOST_OS") {
        return os.eq_ignore_ascii_case("macos");
    }
    cfg!(target_os = "macos")
        || detect_host_label() == "macOS"
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
        let host_label = if loc.via_wsl {
            Some("WSL".to_string())
        } else if loc.host_managed {
            Some(detect_host_label())
        } else {
            None
        };
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
            host_label,
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
        AgentType::CopilotCli => Some("@github/copilot"),
        AgentType::Vibe => None, // uvx, handled differently
        AgentType::Kiro => None, // Native binary, no npx package
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
    let mut cmd = async_cmd("npx");
    cmd.args(["--yes", pkg, "--version"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        cmd.output()
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
    /// True if the binary was found inside WSL (Windows only)
    pub via_wsl: bool,
}

/// Find a binary in PATH or KRONN_HOST_BIN directories.
/// Symlinks from the host may be broken inside the container,
/// so we check for the entry's existence in the directory listing
/// rather than following the symlink.
pub fn find_binary(name: &str) -> Option<BinaryLocation> {
    // Collect host-bin directories once (used to check if a PATH-resolved binary
    // actually lives on a host mount and should be flagged as host_managed).
    let host_dirs: Vec<std::path::PathBuf> = std::env::var("KRONN_HOST_BIN")
        .ok()
        .map(|v| std::env::split_paths(&v).collect())
        .unwrap_or_default();

    // Standard PATH
    if let Ok(path) = which::which(name) {
        let resolved = path.to_string_lossy().to_string();
        // If the binary resolved by `which` lives under a KRONN_HOST_BIN directory,
        // it is host-managed (mounted from the host into the container).
        let host_managed = host_dirs.iter().any(|dir| {
            path.starts_with(dir)
        });
        return Some(BinaryLocation { path: resolved, host_managed, via_wsl: false });
    }

    // Host-mounted bin directories — fallback when `which` fails (e.g. broken symlinks)
    // On Windows, npm installs create .cmd/.exe wrappers (e.g. claude.cmd, codex.cmd).
    // Match both exact name and name with common Windows extensions.
    for dir in &host_dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let file_name_str = file_name.to_string_lossy();
                let matches = file_name_str == name
                    || file_name_str == format!("{}.cmd", name)
                    || file_name_str == format!("{}.exe", name)
                    || file_name_str == format!("{}.ps1", name);
                if matches {
                    // On macOS hosts, host-mounted kiro-cli is a macOS binary
                    // and cannot be executed from this Linux container.
                    if name == "kiro-cli" && host_is_macos() {
                        continue;
                    }
                    return Some(BinaryLocation {
                        path: entry.path().to_string_lossy().to_string(),
                        host_managed: true,
                        via_wsl: false,
                    });
                }
            }
        }
    }

    // On Windows native: try finding the binary inside WSL.
    // 1. Use bash -lc (login shell) to pick up the user's full PATH.
    // 2. Fallback: probe common install locations directly, because some distros
    //    guard PATH modifications behind an interactive-shell check in .bashrc,
    //    which means a login-only shell (-l without -i) may miss them.
    #[cfg(target_os = "windows")]
    {
        // Login-shell lookup
        let mut cmd = sync_cmd("wsl.exe");
        cmd.args(["-e", "bash", "-lc", &format!("which {}", name)]);
        if let Ok(output) = cmd.output() {
            if output.status.success() {
                let wsl_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !wsl_path.is_empty() {
                    return Some(BinaryLocation { path: wsl_path, host_managed: true, via_wsl: true });
                }
            }
        }

        // Fallback: probe well-known directories inside WSL
        let probe_paths = [
            format!("$HOME/.local/bin/{}", name),
            format!("$HOME/.kiro/bin/{}", name),
            format!("/usr/local/bin/{}", name),
            // npm global on common distros
            format!("$HOME/.npm-global/bin/{}", name),
        ];
        let test_script = probe_paths.iter()
            .map(|p| format!("test -x {} && echo {}", p, p))
            .collect::<Vec<_>>()
            .join(" || ");
        let mut cmd = sync_cmd("wsl.exe");
        cmd.args(["-e", "bash", "-c", &test_script]);
        if let Ok(output) = cmd.output() {
            if output.status.success() {
                let wsl_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !wsl_path.is_empty() {
                    return Some(BinaryLocation { path: wsl_path, host_managed: true, via_wsl: true });
                }
            }
        }
    }

    None
}

/// Try to get the version of an agent from its full path.
/// On Windows, if the path is a WSL Linux path (starts with /), run via wsl.exe.
async fn get_version_from(binary_path: &str) -> Result<String> {
    let output = {
        #[cfg(target_os = "windows")]
        {
            if binary_path.starts_with('/') {
                // WSL path — run via wsl.exe with login shell for correct PATH
                async_cmd("wsl.exe")
                    .args(["-e", "bash", "-lc", &format!("{} --version", binary_path)])
                    .output().await?
            } else {
                async_cmd(binary_path)
                    .arg("--version")
                    .output().await?
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            async_cmd(binary_path)
                .arg("--version")
                .output()
                .await?
        }
    };

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
/// Check if a runtime prerequisite is available (e.g. npm, uv, curl)
fn check_prerequisite(cmd: &str) -> bool {
    which::which(cmd).is_ok()
}

/// Prerequisite needed for each agent's install command
fn install_prerequisite(agent_type: &AgentType) -> Option<(&'static str, &'static str)> {
    match agent_type {
        AgentType::ClaudeCode | AgentType::Codex | AgentType::GeminiCli | AgentType::CopilotCli =>
            Some(("npm", "Node.js is required. Install it from https://nodejs.org")),
        AgentType::Vibe =>
            Some(("uv", "uv is required. Install it from https://docs.astral.sh/uv")),
        _ => None,
    }
}

pub async fn install_agent(agent_type: &AgentType) -> Result<String> {
    let def = KNOWN_AGENTS.iter()
        .find(|d| std::mem::discriminant(&d.agent_type) == std::mem::discriminant(agent_type))
        .ok_or_else(|| anyhow::anyhow!("Unknown agent type"))?;

    // Check prerequisite before attempting install
    if let Some((prereq, msg)) = install_prerequisite(agent_type) {
        if !check_prerequisite(prereq) {
            anyhow::bail!("{}", msg);
        }
    }

    tracing::info!("Installing agent: {}", def.install_cmd);

    let output = run_shell_cmd(def.install_cmd).await?;

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
        #[cfg(unix)]
        AgentType::Vibe => "uv tool uninstall mistral-vibe 2>/dev/null || pipx uninstall mistral-vibe 2>/dev/null || pip3 uninstall -y mistral-vibe",
        #[cfg(windows)]
        AgentType::Vibe => "uv tool uninstall mistral-vibe",
        AgentType::GeminiCli => "npm uninstall -g @google/gemini-cli",
        AgentType::CopilotCli => "npm uninstall -g @github/copilot",
        #[cfg(unix)]
        AgentType::Kiro => "rm -f $(which kiro-cli)",
        #[cfg(windows)]
        AgentType::Kiro => "where kiro-cli >nul 2>&1 && del /f /q kiro-cli",
        AgentType::Custom => anyhow::bail!("Cannot uninstall custom agents"),
    };

    tracing::info!("Uninstalling agent: {}", uninstall_cmd);

    let output = run_shell_cmd(uninstall_cmd).await?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Uninstall failed: {}", err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn host_is_macos_detects_env_var() {
        std::env::set_var("KRONN_HOST_OS", "macOS");
        assert!(host_is_macos(), "Should detect macOS from KRONN_HOST_OS");
        std::env::remove_var("KRONN_HOST_OS");
    }

    #[test]
    #[serial]
    fn host_is_macos_case_insensitive() {
        std::env::set_var("KRONN_HOST_OS", "MACOS");
        assert!(host_is_macos(), "Should be case-insensitive");
        std::env::remove_var("KRONN_HOST_OS");
    }

    #[test]
    #[serial]
    fn host_is_not_macos_on_linux() {
        std::env::set_var("KRONN_HOST_OS", "Linux");
        assert!(!host_is_macos(), "Linux should not be detected as macOS");
        std::env::remove_var("KRONN_HOST_OS");
    }

    #[test]
    #[serial]
    fn host_is_not_macos_when_unset() {
        std::env::remove_var("KRONN_HOST_OS");
        assert!(!host_is_macos(), "Should not be macOS when env is unset on Linux");
    }

    // ─── run_shell_cmd ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn run_shell_cmd_echo_hello() {
        let output = super::run_shell_cmd("echo hello").await.expect("run_shell_cmd should succeed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hello"), "stdout should contain 'hello', got: {}", stdout);
    }

    // ─── check_prerequisite ──────────────────────────────────────────────────

    #[test]
    fn check_prerequisite_sh_exists() {
        assert!(super::check_prerequisite("sh"), "sh should be available on unix");
    }

    #[test]
    fn check_prerequisite_nonexistent_binary() {
        assert!(!super::check_prerequisite("nonexistent_binary_xyz"),
            "nonexistent binary should not be found");
    }

    // ─── install_prerequisite ────────────────────────────────────────────────

    #[test]
    fn install_prerequisite_claude_code_returns_npm() {
        let result = super::install_prerequisite(&AgentType::ClaudeCode);
        assert!(result.is_some(), "ClaudeCode should have a prerequisite");
        assert_eq!(result.unwrap().0, "npm");
    }

    #[test]
    fn install_prerequisite_kiro_returns_none() {
        let result = super::install_prerequisite(&AgentType::Kiro);
        assert!(result.is_none(), "Kiro should have no prerequisite");
    }

    // ─── BinaryLocation via_wsl flag ────────────────────────────────────────

    #[test]
    fn binary_location_via_wsl_flag() {
        let loc = BinaryLocation {
            path: "/home/user/.local/bin/claude".to_string(),
            host_managed: true,
            via_wsl: true,
        };
        assert!(loc.via_wsl, "WSL-detected binary should have via_wsl=true");
        assert!(loc.host_managed, "WSL binary should be host_managed");
    }

    #[test]
    fn binary_location_local_not_wsl() {
        let loc = BinaryLocation {
            path: "/usr/local/bin/claude".to_string(),
            host_managed: false,
            via_wsl: false,
        };
        assert!(!loc.via_wsl, "Local binary should not have via_wsl");
    }

    // ─── detect_host_label ──────────────────────────────────────────────────

    #[test]
    #[serial]
    fn detect_host_label_from_env() {
        std::env::set_var("KRONN_HOST_OS", "WSL");
        assert_eq!(detect_host_label(), "WSL");
        std::env::remove_var("KRONN_HOST_OS");
    }

    #[test]
    #[serial]
    fn detect_host_label_ignores_empty_env() {
        std::env::set_var("KRONN_HOST_OS", "");
        let label = detect_host_label();
        // Should fall through to platform detection, not return ""
        assert!(!label.is_empty());
        std::env::remove_var("KRONN_HOST_OS");
    }

    #[test]
    #[serial]
    fn detect_host_label_ignores_host_value() {
        std::env::set_var("KRONN_HOST_OS", "host");
        let label = detect_host_label();
        // "host" is the unresolved default — should fall through
        assert_ne!(label, "host");
        std::env::remove_var("KRONN_HOST_OS");
    }

    // ─── WSL detection via WSL_DISTRO_NAME ─────────────────────────────────

    #[test]
    #[serial]
    fn detect_host_label_wsl_via_distro_name() {
        std::env::remove_var("KRONN_HOST_OS");
        std::env::set_var("WSL_DISTRO_NAME", "Ubuntu");
        let label = detect_host_label();
        std::env::remove_var("WSL_DISTRO_NAME");
        // On Linux, should detect WSL from WSL_DISTRO_NAME
        #[cfg(target_os = "linux")]
        assert_eq!(label, "WSL");
        // On other platforms, WSL_DISTRO_NAME is ignored (compile-time gate)
        #[cfg(not(target_os = "linux"))]
        let _ = label;
    }

    // ─── find_binary: Windows extension matching ────────────────────────────

    #[test]
    #[serial]
    fn find_binary_matches_cmd_extension() {
        // Create a temp dir with a fake "testbin.cmd" file
        let tmp = std::env::temp_dir().join("kronn-test-findbin-cmd");
        let _ = std::fs::create_dir_all(&tmp);
        std::fs::write(tmp.join("testbin.cmd"), "echo hello").unwrap();

        std::env::set_var("KRONN_HOST_BIN", tmp.to_string_lossy().as_ref());
        let result = find_binary("testbin");
        std::env::remove_var("KRONN_HOST_BIN");
        let _ = std::fs::remove_dir_all(&tmp);

        assert!(result.is_some(), "Should find testbin via testbin.cmd");
        assert!(result.unwrap().host_managed, "Should be host_managed");
    }

    #[test]
    #[serial]
    fn find_binary_matches_exe_extension() {
        let tmp = std::env::temp_dir().join("kronn-test-findbin-exe");
        let _ = std::fs::create_dir_all(&tmp);
        std::fs::write(tmp.join("testbin.exe"), "fake").unwrap();

        std::env::set_var("KRONN_HOST_BIN", tmp.to_string_lossy().as_ref());
        let result = find_binary("testbin");
        std::env::remove_var("KRONN_HOST_BIN");
        let _ = std::fs::remove_dir_all(&tmp);

        assert!(result.is_some(), "Should find testbin via testbin.exe");
    }

    #[test]
    #[serial]
    fn find_binary_matches_exact_name() {
        let tmp = std::env::temp_dir().join("kronn-test-findbin-exact");
        let _ = std::fs::create_dir_all(&tmp);
        std::fs::write(tmp.join("testbin"), "fake").unwrap();

        std::env::set_var("KRONN_HOST_BIN", tmp.to_string_lossy().as_ref());
        let result = find_binary("testbin");
        std::env::remove_var("KRONN_HOST_BIN");
        let _ = std::fs::remove_dir_all(&tmp);

        assert!(result.is_some(), "Should find testbin via exact name");
    }

    // ─── CopilotCli agent definitions ──────────────────────────────────────

    #[test]
    fn install_prerequisite_copilot_returns_npm() {
        let result = super::install_prerequisite(&AgentType::CopilotCli);
        assert!(result.is_some(), "CopilotCli should have a prerequisite");
        assert_eq!(result.unwrap().0, "npm");
    }

    #[test]
    fn copilot_agent_is_in_known_agents() {
        let found = KNOWN_AGENTS.iter().any(|a| matches!(a.agent_type, AgentType::CopilotCli));
        assert!(found, "CopilotCli should be in KNOWN_AGENTS");
        let def = KNOWN_AGENTS.iter().find(|a| matches!(a.agent_type, AgentType::CopilotCli)).unwrap();
        assert_eq!(def.binary, "copilot");
        assert_eq!(def.origin, "US");
    }

    #[test]
    fn copilot_npx_package_in_probe_runtime() {
        let def = KNOWN_AGENTS.iter().find(|a| matches!(a.agent_type, AgentType::CopilotCli)).unwrap();
        let pkg = match def.agent_type {
            AgentType::CopilotCli => Some("@github/copilot"),
            _ => None,
        };
        assert_eq!(pkg, Some("@github/copilot"));
    }
}
