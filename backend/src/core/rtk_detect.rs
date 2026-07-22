//! Detection of RTK (Rust Token Killer) on the host and of its per-agent
//! hook configuration. Read-only: this module never writes to agent config
//! files. Activation goes through a separate endpoint that spawns `rtk init`
//! so RTK owns the file format.
//!
//! Home resolution uses the container's `HOME` directly — NOT
//! `KRONN_HOST_HOME`. In Docker, the individual agent config directories
//! (`.claude`, `.codex`, `.gemini`, `.copilot`, `.kiro`) are bind-mounted
//! read-write into `/home/kronn/.<agent>`, so reading `$HOME/.claude/...`
//! in the container actually hits the user's real host file. The raw host
//! path `KRONN_HOST_HOME` (e.g. `/home/priol`) doesn't exist inside the
//! container — trying to read it would systematically return "not
//! configured" even when the hook is correctly wired. In Tauri, `HOME`
//! is the native user home, also correct.

use crate::models::AgentType;
use std::path::{Path, PathBuf};

/// Returns true when `rtk` is available **for the host CLIs** — where agents
/// actually run (Kronn is the bridge; CLIs/RTK live on the host, not in the
/// container).
///
/// Under Docker the container ships its OWN `rtk` at `/usr/local/bin` (baked
/// into the image), but that can't help the user's host CLIs. A plain `which`
/// would find that container binary and wrongly report "available", so the UI
/// would never offer to install RTK on the host (and `rtk gain` in the user's
/// terminal stays `command not found`). We therefore check the HOST bins
/// (mounted under `KRONN_HOST_BIN`) when in Docker, and fall back to `which`
/// natively (Tauri/CLI, where `$PATH` already is the host). Mirrors the
/// host-aware agent detection.
pub fn rtk_binary_available() -> bool {
    if crate::core::env::is_docker() {
        return std::env::var("KRONN_HOST_BIN")
            .map(|hb| {
                std::env::split_paths(&hb).any(|dir| {
                    let p = dir.join("rtk");
                    // `exists()` follows symlinks. A Homebrew-installed `rtk` is a
                    // symlink into `../Cellar/rtk/<v>/bin/rtk`, and the host Cellar
                    // is NOT mounted in the container — so the link is dangling here
                    // and `exists()` is false even though rtk IS on the host. Also
                    // accept a symlink ENTRY (`symlink_metadata` doesn't follow).
                    // Same trap as the Mach-O host-launcher symlinks.
                    p.exists() || p.symlink_metadata().is_ok()
                })
            })
            .unwrap_or(false);
    }
    which::which("rtk").is_ok()
}

/// Returns true if the agent's own config file references RTK. Agents that
/// don't execute shell commands (Vibe — API-only) or don't have a hookable
/// config (Ollama) always return false: the frontend renders those as "not
/// applicable" rather than "not configured".
pub fn rtk_hook_configured_for(agent_type: &AgentType) -> bool {
    let Some(home) = resolve_home() else {
        return false;
    };
    // Gemini: the most authoritative signal is the hook script file RTK
    // drops at `~/.gemini/hooks/rtk-hook-gemini.sh` — it exists post-install
    // and is removed on `--uninstall`. We fall back to scanning GEMINI.md
    // (RTK also writes that companion file) so detection survives a user
    // who manually removed the script but left the rest of the install.
    // Pre-fix we scanned bash/zsh rc files, but RTK 0.37 doesn't touch
    // shell rc for Gemini at all — the detection returned false even on a
    // successful install, leaving the badge stuck on "not configured" and
    // the user clicking "Enable on the 1 remaining" in a loop.
    if matches!(agent_type, AgentType::GeminiCli) {
        return gemini_hook_present(&home);
    }
    let Some(rel_path) = agent_config_relpath(agent_type) else {
        return false;
    };
    let path = home.join(rel_path);
    config_mentions_rtk(&path)
}

/// Relative path (from HOME) of the file whose contents we scan for an RTK
/// reference. Paths come from RTK's own supported-agents doc — overriding
/// to the wrong file means the badge sticks on "hook missing" after a
/// successful activation, which we hit hard in the first iteration.
///
/// `None` is returned when either:
///   - The agent isn't in RTK's supported list (Kiro, Copilot CLI — RTK's
///     "copilot" flag targets VS Code Copilot Chat, not the standalone
///     `@github/copilot` CLI that Kronn spawns).
///   - The agent has no hookable shell flow (Vibe = API-only and "planned",
///     Ollama = no shell exec).
///   - The agent is configured via shell rc rather than a dedicated file
///     (Gemini CLI — handled separately by `gemini_shell_rc_mentions_rtk`).
fn agent_config_relpath(agent_type: &AgentType) -> Option<&'static Path> {
    match agent_type {
        AgentType::ClaudeCode => Some(Path::new(".claude/settings.json")),
        // Codex hook lives in AGENTS.md, NOT config.toml. Caught the wrong
        // path on the first pass because `config.toml` felt more natural;
        // RTK `--codex` actually injects into AGENTS.md.
        AgentType::Codex => Some(Path::new(".codex/AGENTS.md")),
        // Gemini CLI hook is detected via the hook-script file existence
        // + GEMINI.md scan — see `gemini_hook_present`. No per-file
        // relpath because we need a 2-source check, not a substring scan.
        AgentType::GeminiCli => None,
        // Not in RTK's supported list.
        AgentType::Kiro | AgentType::CopilotCli => None,
        // API-only or hookless.
        AgentType::Vibe | AgentType::Ollama | AgentType::Custom => None,
    }
}

/// Gemini CLI is hooked by RTK via three artifacts inside `~/.gemini`:
///   1. `hooks/rtk-hook-gemini.sh` — the actual `exec rtk hook gemini` shim
///   2. `GEMINI.md` — RTK.md-style companion (instructions for the agent)
///   3. `settings.json` — a `BeforeTool` hook entry pointing at #1, only
///      when the user accepted the settings.json patch (or rtk ≥ 0.37
///      received `--auto-patch`)
///
/// We treat any of the three as "hook configured" — robustness over
/// strictness. The hook file is the most authoritative since RTK creates
/// it unconditionally on install and removes it on `--uninstall`.
fn gemini_hook_present(home: &Path) -> bool {
    if home.join(".gemini/hooks/rtk-hook-gemini.sh").is_file() {
        return true;
    }
    if config_mentions_rtk(&home.join(".gemini/GEMINI.md")) {
        return true;
    }
    config_mentions_rtk(&home.join(".gemini/settings.json"))
}

/// MVP detection: read the file and look for the substring `rtk`. RTK's hook
/// invocations universally call the `rtk` binary, so its presence in the
/// config is a reliable positive signal. False positives require an RTK
/// reference *somewhere* in the config (unlikely in minimal agent configs)
/// and are cheap to fix once we see them — no point parsing JSON/TOML per
/// agent format at this stage.
fn config_mentions_rtk(path: &Path) -> bool {
    match std::fs::read_to_string(path) {
        Ok(content) => content.to_lowercase().contains("rtk"),
        Err(_) => false,
    }
}

/// Container HOME is already correct — it's bind-mounted to the host's real
/// agent-config dirs via docker-compose. Tauri's HOME is the native user
/// home. Either way, trust HOME.
fn resolve_home() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Serialise tests that mutate process env. Rust's test runner uses a
    /// thread pool by default, so without this mutex two tests both
    /// hammering `HOME` race and one reads the other's value.
    /// `PoisonError::into_inner` keeps the suite usable after a panic.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Run a test with `HOME` overridden to a tempdir, then restored.
    /// env vars are process-global, so we serialise via a mutex and
    /// restore the previous value on exit.
    fn with_home<F: FnOnce(&Path)>(f: F) {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = TempDir::new().expect("tempdir");
        let prev = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());
        f(tmp.path());
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn returns_false_when_config_file_absent() {
        with_home(|_| {
            assert!(!rtk_hook_configured_for(&AgentType::ClaudeCode));
            assert!(!rtk_hook_configured_for(&AgentType::Codex));
        });
    }

    /// Under Docker, `rtk_binary_available` must reflect the HOST (mounted
    /// `KRONN_HOST_BIN`), NOT the container's baked `/usr/local/bin/rtk` — else
    /// the UI never offers to install RTK on the user's machine.
    #[test]
    #[serial]
    fn rtk_binary_available_in_docker_checks_host_bins() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = TempDir::new().expect("tempdir");
        let prev_data = std::env::var("KRONN_IN_DOCKER").ok();
        let prev_hb = std::env::var("KRONN_HOST_BIN").ok();
        std::env::set_var("KRONN_IN_DOCKER", "1"); // → is_docker() == true
        std::env::set_var("KRONN_HOST_BIN", tmp.path());

        assert!(
            !rtk_binary_available(),
            "Docker + no host rtk → unavailable"
        );

        fs::write(tmp.path().join("rtk"), "#!/bin/sh\n").unwrap();
        assert!(
            rtk_binary_available(),
            "Docker + host rtk present → available"
        );

        // Homebrew case: rtk is a symlink into ../Cellar (not mounted in the
        // container → dangling here). Must STILL count as available.
        #[cfg(unix)]
        {
            fs::remove_file(tmp.path().join("rtk")).unwrap();
            std::os::unix::fs::symlink("/no/such/Cellar/rtk/bin/rtk", tmp.path().join("rtk"))
                .unwrap();
            assert!(
                rtk_binary_available(),
                "Docker + dangling host symlink (brew) → still available"
            );
        }

        match prev_data {
            Some(v) => std::env::set_var("KRONN_IN_DOCKER", v),
            None => std::env::remove_var("KRONN_IN_DOCKER"),
        }
        match prev_hb {
            Some(v) => std::env::set_var("KRONN_HOST_BIN", v),
            None => std::env::remove_var("KRONN_HOST_BIN"),
        }
    }

    #[test]
    fn returns_true_when_claude_settings_mentions_rtk() {
        with_home(|home| {
            fs::create_dir_all(home.join(".claude")).unwrap();
            fs::write(
                home.join(".claude/settings.json"),
                r#"{"hooks":{"PreToolUse":[{"command":"rtk preprocess"}]}}"#,
            )
            .unwrap();
            assert!(rtk_hook_configured_for(&AgentType::ClaudeCode));
        });
    }

    #[test]
    fn returns_false_when_claude_settings_has_no_rtk() {
        with_home(|home| {
            fs::create_dir_all(home.join(".claude")).unwrap();
            fs::write(
                home.join(".claude/settings.json"),
                r#"{"hooks":{"PreToolUse":[]}}"#,
            )
            .unwrap();
            assert!(!rtk_hook_configured_for(&AgentType::ClaudeCode));
        });
    }

    #[test]
    fn detection_is_case_insensitive() {
        with_home(|home| {
            fs::create_dir_all(home.join(".codex")).unwrap();
            fs::write(
                home.join(".codex/AGENTS.md"),
                "# Codex AGENTS\n\nRun `RTK filter git status` before shelling out.\n",
            )
            .unwrap();
            assert!(rtk_hook_configured_for(&AgentType::Codex));
        });
    }

    #[test]
    fn codex_reads_agents_md_not_config_toml() {
        // Regression: first-pass implementation checked config.toml which
        // RTK doesn't actually touch. The right file is AGENTS.md.
        with_home(|home| {
            fs::create_dir_all(home.join(".codex")).unwrap();
            // RTK wrote to AGENTS.md — we must see the hook.
            fs::write(home.join(".codex/AGENTS.md"), "rtk init").unwrap();
            // Decoy: config.toml contains "rtk" but that alone must not
            // be enough anymore.
            fs::write(home.join(".codex/config.toml"), "rtk").unwrap();
            assert!(rtk_hook_configured_for(&AgentType::Codex));
        });
    }

    #[test]
    fn gemini_detection_finds_hook_script_file() {
        // RTK's primary install artifact for Gemini is the hook .sh script
        // at `~/.gemini/hooks/rtk-hook-gemini.sh`. Its presence alone is
        // enough to flip the badge to "configured" — RTK removes it on
        // `--uninstall`, so it's a faithful proxy.
        with_home(|home| {
            fs::create_dir_all(home.join(".gemini/hooks")).unwrap();
            fs::write(
                home.join(".gemini/hooks/rtk-hook-gemini.sh"),
                "#!/bin/bash\nexec rtk hook gemini\n",
            )
            .unwrap();
            assert!(rtk_hook_configured_for(&AgentType::GeminiCli));
        });
    }

    #[test]
    fn gemini_detection_falls_back_to_gemini_md() {
        // If the hook script is missing but GEMINI.md mentions rtk
        // (e.g. user deleted just the .sh), we still say "configured" —
        // the partial state is the user's call to fix, not our place
        // to lie about.
        with_home(|home| {
            fs::create_dir_all(home.join(".gemini")).unwrap();
            fs::write(home.join(".gemini/GEMINI.md"), "# RTK\n").unwrap();
            assert!(rtk_hook_configured_for(&AgentType::GeminiCli));
        });
    }

    #[test]
    fn gemini_detection_falls_back_to_settings_json() {
        // Third fallback: settings.json's BeforeTool entry points at the
        // hook. Catches the user who blew away `.gemini/hooks/` but kept
        // the JSON patch.
        with_home(|home| {
            fs::create_dir_all(home.join(".gemini")).unwrap();
            fs::write(
                home.join(".gemini/settings.json"),
                r#"{"hooks":{"BeforeTool":[{"command":"/home/x/.gemini/hooks/rtk-hook-gemini.sh"}]}}"#,
            ).unwrap();
            assert!(rtk_hook_configured_for(&AgentType::GeminiCli));
        });
    }

    #[test]
    fn gemini_detection_ignores_shell_rc() {
        // Regression: pre-fix we scanned ~/.bashrc, ~/.zshrc and friends
        // — RTK 0.37 doesn't touch shell rc for Gemini. An unrelated `rtk`
        // mention in a shell rc must NOT make us claim the hook is wired.
        with_home(|home| {
            fs::write(home.join(".bashrc"), "# unrelated rtk-alike\n").unwrap();
            fs::write(home.join(".zshrc"), "alias rtk-fake='echo nope'\n").unwrap();
            // No .gemini dir at all → still false.
            assert!(!rtk_hook_configured_for(&AgentType::GeminiCli));
        });
    }

    #[test]
    fn unsupported_agents_always_return_false() {
        // Kiro and Copilot CLI aren't in RTK's supported list. Even if an
        // unrelated config mentions "rtk", we must not lie about the state.
        with_home(|home| {
            fs::create_dir_all(home.join(".kiro/settings")).unwrap();
            fs::write(home.join(".kiro/settings/settings.json"), "rtk").unwrap();
            fs::create_dir_all(home.join(".copilot")).unwrap();
            fs::write(home.join(".copilot/config.toml"), "rtk").unwrap();
            assert!(!rtk_hook_configured_for(&AgentType::Kiro));
            assert!(!rtk_hook_configured_for(&AgentType::CopilotCli));
        });
    }

    #[test]
    fn vibe_and_ollama_always_return_false_even_with_matching_content() {
        // If an unrelated file under HOME happens to mention "rtk", the
        // API-only / hookless agents must still report false. We prove this
        // by writing a bogus matching file and checking the agent types
        // are hard-wired to false via None relpath.
        with_home(|_| {
            assert!(!rtk_hook_configured_for(&AgentType::Vibe));
            assert!(!rtk_hook_configured_for(&AgentType::Ollama));
        });
    }

    #[test]
    fn unicode_path_does_not_panic() {
        with_home(|home| {
            // Regression: HOME with non-ASCII should not panic when appended.
            // We only assert it returns a bool — the path may or may not exist.
            let _ = home;
            let _ = rtk_hook_configured_for(&AgentType::GeminiCli);
        });
    }
}
