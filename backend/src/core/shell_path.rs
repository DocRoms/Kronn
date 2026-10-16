//! Login-shell PATH discovery, shared by the desktop app and the backend run
//! as a background service (KRONN_SERVICE=1).
//!
//! A GUI app (macOS) or a launchd / systemd unit starts with a minimal PATH
//! (`/usr/bin:/bin:…`), so agent CLIs installed through a shell profile
//! (`claude`, `codex`, nvm, homebrew…) are invisible. Both entry points ask
//! the user's login shell for its PATH and merge it into their own.

/// Maximum time the desktop app waits for the user's login shell to print
/// its PATH. 5 seconds is enough for normal zsh/bash startups but kills
/// shells that hang on plugin downloads, broken `compinit`, or interactive
/// prompts.
#[cfg(unix)]
const SHELL_PATH_TIMEOUT_SECS: u64 = 5;

/// The service waits longer: it starts at login or during an update, when
/// the machine is busy and a healthy zsh can take more than 5 s. Missing the
/// PATH there hides every agent CLI until the next restart.
#[cfg(unix)]
const SERVICE_SHELL_PATH_TIMEOUT_SECS: u64 = 20;

/// Messages emitted before the tracing subscriber exists, logged once it does.
pub type StartupLog = Vec<(tracing::Level, String)>;

/// Try to load the user's actual shell PATH by running their login shell.
/// Apps like VS Code use this technique to get the same PATH as Terminal.app.
///
/// Hardened against:
/// - SHELL not set / pointing to /bin/false / non-existent shell
/// - Slow/hanging .zshrc, .bashrc (timeout + kill)
/// - Shells that fail with -i -l (e.g. fish strict mode) — stderr is logged
/// - Empty SHELL var
///
/// Nothing is emitted through tracing (it may not exist yet): messages are
/// pushed to `log` for the caller to emit.
#[cfg(unix)]
pub fn shell_path_from_user_shell(log: &mut StartupLog) -> Option<String> {
    shell_path_within(SHELL_PATH_TIMEOUT_SECS, log)
}

#[cfg(unix)]
fn shell_path_within(timeout_secs: u64, log: &mut StartupLog) -> Option<String> {
    let shell = crate::core::child_env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty() && s != "/bin/false" && std::path::Path::new(s).exists())
        .unwrap_or_else(|| "/bin/bash".to_string());

    // Spawn the shell as a child process so we can enforce a timeout.
    // -i = interactive (sources .zshrc/.bashrc), -l = login (sources .profile)
    // The rc files run with the base allow-list (HOME, PATH, SHELL…), never
    // the whole environment (KT-1006).
    let mut child = match crate::core::cmd::sync_tool_cmd(&shell)
        .args(["-i", "-l", "-c", "echo $PATH"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log.push((
                tracing::Level::WARN,
                format!("Cannot spawn shell {shell} for PATH discovery: {e}"),
            ));
            return None;
        }
    };

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = match child.wait_with_output() {
                    Ok(o) => o,
                    Err(e) => {
                        log.push((
                            tracing::Level::WARN,
                            format!("Failed to read shell output: {e}"),
                        ));
                        return None;
                    }
                };
                if !status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    log.push((
                        tracing::Level::DEBUG,
                        format!(
                            "Shell {shell} exited with {status} when probing PATH; stderr: {}",
                            stderr.trim()
                        ),
                    ));
                    return None;
                }
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                return if path.is_empty() { None } else { Some(path) };
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    log.push((
                        tracing::Level::WARN,
                        format!(
                            "Shell {shell} did not return PATH within {timeout_secs}s — killing and falling back to defaults"
                        ),
                    ));
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                log.push((
                    tracing::Level::WARN,
                    format!("Failed to poll shell child: {e}"),
                ));
                let _ = child.kill();
                return None;
            }
        }
    }
}

#[cfg(not(unix))]
pub fn shell_path_from_user_shell(_log: &mut StartupLog) -> Option<String> {
    None
}

/// Per-user and package-manager bin directories where agent CLIs usually
/// live, for when the login shell gives no PATH. Only existing directories
/// are returned; for nvm, the newest installed Node.
pub fn fallback_bin_dirs(home: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = [
        ".local/bin",
        ".cargo/bin",
        ".npm-global/bin",
        ".bun/bin",
        ".volta/bin",
    ]
    .iter()
    .map(|d| home.join(d))
    .collect();
    if let Some(node) = newest_nvm_node(&home.join(".nvm/versions/node")) {
        dirs.push(node.join("bin"));
    }
    for d in ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin"] {
        dirs.push(std::path::PathBuf::from(d));
    }
    dirs.retain(|d| d.is_dir());
    dirs
}

/// The `vX.Y.Z` directory with the highest version under nvm's node dir.
fn newest_nvm_node(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let parse = |name: &str| -> Option<Vec<u64>> {
        name.strip_prefix('v')?
            .split('.')
            .map(|p| p.parse().ok())
            .collect()
    };
    std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let version = parse(e.file_name().to_str()?)?;
            Some((version, e.path()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, path)| path)
}

/// Append to `current` the entries of `extra` it does not already contain.
/// Existing entries keep their order (and so their precedence); empty
/// entries are dropped from `extra`.
pub fn merge_path(current: &str, extra: &str, separator: char) -> String {
    let mut entries: Vec<&str> = current.split(separator).collect();
    for dir in extra.split(separator) {
        if !dir.is_empty() && !entries.contains(&dir) {
            entries.push(dir);
        }
    }
    entries.join(&separator.to_string())
}

/// Service mode (KRONN_SERVICE=1): merge the login-shell PATH into this
/// process's PATH. It writes the live environment, so it must run before any
/// thread exists. Messages are returned for the log, which starts later.
pub fn enrich_path_for_service() -> StartupLog {
    let mut log = StartupLog::new();
    let separator = if cfg!(windows) { ';' } else { ':' };
    #[cfg(unix)]
    let shell_path = shell_path_within(SERVICE_SHELL_PATH_TIMEOUT_SECS, &mut log);
    #[cfg(not(unix))]
    let shell_path = shell_path_from_user_shell(&mut log);
    let current = crate::core::child_env::var("PATH").unwrap_or_default();
    let extra = match shell_path {
        Some(path) => {
            log.push((
                tracing::Level::INFO,
                "Service mode: loaded PATH from user shell".into(),
            ));
            path
        }
        None => {
            let home = crate::core::child_env::var("HOME").unwrap_or_default();
            let dirs = fallback_bin_dirs(std::path::Path::new(&home));
            let joined = dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(&separator.to_string());
            log.push((
                tracing::Level::WARN,
                format!(
                    "Service mode: no PATH from the user shell; added known bin dirs: {joined}"
                ),
            ));
            joined
        }
    };
    crate::core::child_env::set_var("PATH", merge_path(&current, &extra, separator));
    log
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_existing_order_and_appends_missing() {
        assert_eq!(
            merge_path(
                "/usr/bin:/bin",
                "/opt/homebrew/bin:/usr/bin:/home/u/.nvm/bin",
                ':'
            ),
            "/usr/bin:/bin:/opt/homebrew/bin:/home/u/.nvm/bin"
        );
    }

    #[test]
    fn merge_skips_empty_extra_entries() {
        assert_eq!(merge_path("/a", "", ':'), "/a");
        assert_eq!(merge_path("/a", "/b::/c", ':'), "/a:/b:/c");
        assert_eq!(merge_path("/a", "/a", ';'), "/a");
    }

    #[test]
    fn fallback_keeps_existing_dirs_and_the_newest_nvm_node() {
        let home = std::env::temp_dir().join(format!("kronn-fallback-{}", std::process::id()));
        for d in [
            ".local/bin",
            ".nvm/versions/node/v9.11.2/bin",
            ".nvm/versions/node/v24.14.0/bin",
            ".nvm/versions/node/v18.20.4/bin",
        ] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        let dirs = fallback_bin_dirs(&home);
        let _ = std::fs::remove_dir_all(&home);
        assert!(dirs.contains(&home.join(".local/bin")));
        assert!(dirs.contains(&home.join(".nvm/versions/node/v24.14.0/bin")));
        assert!(!dirs.contains(&home.join(".nvm/versions/node/v9.11.2/bin")));
        assert!(
            !dirs.contains(&home.join(".cargo/bin")),
            "absent dirs are dropped"
        );
    }

    /// A failing login shell returns no PATH and leaves a message for the
    /// log (nothing is dropped).
    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn failing_shell_returns_no_path_and_reports() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("kronn-shell-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shell = dir.join("failing-shell");
        std::fs::write(&shell, "#!/bin/sh\necho broken >&2\nexit 3\n").unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let prev = crate::core::child_env::var("SHELL").ok();
        crate::core::child_env::set_var("SHELL", &shell);
        let mut log = Vec::new();
        let result = shell_path_from_user_shell(&mut log);
        match prev {
            Some(s) => crate::core::child_env::set_var("SHELL", s),
            None => crate::core::child_env::remove_var("SHELL"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(result, None);
        assert!(
            log.iter()
                .any(|(_, m)| m.contains("exited with") && m.contains("broken")),
            "{log:?}"
        );
    }
}
