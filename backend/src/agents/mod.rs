use crate::core::cmd::async_cmd;
#[cfg(target_os = "windows")]
use crate::core::cmd::sync_cmd;
use crate::models::{AgentDetection, AgentType};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// Run a shell command cross-platform (sh on Unix, cmd on Windows)
async fn run_shell_cmd(cmd: &str) -> Result<std::process::Output> {
    #[cfg(unix)]
    {
        Ok(async_cmd("sh").args(["-c", cmd]).output().await?)
    }
    #[cfg(windows)]
    {
        Ok(async_cmd("cmd").args(["/C", cmd]).output().await?)
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
    AgentDef {
        name: "Claude Code",
        agent_type: AgentType::ClaudeCode,
        binary: "claude",
        origin: "US",
        install_cmd: "npm install -g @anthropic-ai/claude-code",
    },
    AgentDef {
        name: "Codex",
        agent_type: AgentType::Codex,
        binary: "codex",
        origin: "US",
        install_cmd: "npm install -g @openai/codex",
    },
    AgentDef {
        name: "Vibe",
        agent_type: AgentType::Vibe,
        binary: "vibe",
        origin: "EU",
        install_cmd: "uv tool install mistral-vibe",
    },
    AgentDef {
        name: "Gemini CLI",
        agent_type: AgentType::GeminiCli,
        binary: "gemini",
        origin: "US",
        install_cmd: "npm install -g @google/gemini-cli",
    },
    AgentDef {
        name: "Kiro",
        agent_type: AgentType::Kiro,
        binary: "kiro-cli",
        origin: "US",
        install_cmd: "curl -fsSL https://cli.kiro.dev/install | bash",
    },
    AgentDef {
        name: "GitHub Copilot",
        agent_type: AgentType::CopilotCli,
        binary: "copilot",
        origin: "US",
        install_cmd: "npm install -g @github/copilot",
    },
    AgentDef {
        name: "Ollama",
        agent_type: AgentType::Ollama,
        binary: "ollama",
        origin: "US",
        install_cmd: "curl -fsSL https://ollama.com/install.sh | sh",
    },
];

/// Binaries that must NEVER be resolved from the host-mounted bin directory
/// when running in a Linux container on a macOS host. The host-mounted
/// binaries are Darwin (Mach-O) and will fail to execute under `exec()` in
/// the container; `entrypoint.sh` re-installs Linux versions via npm/curl.
///
/// Keep this list in sync with `entrypoint.sh`. The
/// `cross_agent_macos_skip_covers_npm_agents` test enforces that every
/// `npm install`-style agent is present here.
pub(crate) const MACOS_HOST_BIN_SKIP: &[&str] =
    &["claude", "codex", "gemini", "copilot", "kiro-cli"];

/// Pure: should a *resolved* binary be ignored because it is a Darwin host
/// binary that cannot `exec()` in this Linux container?
///
/// This must be consulted on EVERY resolution path, including the primary
/// `which::which` lookup — the container `PATH` includes the host-mounted
/// `~/.local/bin` (`/host-bin/local`), so on a macOS host `which` happily
/// returns the Darwin `claude` and, without this guard, version detection
/// execs it and surfaces an "Exec format error" instead of falling back to the
/// npx runtime / the Linux copy installed by `entrypoint.sh`.
pub(crate) fn should_skip_darwin_host_binary(
    name: &str,
    host_managed: bool,
    host_is_macos: bool,
) -> bool {
    host_managed && host_is_macos && MACOS_HOST_BIN_SKIP.contains(&name)
}

/// Public accessor so `/api/health` can stamp the host label into its body
/// (used by the "Report bug" flow to pre-fill a GitHub issue template).
/// Keeping the core helper private preserves the module boundary.
pub fn detect_host_label_public() -> String {
    detect_host_label()
}

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
    cfg!(target_os = "macos") || detect_host_label() == "macOS"
}

/// Detect all known agents on the system
pub async fn detect_all() -> Vec<AgentDetection> {
    // One-shot env dump at the start of a detection sweep. Critical for
    // diagnosing "why can't macOS see my agents" — the 4 env vars below
    // fully determine the detection logic and a mismatch between
    // Makefile/.env/docker-compose/entrypoint is the usual culprit.
    tracing::info!(
        target: "kronn::agent_detect",
        host_os = %std::env::var("KRONN_HOST_OS").unwrap_or_else(|_| "<unset>".into()),
        host_home = %std::env::var("KRONN_HOST_HOME").unwrap_or_else(|_| "<unset>".into()),
        host_bin = %std::env::var("KRONN_HOST_BIN").unwrap_or_else(|_| "<unset>".into()),
        host_label = %detect_host_label(),
        "starting agent detection sweep",
    );

    // Probe every agent CONCURRENTLY. Each `detect_agent` spawns a `<binary>
    // --version` subprocess (and possibly an npx fallback), so running them
    // sequentially made the sweep ~N×(spawn latency) — ~13s on a 7-agent
    // host-managed WSL setup. join_all fires them at once → ~the slowest single
    // probe. Order is preserved (join_all keeps input order).
    let agents: Vec<AgentDetection> =
        futures::future::join_all(KNOWN_AGENTS.iter().map(detect_agent)).await;

    // Summary — lets the user grep `kronn::agent_detect` and see in one
    // place which agents the container found and which are missing.
    let summary: Vec<String> = agents
        .iter()
        .map(|a| {
            format!(
                "{:?}={}{}",
                a.agent_type,
                if a.installed {
                    "installed"
                } else if a.runtime_available {
                    "runtime"
                } else {
                    "missing"
                },
                a.path
                    .as_ref()
                    .map(|p| format!(" ({})", p))
                    .unwrap_or_default(),
            )
        })
        .collect();
    tracing::info!(target: "kronn::agent_detect", "sweep done: {}", summary.join(", "));

    agents
}

/// TTL for the whole-sweep cache below.
const DETECT_ALL_TTL: std::time::Duration = std::time::Duration::from_secs(60);
/// Soft wall on a single sweep when a STALE result already exists: under heavy
/// concurrent-agent load `get_version_from`'s `<binary> --version` spawns can
/// stall, and `setup/status` (which the boot blocks on) shouldn't hang — so on
/// overrun we keep serving the previous (still-valid) sweep. Generous, because
/// a legitimate sweep on a multi-agent host runs ~4-5s and we'd rather let it
/// COMPLETE (and refresh the cache) than churn. NEVER applied to a cold cache:
/// returning an empty agent list breaks discussion creation + the Agents page,
/// so a first/cold sweep always runs to completion (see detect_all_cached).
const DETECT_ALL_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);
/// Cache payload: the last completed sweep + the instant it was taken.
type DetectAllCache = Mutex<Option<(Vec<AgentDetection>, Instant)>>;
static DETECT_ALL_CACHE: std::sync::LazyLock<DetectAllCache> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// Cached agent-detection sweep. `detect_all` spawns `<binary> --version` for
/// every installed agent on every call (~5s idle; under concurrent-agent load
/// the subprocess spawns contend and the call can hang for tens of seconds).
/// `setup/status` runs on every dashboard boot AND the frontend boot blocks on
/// it, so an uncached sweep froze the whole app on "Almost ready…" whenever a
/// batch/WF was spawning agents. Cache the result for `DETECT_ALL_TTL`.
/// `force = true` bypasses + refreshes (right after an install/uninstall).
pub async fn detect_all_cached(force: bool) -> Vec<AgentDetection> {
    if !force {
        if let Ok(guard) = DETECT_ALL_CACHE.lock() {
            if let Some((cached, at)) = guard.as_ref() {
                if at.elapsed() < DETECT_ALL_TTL {
                    return cached.clone();
                }
            }
        }
    }
    // Snapshot any previous (stale) sweep to fall back on.
    let stale: Option<Vec<AgentDetection>> = DETECT_ALL_CACHE
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|(v, _)| v.clone()));

    let store = |fresh: Vec<AgentDetection>| -> Vec<AgentDetection> {
        if let Ok(mut guard) = DETECT_ALL_CACHE.lock() {
            *guard = Some((fresh.clone(), Instant::now()));
        }
        fresh
    };

    match stale {
        // COLD cache: an empty agent list breaks discussion creation + the
        // Agents page, so the first sweep MUST produce real results — run it to
        // completion, no budget. (The frontend boot has its own timeout-and-
        // proceed, so a slow first sweep never freezes the UI.)
        None => store(detect_all().await),
        // Warm-ish: bound the refresh; on overrun keep serving the previous
        // (still-valid) sweep rather than block. Never empty here.
        Some(prev) => match tokio::time::timeout(DETECT_ALL_BUDGET, detect_all()).await {
            Ok(fresh) => store(fresh),
            Err(_) => {
                tracing::warn!(
                    target: "kronn::agent_detect",
                    "detect_all exceeded {}s — serving the previous sweep ({} agents)",
                    DETECT_ALL_BUDGET.as_secs(), prev.len(),
                );
                prev
            }
        },
    }
}

/// Drop the cached sweep so the next `detect_all_cached(false)` re-probes.
/// Called after install/uninstall so the UI reflects the change immediately.
pub fn invalidate_detect_cache() {
    if let Ok(mut guard) = DETECT_ALL_CACHE.lock() {
        *guard = None;
    }
}

/// Detect a single agent by checking if its binary exists in PATH or host bin dirs.
/// If no local binary is found but the agent has an npx package, probe runtime availability.
async fn detect_agent(def: &AgentDef) -> AgentDetection {
    // Check standard PATH first, then host-mounted bin directories
    let found = find_binary(def.binary);

    // RTK detection is agent-type-scoped: the binary check is global but the
    // hook check reads the specific agent's config file. Agents with no
    // shell-exec (Vibe) or no hookable config (Ollama) will return false.
    let rtk_available = crate::core::rtk_detect::rtk_binary_available();
    let rtk_hook_configured = crate::core::rtk_detect::rtk_hook_configured_for(&def.agent_type);

    let runtime_warning = detect_runtime_warning(&def.agent_type);

    if let Some(loc) = found {
        // Version detection may fail (broken symlinks) OR hang — some agent
        // CLIs do a network/update check on `--version` and can stall for
        // 10s+, which gated the whole sweep. Cap it: the agent is already
        // confirmed INSTALLED (find_binary hit); a slow/failed `--version`
        // just means no version string, never a missing agent.
        let version = match tokio::time::timeout(
            std::time::Duration::from_secs(3),
            get_version_from(&loc.path),
        )
        .await
        {
            Ok(Ok(v)) => Some(v),
            _ => None,
        };
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
            latest_version: crate::core::versions::latest_known_agent_version(&def.agent_type)
                .map(|s| s.to_string()),
            origin: def.origin.to_string(),
            install_command: Some(def.install_cmd.to_string()),
            host_managed: loc.host_managed,
            host_label,
            runtime_available: true,
            rtk_available,
            rtk_hook_configured,
            runtime_warning,
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
            latest_version: crate::core::versions::latest_known_agent_version(&def.agent_type)
                .map(|s| s.to_string()),
            origin: def.origin.to_string(),
            install_command: Some(def.install_cmd.to_string()),
            host_managed: false,
            host_label: None,
            runtime_available,
            rtk_available,
            rtk_hook_configured,
            runtime_warning,
        }
    }
}

/// Surface a per-agent runtime-degradation warning to the frontend.
///
/// Today only Vibe sets one: when `vibe-runner.py` has hit the SDK
/// signature mismatch in this boot cycle, it writes a sentinel file
/// (cf. `_sdk_sentinel_path` in the runner). Subsequent agent calls
/// skip the SDK probe and run via the direct Mistral API path —
/// faster, but without local tools (bash, file I/O, MCP). The UI
/// shows a yellow note in Settings → Agents so the user knows
/// they're in fallback mode without having to read backend logs.
///
/// Returns the i18n key the frontend will resolve, or `None` when the
/// agent is running in its primary mode.
fn detect_runtime_warning(agent_type: &AgentType) -> Option<String> {
    if !matches!(agent_type, AgentType::Vibe) {
        return None;
    }
    // Mirror of vibe-runner.py:_sdk_sentinel_path. Per-uid suffix +
    // XDG_RUNTIME_DIR / TMPDIR / /tmp fallback. Auto-clears on
    // machine reboot, so this warning is self-healing.
    let base = std::env::var("XDG_RUNTIME_DIR")
        .or_else(|_| std::env::var("TMPDIR"))
        .unwrap_or_else(|_| "/tmp".into());
    let uid_suffix = if cfg!(unix) {
        // Pull libc::getuid via a tiny detour: read /proc/self/status. Avoids
        // pulling a libc dep just for this.
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("Uid:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .map(|n| n.to_string())
            })
            .unwrap_or_else(|| "unknown".to_string())
    } else {
        "win".to_string()
    };
    let sentinel = std::path::Path::new(&base).join(format!("kronn-vibe-no-sdk-{}", uid_suffix));
    if sentinel.exists() {
        Some("vibe.sdk_fallback".to_string())
    } else {
        None
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
        AgentType::Vibe => None,   // uvx, handled differently
        AgentType::Kiro => None,   // Native binary, no npx package
        AgentType::Ollama => None, // Native binary, own installer
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
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output()).await;

    let available = match result {
        Ok(Ok(output)) => output.status.success(),
        _ => false,
    };

    // Update cache
    if let Ok(mut cache) = RUNTIME_CACHE.lock() {
        cache.insert(cache_key, (available, Instant::now()));
    }

    tracing::info!(
        "Runtime probe for {}: {}",
        def.name,
        if available { "OK" } else { "unavailable" }
    );
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

    tracing::debug!(
        target: "kronn::agent_detect",
        "find_binary('{}'): PATH={:?}, host_dirs={:?}",
        name,
        std::env::var("PATH").ok(),
        host_dirs,
    );

    // Standard PATH
    if let Ok(path) = which::which(name) {
        let resolved = path.to_string_lossy().to_string();
        // If the binary resolved by `which` lives under a KRONN_HOST_BIN directory,
        // it is host-managed (mounted from the host into the container).
        let host_managed = host_dirs.iter().any(|dir| path.starts_with(dir));
        // A Darwin host binary reachable via PATH (the container mounts the
        // host `~/.local/bin` onto PATH) must NOT be returned — it can't exec
        // here. Fall through to the host-dir scan / Linux-package probes /
        // npx runtime fallback instead of surfacing an "Exec format error".
        if should_skip_darwin_host_binary(name, host_managed, host_is_macos()) {
            tracing::debug!(
                target: "kronn::agent_detect",
                "find_binary('{}'): skipping Darwin host-mounted binary resolved via PATH at {} (macOS host — using Linux copy / npx runtime instead)",
                name, resolved,
            );
        } else {
            tracing::debug!(
                target: "kronn::agent_detect",
                "find_binary('{}'): resolved via PATH -> {} (host_managed={})",
                name, resolved, host_managed,
            );
            return Some(BinaryLocation {
                path: resolved,
                host_managed,
                via_wsl: false,
            });
        }
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
                    // On macOS hosts, host-mounted binaries are Darwin
                    // binaries that cannot execute in this Linux container.
                    // The entrypoint.sh installs Linux versions via npm/curl.
                    // These entries live under a host-mount dir, so they are
                    // host_managed by construction. Source of truth for the
                    // skip list is MACOS_HOST_BIN_SKIP above — a test enforces
                    // that every npm agent is covered.
                    if should_skip_darwin_host_binary(name, true, host_is_macos()) {
                        tracing::debug!(
                            target: "kronn::agent_detect",
                            "find_binary('{}'): skipping Darwin host-mounted binary at {} (macOS host — Linux copy should come from entrypoint.sh)",
                            name,
                            entry.path().display(),
                        );
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

    // On Linux native: probe non-PATH package manager locations.
    // `which::which` only checks $PATH, but Snap/Flatpak/Nix install binaries
    // outside the standard PATH on minimal Tauri-launched sessions. We probe
    // explicitly so users with `snap install …` or NixOS profiles get
    // proper detection without having to fix their shell rc files.
    #[cfg(target_os = "linux")]
    {
        let mut linux_dirs: Vec<std::path::PathBuf> = vec![
            std::path::PathBuf::from("/snap/bin"),
            std::path::PathBuf::from("/var/lib/flatpak/exports/bin"),
            std::path::PathBuf::from("/run/current-system/sw/bin"),
        ];
        if let Ok(home) = std::env::var("HOME") {
            linux_dirs.push(std::path::PathBuf::from(format!(
                "{}/.local/share/flatpak/exports/bin",
                home
            )));
            linux_dirs.push(std::path::PathBuf::from(format!(
                "{}/.nix-profile/bin",
                home
            )));
            linux_dirs.push(std::path::PathBuf::from(format!("{}/.asdf/shims", home)));
            linux_dirs.push(std::path::PathBuf::from(format!("{}/.local/bin", home)));
        }
        for dir in &linux_dirs {
            let candidate = dir.join(name);
            if candidate.exists() {
                return Some(BinaryLocation {
                    path: candidate.to_string_lossy().to_string(),
                    host_managed: false,
                    via_wsl: false,
                });
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
        // SECURITY: `name` is interpolated into a shell command. Reject anything
        // that isn't a plain executable identifier so a future caller can never
        // turn this into a command-injection sink. Agent names are static
        // strings (claude, codex, gemini, …) so this is purely defensive.
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            tracing::warn!("Refusing WSL binary lookup for suspicious name: {:?}", name);
            return None;
        }

        // Login-shell lookup. We pass the binary name through `command -v` rather
        // than `which` so it works even when which is not installed inside the
        // WSL distro. Single-quoted to avoid shell interpolation surprises.
        let mut cmd = sync_cmd("wsl.exe");
        cmd.args(["-e", "bash", "-lc", &format!("command -v '{}'", name)]);
        if let Ok(output) = cmd.output() {
            if output.status.success() {
                let wsl_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !wsl_path.is_empty() {
                    return Some(BinaryLocation {
                        path: wsl_path,
                        host_managed: true,
                        via_wsl: true,
                    });
                }
            }
        }

        // Fallback: probe well-known directories inside WSL.
        // Each path is double-quoted in the shell so spaces in $HOME (rare on
        // WSL but possible with custom usernames) cannot break the test out of
        // its argument; the binary name is constrained above to safe ASCII.
        let probe_paths = [
            format!("\"$HOME/.local/bin/{}\"", name),
            format!("\"$HOME/.kiro/bin/{}\"", name),
            format!("\"/usr/local/bin/{}\"", name),
            format!("\"$HOME/.npm-global/bin/{}\"", name),
        ];
        let test_script = probe_paths
            .iter()
            .map(|p| format!("test -x {} && echo {}", p, p))
            .collect::<Vec<_>>()
            .join(" || ");
        let mut cmd = sync_cmd("wsl.exe");
        cmd.args(["-e", "bash", "-c", &test_script]);
        if let Ok(output) = cmd.output() {
            if output.status.success() {
                let wsl_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !wsl_path.is_empty() {
                    return Some(BinaryLocation {
                        path: wsl_path,
                        host_managed: true,
                        via_wsl: true,
                    });
                }
            }
        }
    }

    tracing::debug!(
        target: "kronn::agent_detect",
        "find_binary('{}'): NOT FOUND (not in PATH, not in host_dirs, not in platform-specific fallbacks)",
        name,
    );
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
                    .output()
                    .await?
            } else {
                async_cmd(binary_path).arg("--version").output().await?
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            async_cmd(binary_path).arg("--version").output().await?
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
        AgentType::ClaudeCode | AgentType::Codex | AgentType::GeminiCli | AgentType::CopilotCli => {
            Some((
                "npm",
                "Node.js is required. Install it from https://nodejs.org",
            ))
        }
        AgentType::Vibe => Some((
            "uv",
            "uv is required. Install it from https://docs.astral.sh/uv",
        )),
        AgentType::Kiro | AgentType::Ollama | AgentType::Custom => None,
    }
}

pub async fn install_agent(agent_type: &AgentType) -> Result<String> {
    let def = KNOWN_AGENTS
        .iter()
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
    let def = KNOWN_AGENTS
        .iter()
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
        #[cfg(unix)]
        AgentType::Ollama => "sudo rm -f $(which ollama)",
        #[cfg(windows)]
        AgentType::Ollama => "winget uninstall Ollama.Ollama",
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
        // With no override, detection falls back to the REAL host OS. Assert
        // against the actual build target so this is correct whether the suite
        // runs on Linux (CI/Docker) OR natively on macOS — the latter is now a
        // supported Mac dev path (`./kronn start-dev`), where a hardcoded
        // `!host_is_macos()` would wrongly fail.
        assert_eq!(
            host_is_macos(),
            cfg!(target_os = "macos"),
            "unset env should fall back to the real host OS"
        );
    }

    // ─── run_shell_cmd ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn run_shell_cmd_echo_hello() {
        let output = super::run_shell_cmd("echo hello")
            .await
            .expect("run_shell_cmd should succeed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("hello"),
            "stdout should contain 'hello', got: {}",
            stdout
        );
    }

    // ─── detect_all cache (boot-hang fix) ────────────────────────────────────

    #[tokio::test]
    #[serial]
    async fn detect_all_cached_serves_fresh_entry_without_resweeping() {
        // Seed a fresh sentinel: an EMPTY sweep, which a real `detect_all`
        // never produces (it lists every KNOWN_AGENT). If `detect_all_cached`
        // returns empty, it served the cache instead of spawning subprocesses.
        {
            let mut g = DETECT_ALL_CACHE.lock().unwrap();
            *g = Some((Vec::new(), Instant::now()));
        }
        let hit = detect_all_cached(false).await;
        assert!(
            hit.is_empty(),
            "within TTL the cached sweep must be served as-is (no re-probe)"
        );
        invalidate_detect_cache(); // leave the static clean for other tests
    }

    #[tokio::test]
    #[serial]
    async fn invalidate_detect_cache_drops_the_entry() {
        {
            let mut g = DETECT_ALL_CACHE.lock().unwrap();
            *g = Some((Vec::new(), Instant::now()));
        }
        invalidate_detect_cache();
        assert!(
            DETECT_ALL_CACHE.lock().unwrap().is_none(),
            "invalidate must drop the cached entry so the next call re-probes",
        );
    }

    // ─── check_prerequisite ──────────────────────────────────────────────────

    #[test]
    fn check_prerequisite_sh_exists() {
        assert!(
            super::check_prerequisite("sh"),
            "sh should be available on unix"
        );
    }

    #[test]
    fn check_prerequisite_nonexistent_binary() {
        assert!(
            !super::check_prerequisite("nonexistent_binary_xyz"),
            "nonexistent binary should not be found"
        );
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
        let found = KNOWN_AGENTS
            .iter()
            .any(|a| matches!(a.agent_type, AgentType::CopilotCli));
        assert!(found, "CopilotCli should be in KNOWN_AGENTS");
        let def = KNOWN_AGENTS
            .iter()
            .find(|a| matches!(a.agent_type, AgentType::CopilotCli))
            .unwrap();
        assert_eq!(def.binary, "copilot");
        assert_eq!(def.origin, "US");
    }

    #[test]
    fn copilot_npx_package_in_probe_runtime() {
        let def = KNOWN_AGENTS
            .iter()
            .find(|a| matches!(a.agent_type, AgentType::CopilotCli))
            .unwrap();
        let pkg = match def.agent_type {
            AgentType::CopilotCli => Some("@github/copilot"),
            _ => None,
        };
        assert_eq!(pkg, Some("@github/copilot"));
    }

    // ═══════════════════════════════════════════════════════════════════
    // Cross-agent regression tests (auto-extends when agents are added)
    // ═══════════════════════════════════════════════════════════════════
    //
    // These tests iterate over ALL known agents and verify that each one
    // has a complete configuration. When you add a new AgentType variant
    // or a new KNOWN_AGENTS entry, these tests fail if you forget any
    // of the supporting pieces (runner config, DB serialization, etc.).

    /// All non-Custom AgentType variants must have a KNOWN_AGENTS entry.
    #[test]
    fn cross_agent_every_type_in_known_agents() {
        let all_types = [
            AgentType::ClaudeCode,
            AgentType::Codex,
            AgentType::Vibe,
            AgentType::GeminiCli,
            AgentType::Kiro,
            AgentType::CopilotCli,
            AgentType::Ollama,
        ];
        for agent_type in &all_types {
            let found = KNOWN_AGENTS.iter().any(|a| {
                std::mem::discriminant(&a.agent_type) == std::mem::discriminant(agent_type)
            });
            assert!(
                found,
                "AgentType::{:?} is missing from KNOWN_AGENTS — add an AgentDef entry",
                agent_type
            );
        }
        // Reverse: every KNOWN_AGENTS entry must map to a real AgentType
        assert_eq!(
            KNOWN_AGENTS.len(),
            all_types.len(),
            "KNOWN_AGENTS has {} entries but we expect {} (one per non-Custom AgentType)",
            KNOWN_AGENTS.len(),
            all_types.len()
        );
    }

    /// Every KNOWN_AGENTS entry must have a non-empty binary name and install command.
    #[test]
    fn cross_agent_definitions_are_complete() {
        for def in KNOWN_AGENTS {
            assert!(
                !def.binary.is_empty(),
                "{:?} has empty binary name",
                def.agent_type
            );
            assert!(
                !def.install_cmd.is_empty(),
                "{:?} has empty install_cmd",
                def.agent_type
            );
            assert!(
                !def.name.is_empty(),
                "{:?} has empty display name",
                def.agent_type
            );
            assert!(
                !def.origin.is_empty(),
                "{:?} has empty origin",
                def.agent_type
            );
        }
    }

    /// Every agent in KNOWN_AGENTS must NOT be the Custom variant.
    /// runner.rs has exhaustive match on AgentType — the compiler enforces
    /// that every variant has a command builder. This test documents that
    /// KNOWN_AGENTS only contains real agents.
    #[test]
    fn cross_agent_no_custom_in_known_agents() {
        for def in KNOWN_AGENTS {
            assert!(
                !matches!(def.agent_type, AgentType::Custom),
                "KNOWN_AGENTS must not contain Custom — it has no CLI binary"
            );
        }
    }

    /// The macOS binary skip list must include ALL npm-based agents.
    /// If you add a new npm agent, it needs to be in the skip list
    /// (otherwise macOS Docker users get a Darwin binary that can't execute).
    #[test]
    fn cross_agent_macos_skip_covers_npm_agents() {
        // ENFORCES (no longer just documents) that every npm-installed agent
        // is present in the macOS skip list. If this test fails after adding
        // a new npm agent, update BOTH `MACOS_HOST_BIN_SKIP` and
        // `backend/entrypoint.sh`.
        let npm_agents: Vec<&str> = KNOWN_AGENTS
            .iter()
            .filter(|d| d.install_cmd.starts_with("npm "))
            .map(|d| d.binary)
            .collect();

        assert!(npm_agents.len() >= 4,
            "Expected at least 4 npm agents, found {}. If you added one, extend MACOS_HOST_BIN_SKIP + entrypoint.sh.",
            npm_agents.len());

        for binary in &npm_agents {
            assert!(
                MACOS_HOST_BIN_SKIP.contains(binary),
                "macOS skip list MUST include '{}' — without it, macOS users \
                 get the Darwin binary from the host mount which can't execute \
                 in the Linux container. Add it to MACOS_HOST_BIN_SKIP in \
                 agents/mod.rs AND to backend/entrypoint.sh.",
                binary,
            );
        }
    }

    /// Regression guard for the gemini + copilot bug reported 2026-04-15:
    /// macOS users couldn't see Gemini detected because `gemini` was missing
    /// from the skip list despite being npm-installed.
    #[test]
    fn macos_skip_list_includes_gemini_and_copilot() {
        assert!(
            MACOS_HOST_BIN_SKIP.contains(&"gemini"),
            "regression: gemini dropped from macOS skip list"
        );
        assert!(
            MACOS_HOST_BIN_SKIP.contains(&"copilot"),
            "regression: copilot dropped from macOS skip list"
        );
    }

    /// The Darwin-host skip decision must fire for a host-managed skip-listed
    /// binary on a macOS host. Regression for the "claude → Exec format error"
    /// bug: a Darwin `claude` reachable via the container PATH (host mount)
    /// was returned by `which::which` because the skip was only checked in the
    /// fallback dir-scan, never on the primary PATH resolution.
    #[test]
    fn should_skip_darwin_host_binary_fires_on_macos_host_managed() {
        assert!(
            should_skip_darwin_host_binary("claude", true, true),
            "host-managed claude on a macOS host must be skipped"
        );
    }

    /// Must NOT skip when the binary is not host-managed: a Linux `claude`
    /// installed by entrypoint.sh into the container (e.g. ~/.local/bin) is
    /// runnable and must be returned even on a macOS host.
    #[test]
    fn should_skip_darwin_host_binary_keeps_container_linux_copy() {
        assert!(
            !should_skip_darwin_host_binary("claude", false, true),
            "a non-host-managed (in-container Linux) claude must be kept"
        );
    }

    /// Must NOT skip on a non-macOS host: a host-mounted binary on Linux/WSL is
    /// a Linux binary and runs fine in the container.
    #[test]
    fn should_skip_darwin_host_binary_inert_off_macos() {
        assert!(
            !should_skip_darwin_host_binary("claude", true, false),
            "host-managed claude on a non-macOS host must NOT be skipped"
        );
    }

    /// Must NOT skip a binary that isn't in the skip list, even on macOS.
    #[test]
    fn should_skip_darwin_host_binary_only_for_listed_names() {
        assert!(
            !should_skip_darwin_host_binary("ollama", true, true),
            "ollama is not in MACOS_HOST_BIN_SKIP and must not be skipped"
        );
        // Every listed agent is covered when host-managed on macOS.
        for name in MACOS_HOST_BIN_SKIP {
            assert!(
                should_skip_darwin_host_binary(name, true, true),
                "{} is in the skip list but was not skipped",
                name
            );
        }
    }
}
